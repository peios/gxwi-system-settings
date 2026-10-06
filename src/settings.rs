//! The window: four sections of the machine's settings down the side, the
//! one chosen beside them.
//!
//! Each section reads what is set, and asks the system whether this person
//! may change it. What they may not change is shown as it is, with the
//! reason under its group. A choice — a switch, a list — applies the moment
//! it is made, and is put back with the reason if it can't be; what is typed
//! applies when asked to, with a button that appears once there is something
//! to apply. What is shown is read again in the background, so a value
//! changed elsewhere appears unless it is being edited here.

use std::sync::Weak;

use libauthd_client::ident::Ident;
use libgxwi::settings::{self, Glyph, Nav, Section, Tile};
use libgxwi::{Facts, Fields, Live, Surface, Value};
use libtimed::zone::Listed;

use crate::about::{self, About};
use crate::language::{self, Installed, Language};
use crate::startup::{self, Startup};
use crate::time::{self, Time};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Time,
    Language,
    Startup,
    About,
}

impl View {
    const ALL: [(View, &'static str); 4] = [(View::Time, "time"), (View::Language, "language"), (View::Startup, "startup"), (View::About, "about")];

    fn by(name: &str) -> Option<View> {
        View::ALL.iter().find(|(_, by)| *by == name).map(|(view, _)| *view)
    }

    fn id(self) -> &'static str {
        View::ALL.iter().find(|(view, _)| *view == self).map(|(_, by)| *by).unwrap_or("time")
    }
}

/// Everything read in one look, outside the window's lock.
pub struct Read {
    pub time: Time,
    pub language: Language,
    pub about: About,
    pub startup: Startup,
}

impl Read {
    pub fn now(ident: &Ident) -> Read {
        Read {
            time: time::read(),
            language: language::read(),
            about: about::read(),
            startup: startup::read(ident),
        }
    }
}

pub struct Settings {
    pub window: Weak<Surface<Settings>>,
    ident: Ident,
    view: View,
    time: Time,
    zones: Vec<Listed>,
    language: Language,
    installed: Installed,
    about: About,
    startup: Startup,
    /// The time servers' list is open to be edited.
    editing_servers: bool,
    /// What came of the last change: what was done, or why it wasn't.
    said: Option<Result<String, String>>,
    pub now: jiff::Timestamp,
}

impl Settings {
    pub fn new(ident: Ident, read: Read) -> Settings {
        Settings {
            window: Weak::new(),
            ident,
            view: View::Time,
            time: read.time,
            zones: libtimed::zone::list(),
            language: read.language,
            installed: language::installed(),
            about: read.about,
            startup: read.startup,
            editing_servers: false,
            said: None,
            now: jiff::Timestamp::now(),
        }
    }

    pub fn ident(&self) -> &Ident {
        &self.ident
    }

    /// Whether the clock shown would read differently at `now`: only the
    /// time section shows it, to the minute.
    pub fn minute_turned(&self, now: jiff::Timestamp) -> bool {
        self.view == View::Time && self.now.as_second() / 60 != now.as_second() / 60
    }

    /// Whether what was read differs from what is shown.
    pub fn differs(&self, read: &Read) -> bool {
        read.time != self.time || read.language != self.language || read.about != self.about || read.startup != self.startup
    }

    /// Puts what is set in every field.
    pub fn fill(&self, fields: &mut Fields) {
        time::fill(&self.time.policy, fields);
        language::fill(&self.language, fields);
        about::fill(&self.about, fields);
        startup::fill(&self.startup, fields);
    }

    /// What was read again. A field still holding what was set before
    /// follows what is set now; one being edited is left.
    pub fn heard(&mut self, read: Read, fields: &mut Fields) {
        let mut before = Fields::default();
        self.fill(&mut before);
        self.time = read.time;
        self.language = read.language;
        self.about = read.about;
        self.startup = read.startup;
        let mut after = Fields::default();
        self.fill(&mut after);
        for name in field_names() {
            if fields.get(name) == before.get(name) && before.get(name) != after.get(name) {
                fields.set(name, after.get(name));
            }
        }
    }

    /// Puts one field back to what is set.
    fn restore(&self, name: &str, fields: &mut Fields) {
        let mut set = Fields::default();
        self.fill(&mut set);
        fields.set(name, set.get(name));
    }

    /// Reads everything again after a change, giving the daemons that act
    /// on it a moment to have.
    fn changed(&mut self, fields: &mut Fields) {
        // timed and netd act on a change within a second; give the window
        // what they have made of it.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let read = Read::now(&self.ident);
        self.heard(read, fields);
    }

