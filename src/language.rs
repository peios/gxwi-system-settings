//! Language & keyboard: the machine's language and formats, which every
//! session starts in unless the person has chosen their own, and the
//! console's keyboard layout.
//!
//! The language is `Machine\System\Locale`, read by each session as it
//! starts. The layout is `Machine\System\Console Keymap`, loaded by the
//! console-keymap service, which is restarted to load a new one.

use libgxwi::settings::{self, Glyph, Tile};
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
    pub loader: Result<(), NotNow>,
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

/// Why a chosen layout can't be loaded now.
#[derive(Debug, Clone, PartialEq)]
pub enum NotNow {
    /// The service is there, and loads it at the next boot.
    NextBoot(String),
    /// Nothing loads a layout on this machine.
    Never(String),
}

/// Whether the keymap service is there and this person may restart it,
/// asked of peinit, which says what rights the caller holds on it. A
/// service the person may not even query is left out of what peinit lists,
/// which is told apart from one that isn't defined by the registry.
fn loader() -> Result<(), NotNow> {
    let defined = reg::exists(&format!("Machine\\System\\Services\\{KEYMAP_SERVICE}"));
    if !defined {
        return Err(NotNow::Never(
            "This machine has no console-keymap service, so nothing loads a layout chosen here.".into(),
        ));
    }
    let next_boot = || {
        NotNow::NextBoot(
            "You may choose the layout, but loading it now needs the right to restart the console-keymap service, so it applies at the next boot.".into(),
        )
    };
    let mut client = ControlClient::connect_default().map_err(|_| next_boot())?;
    let status = client.status_of(KEYMAP_SERVICE).map_err(|_| next_boot())?;
    let needs = ServiceAccess::for_command(Command::Restart);
    if needs.wire_names().iter().all(|right| status.granted.iter().any(|g| g == right)) {
        Ok(())
    } else {
        Err(next_boot())
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

/// A language in a few words, for the side: "English (UK)".
fn short(lang: &str, installed: &Installed) -> String {
    match installed.locales.iter().find(|a| a.name == lang) {
        Some(Available { language: Some(language), territory: Some(territory), .. }) => {
            let place = match territory.as_str() {
                "United Kingdom" => "UK",
                "United States" => "US",
                other => other,
            };
            format!("{language} ({place})")
        }
        Some(Available { language: Some(language), .. }) => language.clone(),
        _ if lang == locale::DEFAULT => "Plain English".into(),
        _ => lang.to_string(),
    }
}

/// What the side says of this section.
pub fn now(language: &Language, installed: &Installed) -> String {
    let lang = short(language.lang.as_deref().unwrap_or(locale::DEFAULT), installed);
    let keymap = match language.keymap.as_deref() {
        None => "US keyboard".to_string(),
        Some(map) => format!("{} keyboard", map.to_uppercase()),
    };
    format!("{lang} · {keymap}")
}

pub fn render(language: &Language, installed: &Installed, fields: &Fields) -> String {
    let may_locale = language.may_locale.is_ok();
    let locales = |first: Option<&str>, chosen: &str| {
        let mut options: Vec<(String, String)> = first.map(|f| vec![(String::new(), f.to_string())]).unwrap_or_default();
        options.extend(installed.locales.iter().map(|a| (a.name.clone(), describe(a))));
        if !chosen.is_empty() && !installed.locales.iter().any(|a| a.name == chosen) {
            options.push((chosen.to_string(), format!("{chosen} — not installed")));
        }
        options
    };
    let mixed = language.formats.as_deref() == Some("");
    let mut foot = String::new();
    if mixed {
        foot.push_str(&settings::note("Formats are currently set individually, to different languages. Choosing one here sets them all."));
    }
    foot.push_str(&match &language.may_locale {
        Err(why) => settings::locked(why),
        Ok(()) => settings::hint(
            "To add a language, install its language pack, such as org.gnu.glibc-langpack-de for German.",
        ),
    });
    let rows = format!(
        "{}{}",
        settings::row(
            "Language",
            "The System Default for everyone who hasn't chosen their own. Applies to new sessions.",
            &settings::select("lang", "Language", &locales(None, fields.get("lang")), may_locale),
        ),
        settings::row(
            "Formats",
            "How dates, numbers, currency, measurements and paper sizes are written.",
            &settings::select("formats", "Formats", &locales(Some("Match Language"), fields.get("formats")), may_locale),
        ),
    );
    let language_group = settings::group("Language & Formats", &rows, &foot);

    let may_keymap = language.may_keymap.is_ok();
    let chosen = fields.get("keymap");
    let mut maps = format!(r#"<option value=""{}>US (System Default)</option>"#, if chosen.is_empty() { " selected" } else { "" });
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
    let layout = format!(
        r#"<select class="st-select" name="keymap" aria-label="Layout"{}>{maps}</select>"#,
        if may_keymap { "" } else { " disabled" }
    );
    let mut foot = String::new();
    if let Err(NotNow::NextBoot(why) | NotNow::Never(why)) = &language.loader {
        foot.push_str(&settings::note(why));
    }
    if let Err(why) = &language.may_keymap {
        foot.push_str(&settings::locked(why));
    }
    let keyboard_group = settings::group(
        "Console Keyboard",
        &settings::row(
            "Layout",
            "For keyboards attached to this machine, at its text consoles. The desktop and SSH use the layout of the device you type on.",
            &layout,
        ),
        &foot,
    );
    format!(
        "{}{language_group}{keyboard_group}",
        settings::head(Glyph::Globe, Tile::Violet, "Language & Keyboard", "The default language for all users, and the console keyboard.")
    )
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
    Ok("Saved. Applies to new sessions.".into())
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
    match &language.loader {
        Ok(()) => {}
        Err(NotNow::NextBoot(_)) => return Ok(format!("Console layout set to {name}. Applies at the next boot.")),
        Err(NotNow::Never(_)) => {
            return Ok(format!("Console layout set to {name}, but this machine has no console-keymap service to load it."));
        }
    }
    let mut client = ControlClient::connect_default().map_err(|e| format!("Saved, but peinit can't be reached to load it: {e}."))?;
    client
        .command(Command::Restart, KEYMAP_SERVICE)
        .map_err(|e| format!("Saved, but it couldn't be loaded now: {e}. It applies at the next boot."))?;
    Ok(format!("Console layout set to {name}."))
}

/// Applies a field that applies as it changes.
pub fn apply(name: &str, language: &Language, installed: &Installed, fields: &Fields) -> Option<Result<String, String>> {
    Some(match name {
        "lang" | "formats" => save_language(fields),
        "keymap" => save_keymap(language, installed, fields),
        _ => return None,
    })
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
        let installed = Installed { locales: vec![gb], keymaps: Vec::new() };
        assert_eq!(short("en_GB.UTF-8", &installed), "English (UK)");
        assert_eq!(short("C.UTF-8", &installed), "Plain English");
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
