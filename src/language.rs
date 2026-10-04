//! Language & keyboard: the machine's language and formats, which every
//! session starts in unless the person has chosen their own, and the
//! console's keyboard layout.
//!
//! The language is `Machine\System\Locale`, read by each session as it
//! starts. The layout is `Machine\System\Console Keymap`, loaded by the
//! console-keymap service, which is restarted to load a new one.

use libgxwi::{Fields, escape};
use libsession::locale::{self, Available};
use peinit::client::{Command, ControlClient, ServiceAccess};
use peios::registry::Data;

use crate::reg;

pub const LOCALE_KEY: &str = locale::MACHINE_KEY;
pub const CONSOLE_KEY: &str = "Machine\\System\\Console";
pub const KEYMAP_VALUE: &str = "Keymap";
pub const KEYMAP_SERVICE: &str = "console-keymap";
const KEYMAPS: &str = "/usr/share/kbd/keymaps/i386";

/// The categories that are formats rather than language: how dates,
/// numbers, money, measures and paper are written.
pub const FORMATS: [&str; 5] = ["LC_TIME", "LC_NUMERIC", "LC_MONETARY", "LC_MEASUREMENT", "LC_PAPER"];

#[derive(Debug, Clone, PartialEq)]
pub struct Language {
    pub lang: Option<String>,
    /// The formats' locale, where every format category names the same
    /// one; `Some("")` where they differ.
    pub formats: Option<String>,
    pub keymap: Option<String>,
    pub may_locale: Result<(), String>,
    pub may_keymap: Result<(), String>,
    /// Whether the layout can be loaded now: the service is there, and
    /// this person may restart it.
    pub loader: Result<(), String>,
}

/// What is installed: read once, since installing more needs a package.
pub struct Installed {
    pub locales: Vec<Available>,
    /// Keymaps by family (`qwerty`, `azerty`, …), each sorted.
    pub keymaps: Vec<(String, Vec<String>)>,
}

pub fn installed() -> Installed {
    Installed {
        locales: locale::available(),
        keymaps: keymaps(),
    }
}

fn keymaps() -> Vec<(String, Vec<String>)> {
    let Ok(families) = std::fs::read_dir(KEYMAPS) else {
        return Vec::new();
    };
    let mut found: Vec<(String, Vec<String>)> = families
        .flatten()
        .filter_map(|family| {
            let name = family.file_name().to_str()?.to_string();
            if name == "include" || !family.path().is_dir() {
                return None;
            }
            let mut maps: Vec<String> = std::fs::read_dir(family.path())
                .ok()?
                .flatten()
                .filter_map(|map| {
                    let file = map.file_name().to_str()?.to_string();
                    let stem = file.strip_suffix(".map.gz").or_else(|| file.strip_suffix(".map"))?;
                    Some(stem.to_string())
                })
                .collect();
            maps.sort();
            maps.dedup();
            (!maps.is_empty()).then_some((name, maps))
        })
        .collect();
    found.sort();
    found
}

pub fn read() -> Language {
    let formats: Vec<Option<String>> = FORMATS.iter().map(|c| reg::text(LOCALE_KEY, c)).collect();
    let formats = match formats.first().cloned().flatten() {
        None if formats.iter().all(Option::is_none) => None,
        Some(first) if formats.iter().all(|f| f.as_deref() == Some(first.as_str())) => Some(first),
        _ => Some(String::new()),
    };
    Language {
        lang: reg::text(LOCALE_KEY, "LANG"),
        formats,
        keymap: reg::text(CONSOLE_KEY, KEYMAP_VALUE).filter(|k| !k.is_empty()),
        may_locale: reg::may_change(LOCALE_KEY, "the machine's language"),
        may_keymap: reg::may_change(CONSOLE_KEY, "the console's keyboard layout"),
        loader: loader(),
    }
}

/// Whether the keymap service is there and this person may restart it,
/// asked of peinit, which says what rights the caller holds on it.
fn loader() -> Result<(), String> {
    let mut client = ControlClient::connect_default().map_err(|e| format!("peinit can't be reached: {e}."))?;
    let status = client.status_of(KEYMAP_SERVICE).map_err(|_| {
        "This machine has no console-keymap service to load a layout, so one chosen here applies once the service is installed.".to_string()
    })?;
    let needs = ServiceAccess::for_command(Command::Restart);
    let held = needs.wire_names().iter().all(|right| status.granted.iter().any(|g| g == right));
    if held {
        Ok(())
    } else {
        Err("You may choose the layout, but loading it needs the right to restart the console-keymap service, so it applies at the next boot.".into())
    }
}