    fn nav(&self) -> Vec<Nav> {
        let section = |view: View, title, now: String, glyph, tile| Nav::Section(Section { id: view.id(), title, now, glyph, tile });
        vec![
            section(View::Time, "Time & Date", time::now(&self.time), Glyph::Clock, Tile::Blue),
            section(View::Language, "Language & Keyboard", language::now(&self.language, &self.installed), Glyph::Globe, Tile::Violet),
            section(View::Startup, "Startup & Shutdown", startup::now(&self.startup), Glyph::Power, Tile::Orange),
            section(View::About, "About", about::now(&self.about), Glyph::Info, Tile::Slate),
        ]
    }

    /// Whether anything on the section shown may be changed here.
    fn may_change_shown(&self) -> bool {
        match self.view {
            View::Time => self.time.may.is_ok(),
            View::Language => self.language.may_locale.is_ok() || self.language.may_keymap.is_ok(),
            View::Startup => self.startup.may_boot.is_ok() || self.startup.may_login.is_ok() || self.startup.may_cmdline.is_ok(),
            View::About => self.about.may_name.is_ok(),
        }
    }
}

/// Every field the window has.
fn field_names() -> Vec<&'static str> {
    let mut names = vec![
        "automatic", "zone", "servers", "unauthenticated", "dhcp", "min-poll", "max-poll", "lang", "formats", "keymap", "hostname",
        "autologon", "bootattempts", "quiet", "safemode",
    ];
    names.extend(startup::TIMEOUTS.iter().map(|(name, ..)| *name));
    names
}

impl Live for Settings {
    fn render(&self, facts: &Facts) -> String {
        let fields = facts.fields;
        let page = match self.view {
            View::Time => time::render(&self.time, &self.zones, fields, self.now, self.editing_servers),
            View::Language => language::render(&self.language, &self.installed, fields),
            View::Startup => startup::render(&self.startup, fields),
            View::About => about::render(&self.about, fields),
        };
        let aside = if self.may_change_shown() { "Choices apply at once" } else { "" };
        settings::window(&self.nav(), self.view.id(), &page, &settings::status(self.said.as_ref(), aside))
    }

    fn input(&mut self, name: &str, fields: &mut Fields) {
        let applied = time::apply(name, fields)
            .or_else(|| language::apply(name, &self.language, &self.installed, fields))
            .or_else(|| startup::apply(name, &self.startup, fields));
        let Some(done) = applied else { return };
        match done {
            Ok(_) => self.changed(fields),
            // What couldn't be done is shown as it still is.
            Err(_) => self.restore(name, fields),
        }
        self.said = Some(done);
    }

    fn event(&mut self, name: &str, value: &Value, fields: &mut Fields) {
        let done = match name {
            "section" => {
                if let Some(view) = value.get("section").and_then(Value::as_str).and_then(View::by) {
                    self.view = view;
                    self.said = None;
                    self.editing_servers = false;
                }
                return;
            }
            "edit-servers" => {
                self.editing_servers = true;
                return;
            }
            "cancel-servers" => {
                self.editing_servers = false;
                self.restore("servers", fields);
                return;
            }
            "undo-timeouts" => {
                for (name, ..) in startup::TIMEOUTS {
                    self.restore(name, fields);
                }
                return;
            }
            "undo-cmdline" => {
                self.restore("bootattempts", fields);
                self.restore("quiet", fields);
                self.restore("safemode", fields);
                return;
            }
            "set-time" => time::set_clock(&self.time, fields),
            "save-servers" => time::save_servers(fields),
            "save-name" => about::save_name(fields),
            "save-timeouts" => startup::save_timeouts(&self.startup, fields),
            "save-cmdline" => startup::save_cmdline(&self.startup, fields),
            _ => return,
        };
        let succeeded = done.is_ok();
        self.said = Some(done);
        if succeeded {
            if name == "save-servers" {
                self.editing_servers = false;
            }
            if name == "set-time" {
                fields.set("date", "");
                fields.set("clock", "");
            }
            self.changed(fields);
            // What was just applied is what is set now.
            match name {
                "save-name" => self.restore("hostname", fields),
                "save-servers" => self.restore("servers", fields),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_section_names_a_view() {
        for (view, by) in View::ALL {
            assert_eq!(View::by(by), Some(view));
            assert_eq!(view.id(), by);
        }
        assert_eq!(View::by("Nonsense"), None);
    }
}
