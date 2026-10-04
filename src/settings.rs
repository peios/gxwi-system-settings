//! The window: four parts of the machine's settings, one shown at a time.
//!
//! Each part reads what is set, and asks the system whether this person may
//! change it. What they may not change is shown as it is, with the reason;
//! what they may is a form, saved with its own button. A form holds what is
//! typed until it is saved, while what it shows is read again in the
//! background, so a value changed elsewhere appears unless it is being
//! edited here.

use std::sync::Weak;

use libauthd_client::ident::Ident;
use libgxwi::{Facts, Fields, Live, Surface, Value, escape};
use libtimed::zone::Listed;

use crate::about::{self, About};
use crate::language::{self, Installed, Language};
use crate::startup::{self, Startup};
use crate::time::{self, Time};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Time,
    Language,
    About,
    Startup,
}

impl View {
    const ALL: [(View, &'static str, &'static str); 4] = [
        (View::Time, "Time", "Time & date"),
        (View::Language, "Language", "Language & keyboard"),
        (View::About, "About", "About"),
        (View::Startup, "Startup", "Startup & shutdown"),
    ];

    fn by(name: &str) -> Option<View> {
        View::ALL.iter().find(|(_, by, _)| *by == name).map(|(view, ..)| *view)
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
            said: None,
            now: jiff::Timestamp::now(),
        }
    }

    pub fn ident(&self) -> &Ident {
        &self.ident
    }

    /// Whether the clock shown would read differently at `now`: only the
    /// time tab shows it, to the minute.
    pub fn minute_turned(&self, now: jiff::Timestamp) -> bool {
        self.view == View::Time && self.now.as_second() / 60 != now.as_second() / 60
    }

    /// Whether what was read differs from what is shown.
    pub fn differs(&self, read: &Read) -> bool {
        read.time != self.time || read.language != self.language || read.about != self.about || read.startup != self.startup
    }

    /// Puts what is set in every form.
    pub fn fill(&self, fields: &mut Fields) {
        time::fill(&self.time.policy, fields);
        language::fill(&self.language, fields);
        about::fill(&self.about, fields);
        startup::fill(&self.startup, fields);
    }

    /// What was read again in the background. A field still holding what
    /// was set before follows what is set now; one being edited is left.
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

    /// Reads everything again now, and puts it in the forms.
    fn reread(&mut self, fields: &mut Fields) {
        let read = Read::now(&self.ident);
        self.time = read.time;
        self.language = read.language;
        self.about = read.about;
        self.startup = read.startup;
        self.fill(fields);
    }
}

/// Every field a form has.
fn field_names() -> Vec<&'static str> {
    let mut names = vec![
        "zone", "servers", "unauthenticated", "dhcp", "min-poll", "max-poll", "lang", "formats", "keymap", "hostname",
        "autologon",
    ];
    names.extend(startup::TIMEOUTS.iter().map(|(name, ..)| *name));
    names
}

impl Live for Settings {
    fn render(&self, facts: &Facts) -> String {
        let fields = facts.fields;
        let tabs: String = View::ALL
            .iter()
            .map(|(view, by, label)| {
                format!(
                    "<button class=\"tab\" fx-click=\"view\" fx-value-by=\"{by}\" aria-pressed=\"{}\">{label}</button>",
                    *view == self.view
                )
            })
            .collect();
        let body = match self.view {
            View::Time => time::render(&self.time, &self.zones, fields, self.now),
            View::Language => language::render(&self.language, &self.installed, fields),
            View::About => about::render(&self.about, fields),
            View::Startup => startup::render(&self.startup, fields),
        };
        let said = match &self.said {
            None => String::new(),
            Some(Ok(done)) => format!("<p class=\"said\" role=\"status\">{}</p>", escape(done)),
            Some(Err(why)) => format!("<p class=\"said bad\" role=\"alert\">{}</p>", escape(why)),
        };
        format!(
            "<nav class=\"bar\" aria-label=\"Settings\"><div class=\"tabs\">{tabs}</div></nav>\
             {said}<div class=\"body\"><div class=\"cards\">{body}</div></div>"
        )
    }

    fn event(&mut self, name: &str, value: &Value, fields: &mut Fields) {
        let done = match name {
            "view" => {
                if let Some(view) = value.get("by").and_then(Value::as_str).and_then(View::by) {
                    self.view = view;
                    self.said = None;
                }
                return;
            }
            "save-zone" => time::save_zone(fields),
            "by-hand" => time::set_automatic(false),
            "automatic" => time::set_automatic(true),
            "set-time" => time::set_clock(&self.time, fields),
            "save-servers" => time::save_servers(fields),
            "save-language" => language::save_language(fields),
            "save-keymap" => language::save_keymap(&self.language, &self.installed, fields),
            "save-name" => about::save_name(fields),
            "save-timeouts" => startup::save_timeouts(&self.startup, fields),
            "save-autologon" => startup::save_autologon(&self.startup, fields),
            _ => return,
        };
        let succeeded = done.is_ok();
        self.said = Some(done);
        if succeeded {
            // timed and netd act on a change within a second; give the
            // window what they have made of it.
            std::thread::sleep(std::time::Duration::from_millis(300));
            self.reread(fields);
            if name == "set-time" {
                fields.set("date", "");
                fields.set("clock", "");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_tab_names_a_view() {
        for (view, by, _) in View::ALL {
            assert_eq!(View::by(by), Some(view));
        }
        assert_eq!(View::by("Nonsense"), None);
    }
}