pub fn fill(language: &Language, fields: &mut Fields) {
    fields.set("lang", language.lang.as_deref().unwrap_or(locale::DEFAULT));
    fields.set("formats", language.formats.as_deref().unwrap_or(""));
    fields.set("keymap", language.keymap.as_deref().unwrap_or(""));
}

fn describe(available: &Available) -> String {
    match (&available.language, &available.territory) {
        (Some(language), Some(territory)) => format!("{language} ({territory}) — {}", available.name),
        (Some(language), None) => format!("{language} — {}", available.name),
        _ if available.name == locale::DEFAULT => format!("Plain English, international conventions — {}", available.name),
        _ => available.name.clone(),
    }
}

pub fn render(language: &Language, installed: &Installed, fields: &Fields) -> String {
    let may_locale = language.may_locale.is_ok();
    let off = |may: bool| if may { "" } else { " disabled" };

    let options = |chosen: &str, first: Option<&str>| {
        let mut html = String::new();
        if let Some(first) = first {
            html.push_str(&format!("<option value=\"\"{}>{}</option>", if chosen.is_empty() { " selected" } else { "" }, escape(first)));
        }
        for available in &installed.locales {
            html.push_str(&format!(
                "<option value=\"{v}\"{s}>{l}</option>",
                v = escape(&available.name),
                s = if chosen == available.name { " selected" } else { "" },
                l = escape(&describe(available))
            ));
        }
        if !chosen.is_empty() && !installed.locales.iter().any(|a| a.name == chosen) {
            html.push_str(&format!("<option value=\"{v}\" selected>{v} — not installed</option>", v = escape(chosen)));
        }
        html
    };
    let mixed = language.formats.as_deref() == Some("");
    let formats_chosen = fields.get("formats");
    let changed = fields.get("lang") != language.lang.as_deref().unwrap_or(locale::DEFAULT)
        || formats_chosen != language.formats.as_deref().unwrap_or("");
    let mixed_note = if mixed {
        "<p class=\"note\">The formats are set one by one, to different languages. Choosing here sets them all.</p>"
    } else {
        ""
    };
    let language_card = format!(
        "<section class=\"card\" aria-label=\"Language and formats\"><h2>Language and formats</h2>\
         <form class=\"edit\" fx-submit=\"save-language\">\
         <label>Language<select name=\"lang\"{d}>{langs}</select></label>\
         <label>Formats<select name=\"formats\"{d}>{formats}</select></label>{mixed_note}\
         <p class=\"hint\">Formats are how dates, numbers, money, measures and paper sizes are written. Each person may choose their own in My Settings; this is for everyone who hasn't. It applies to sessions that start after this.</p>\
         <p class=\"hint\">Another language is added by installing its language pack: <code>peipkg install org.gnu.glibc-langpack-</code> and the language's code, such as <code>de</code>.</p>\
         {save}</form>{why}</section>",
        d = off(may_locale),
        langs = options(fields.get("lang"), None),
        formats = options(if mixed && formats_chosen.is_empty() { "" } else { formats_chosen }, Some("The same as the language")),
        save = save(may_locale, changed),
        why = why(&language.may_locale),
    );

    let may_keymap = language.may_keymap.is_ok();
    let chosen = fields.get("keymap");
    let mut maps = format!("<option value=\"\"{}>US — built in</option>", if chosen.is_empty() { " selected" } else { "" });
    for (family, names) in &installed.keymaps {
        maps.push_str(&format!("<optgroup label=\"{}\">", escape(&family_name(family))));
        for name in names {
            maps.push_str(&format!(
                "<option value=\"{v}\"{s}>{v}</option>",
                v = escape(name),
                s = if chosen == name { " selected" } else { "" }
            ));
        }
        maps.push_str("</optgroup>");
    }
    let keymap_changed = chosen != language.keymap.as_deref().unwrap_or("");
    let loader = match &language.loader {
        Ok(()) => String::new(),
        Err(why) => format!("<p class=\"note\">{}</p>", escape(why)),
    };
    let keyboard_card = format!(
        "<section class=\"card\" aria-label=\"Console keyboard\"><h2>Console keyboard</h2>\
         <form class=\"edit\" fx-submit=\"save-keymap\">\
         <label>Layout<select name=\"keymap\"{d}>{maps}</select></label>\
         <p class=\"hint\">The layout of the keyboard plugged into this machine, on its text consoles. The desktop's keyboard is the browser's, and a terminal over SSH uses the computer it's typed on.</p>\
         {loader}{save}</form>{why}</section>",
        d = off(may_keymap),
        save = save(may_keymap, keymap_changed),
        why = why(&language.may_keymap),
    );
    format!("{language_card}{keyboard_card}")
}

fn family_name(family: &str) -> String {
    match family {
        "qwerty" => "QWERTY".into(),
        "qwertz" => "QWERTZ".into(),
        "azerty" => "AZERTY".into(),
        "dvorak" => "Dvorak".into(),
        "colemak" => "Colemak".into(),
        "bepo" => "BÉPO".into(),
        "neo" => "Neo".into(),
        "olpc" => "OLPC".into(),
        other => other.to_string(),
    }
}

fn save(may: bool, changed: bool) -> String {
    if !may {
        return String::new();
    }
    format!("<div class=\"actions\"><button class=\"primary\"{}>Save</button></div>", if changed { "" } else { " disabled" })
}

fn why(may: &Result<(), String>) -> String {
    match may {
        Ok(()) => String::new(),
        Err(why) => format!("<p class=\"why\">{}</p>", escape(why)),
    }
}

pub fn save_language(fields: &Fields) -> Result<String, String> {
    let lang = fields.get("lang").trim();
    if !locale::usable("LANG", lang) {
        return Err(format!("{lang} isn't installed in full, so a session would ignore it."));
    }
    let formats = fields.get("formats").trim();
    if !formats.is_empty() {
        for category in FORMATS {
            if !locale::usable(category, formats) {
                return Err(format!("{formats} isn't installed, so a session would ignore it."));
            }
        }
    }
    let mut values = vec![("LANG", Some(Data::Sz(lang.to_string())))];
    for category in FORMATS {
        values.push((category, (!formats.is_empty()).then(|| Data::Sz(formats.to_string()))));
    }
    reg::set(LOCALE_KEY, "the machine's language", &values)?;
    Ok("Saved. Sessions that start from now on use it.".into())
}

/// Saves the layout, then has the console-keymap service load it.
pub fn save_keymap(language: &Language, installed: &Installed, fields: &Fields) -> Result<String, String> {
    let chosen = fields.get("keymap").trim();
    if !chosen.is_empty() && !installed.keymaps.iter().any(|(_, maps)| maps.iter().any(|m| m == chosen)) {
        return Err(format!("There is no layout called {chosen} on this machine."));
    }
    let value = (!chosen.is_empty()).then(|| Data::Sz(chosen.to_string()));
    reg::set(CONSOLE_KEY, "the console's keyboard layout", &[(KEYMAP_VALUE, value)])?;
    let name = if chosen.is_empty() { "US" } else { chosen };
    if language.loader.is_err() {
        return Ok(format!("Saved: the console's layout is {name} from the next boot."));
    }
    let mut client = ControlClient::connect_default().map_err(|e| format!("Saved, but peinit can't be reached to load it: {e}."))?;
    client
        .command(Command::Restart, KEYMAP_SERVICE)
        .map_err(|e| format!("Saved, but it couldn't be loaded now: {e}. It applies at the next boot."))?;
    Ok(format!("The console's layout is {name}."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locale_is_described_by_its_language_and_place() {
        let gb = Available { name: "en_GB.UTF-8".into(), language: Some("English".into()), territory: Some("United Kingdom".into()) };
        assert_eq!(describe(&gb), "English (United Kingdom) — en_GB.UTF-8");
        let c = Available { name: "C.UTF-8".into(), language: None, territory: None };
        assert!(describe(&c).starts_with("Plain English"));
    }

    #[test]
    fn a_layout_that_isnt_installed_is_refused_before_anything_is_written() {
        let language = Language {
            lang: None,
            formats: None,
            keymap: None,
            may_locale: Ok(()),
            may_keymap: Ok(()),
            loader: Ok(()),
        };
        let installed = Installed { locales: Vec::new(), keymaps: vec![("qwerty".into(), vec!["uk".into(), "us".into()])] };
        let mut fields = Fields::default();
        fields.set("keymap", "../../etc/x");
        assert!(save_keymap(&language, &installed, &fields).unwrap_err().contains("no layout"));
    }
}
