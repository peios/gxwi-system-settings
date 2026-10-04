//! Startup & shutdown: how this boot went, how long peinit waits at each
//! stage, signing in at the console without a password, and the kernel's
//! command line.
//!
//! How this boot went is asked of peinit. The timeouts are
//! `Machine\System\Boot`, which peinit reads at boot (and, for shutdown, on
//! `reload-config`). Signing in at the console is the `login-console`
//! service's arguments. The command line is the one the kernel was started
//! with, and the one in `/lcl/etc/boot/cmdline` that the next boot image is
//! made from.

use libauthd::ident::{Fields as Asked, Kind};
use libauthd_client::ident::Ident;
use libgxwi::{Fields, escape};
use peinit::client::{Boot, ControlClient};
use peios::registry::Data;

use crate::reg;

pub const BOOT_KEY: &str = "Machine\\System\\Boot";
pub const LOGIN_KEY: &str = "Machine\\System\\Services\\login-console";
const NEXT_CMDLINE: &str = "/lcl/etc/boot/cmdline";
/// The service that rebuilds the boot image when the command line changes;
/// installed with the dynamic-boot feature.
const REBUILDER_KEY: &str = "Machine\\System\\Services\\mkuki-watch";

/// The boot timeouts: value, what it is, its default, its unit, when a
/// change applies.
pub const TIMEOUTS: [(&str, &str, u32, &str, &str); 5] = [
    ("ShutdownTimeout", "Waiting for services to stop at shutdown", 90, "seconds", "Applies the next time peinit re-reads its configuration."),
    ("PostKillTimeout", "Waiting after ending what didn't stop", 5, "seconds", "Applies at the next boot."),
    ("BootSuccessGrace", "Running well before a boot counts as good", 30, "seconds", "Applies at the next boot."),
    ("SettleTimeout", "Waiting for devices to settle at boot", 5, "seconds", "Applies at the next boot."),
    ("MaxParallelStarts", "Services started at once", 10, "at most", "Applies at the next boot."),
];

#[derive(Debug, Clone, PartialEq)]
pub struct Startup {
    pub boot: Result<Boot, String>,
    pub timeouts: Vec<Option<u32>>,
    pub may_boot: Result<(), String>,
    /// The account the console signs in without a password, if any, or
    /// `Err` where the console has no sign-in service.
    pub autologon: Result<Option<String>, String>,
    pub may_login: Result<(), String>,
    /// Local accounts, by name, to choose from.
    pub accounts: Vec<String>,
    pub running: String,
    pub next: Option<String>,
    pub rebuilds: bool,
    /// Whether this person may change the next boot's command line: asked
    /// by opening the file for writing.
    pub may_cmdline: Result<(), String>,
}

/// The Peios flags System Settings offers as switches: the boot-attempt
/// threshold (3 unless the line says) and how much peinit writes to the
/// console (1 unless it says).
pub const ATTEMPTS: &str = "peios.bootattempts";
pub const QUIET: &str = "peios.quiet";
const DEFAULT_ATTEMPTS: u32 = 3;
const DEFAULT_QUIET: u32 = 1;

/// A flag's value on a command line, if it is there and a number.
pub fn flag(line: &str, name: &str) -> Option<u32> {
    line.split_whitespace().find_map(|token| token.strip_prefix(name)?.strip_prefix('=')?.parse().ok())
}

/// The line with the two flags as given: each taken out wherever it was,
/// and put back at the end only where it isn't the default, so a line
/// left as it was is not rewritten for nothing.
pub fn with_flags(line: &str, attempts: u32, quiet: u32) -> String {
    let mut tokens: Vec<String> = line
        .split_whitespace()
        .filter(|token| ![ATTEMPTS, QUIET].iter().any(|name| token.strip_prefix(name).is_some_and(|rest| rest.starts_with('='))))
        .map(str::to_string)
        .collect();
    if attempts != DEFAULT_ATTEMPTS {
        tokens.push(format!("{ATTEMPTS}={attempts}"));
    }
    if quiet != DEFAULT_QUIET {
        tokens.push(format!("{QUIET}={quiet}"));
    }
    tokens.join(" ")
}

pub fn read(ident: &Ident) -> Startup {
    let arguments = reg::list(LOGIN_KEY, "Arguments");
    let autologon = if reg::exists(LOGIN_KEY) {
        Ok(arguments.as_deref().and_then(autologon_of))
    } else {
        Err("This machine has no sign-in at its console.".to_string())
    };
    Startup {
        boot: boot(),
        timeouts: TIMEOUTS.iter().map(|(name, ..)| reg::number(BOOT_KEY, name)).collect(),
        may_boot: reg::may_change(BOOT_KEY, "how the machine starts"),
        autologon,
        may_login: reg::may_change(LOGIN_KEY, "signing in at the console"),
        accounts: accounts(ident),
        running: std::fs::read_to_string("/proc/cmdline").unwrap_or_default().trim().to_string(),
        next: std::fs::read_to_string(NEXT_CMDLINE).ok().map(|t| t.trim().to_string()),
        rebuilds: reg::exists(REBUILDER_KEY) && reg::number(REBUILDER_KEY, "Disabled").is_none_or(|d| d == 0),
        may_cmdline: std::fs::OpenOptions::new().write(true).open(NEXT_CMDLINE).map(|_| ()).map_err(|e| match e.kind() {
            std::io::ErrorKind::PermissionDenied => format!(
                "You may look, but changing the next boot's command line needs write access to {NEXT_CMDLINE}, which as shipped only Administrators have."
            ),
            std::io::ErrorKind::NotFound => "This machine has no command line to change: it boots from its install image.".to_string(),
            _ => format!("The next boot's command line can't be changed: {e}."),
        }),
    }
}

fn boot() -> Result<Boot, String> {
    let mut client = ControlClient::connect_default().map_err(|e| format!("peinit can't be reached: {e}."))?;
    client.boot().map_err(|e| {
        let said = e.to_string();
        if said.contains("ACCESS_DENIED") {
            "How this boot went is peinit's to say, and its control descriptor (Machine\\System\\Init ControlSecurity) doesn't let you ask.".to_string()
        } else {
            format!("peinit didn't say how this boot went: {said}.")
        }
    })
}

/// The account `--try-no-password NAME` names in login's arguments.
fn autologon_of(arguments: &[String]) -> Option<String> {
    let at = arguments.iter().position(|a| a == "--try-no-password")?;
    arguments.get(at + 1).cloned()
}

fn accounts(ident: &Ident) -> Vec<String> {
    let Ok(listing) = ident.enumerate(Kind::Principal, Asked::empty(), None) else {
        return Vec::new();
    };
    let mut names: Vec<String> = listing
        .records
        .into_iter()
        .filter(|record| peios::security::SidRef::from_bytes(&record.sid).is_some_and(|sid| sid.to_string().starts_with("S-1-5-21-")))
        .map(|record| record.qualified_name.rsplit('\\').next().unwrap_or_default().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names
}

pub fn fill(startup: &Startup, fields: &mut Fields) {
    for ((name, _, default, ..), value) in TIMEOUTS.iter().zip(&startup.timeouts) {
        fields.set(name, &value.unwrap_or(*default).to_string());
    }
    fields.set("autologon", startup.autologon.as_ref().ok().cloned().flatten().as_deref().unwrap_or(""));
    let next = startup.next.as_deref().unwrap_or_default();
    fields.set("bootattempts", &flag(next, ATTEMPTS).unwrap_or(DEFAULT_ATTEMPTS).to_string());
    fields.set("quiet", &flag(next, QUIET).unwrap_or(DEFAULT_QUIET).to_string());
}

/// Writes the two flags into the next boot's command line, for the boot
/// image to be made again from it.
pub fn save_cmdline(startup: &Startup, fields: &Fields) -> Result<String, String> {
    let next = startup.next.as_deref().ok_or("This machine has no command line to change.")?;
    let number = |name: &str, most: u32| fields.get(name).parse::<u32>().ok().filter(|n| *n <= most).ok_or_else(|| "Choose from the list.".to_string());
    let line = with_flags(next, number("bootattempts", 10)?, number("quiet", 2)?);
    // Beside it, then over it: mkuki-watch makes the boot image again as
    // soon as the file changes, and must never find it half-written.
    let staging = format!("{NEXT_CMDLINE}.new");
    std::fs::write(&staging, format!("{line}\n"))
        .and_then(|()| std::fs::rename(&staging, NEXT_CMDLINE))
        .map_err(|e| {
            let _ = std::fs::remove_file(&staging);
            format!("The command line couldn't be written: {e}.")
        })?;
    Ok("Saved. It applies at the next boot.".into())
}

pub fn render(startup: &Startup, fields: &Fields) -> String {
    let boot = match &startup.boot {
        Err(why) => format!("<p class=\"note bad\">{}</p>", escape(why)),
        Ok(boot) => {
            let mut facts = format!("<dt>Started in</dt><dd>{}</dd>", escape(&crate::words::boot_mode(boot)));
            facts.push_str(&format!("<dt>Health</dt><dd>{}</dd>", escape(&crate::words::boot_health(boot))));
            format!("<dl class=\"facts\">{facts}</dl>")
        }
    };
    let boot_card = format!("<section class=\"card\" aria-label=\"This boot\"><h2>This boot</h2>{boot}</section>");

    let may = startup.may_boot.is_ok();
    let off = if may { "" } else { " disabled" };
    let mut rows = String::new();
    let mut changed = false;
    for ((name, label, default, unit, applies), value) in TIMEOUTS.iter().zip(&startup.timeouts) {
        let typed = fields.get(name);
        changed |= typed.trim() != value.unwrap_or(*default).to_string();
        rows.push_str(&format!(
            "<label>{label}<span class=\"amount\"><input name=\"{name}\" inputmode=\"numeric\" autocomplete=\"off\"{off}> {unit}</span></label>\
             <p class=\"hint\">{applies} Usually {default}.</p>",
            label = escape(label),
            unit = escape(unit),
            applies = escape(applies),
        ));
    }
    let timeouts_card = format!(
        "<section class=\"card\" aria-label=\"Timeouts\"><h2>Timeouts</h2>\
         <form class=\"edit\" fx-submit=\"save-timeouts\">{rows}{save}</form>{why}</section>",
        save = save(may, changed),
        why = why(&startup.may_boot),
    );

    let login_card = match &startup.autologon {
        Err(why) => format!("<section class=\"card\" aria-label=\"Signing in at the console\"><h2>Signing in at the console</h2><p class=\"note\">{}</p></section>", escape(why)),
        Ok(current) => {
            let may = startup.may_login.is_ok();
            let chosen = fields.get("autologon");
            let mut options = format!("<option value=\"\"{}>Nobody: always ask</option>", if chosen.is_empty() { " selected" } else { "" });
            let mut names = startup.accounts.clone();
            if let Some(current) = current
                && !names.contains(current)
            {
                names.push(current.clone());
            }
            for name in &names {
                options.push_str(&format!("<option value=\"{n}\"{s}>{n}</option>", n = escape(name), s = if chosen == name { " selected" } else { "" }));
            }
            format!(
                "<section class=\"card\" aria-label=\"Signing in at the console\"><h2>Signing in at the console</h2>\
                 <form class=\"edit\" fx-submit=\"save-autologon\">\
                 <label>Sign in without asking, as<select name=\"autologon\"{off}>{options}</select></label>\
                 <p class=\"hint\">At this machine's own screen and keyboard. It works only for an account that signs in without a password; anyone else is asked as usual. It applies the next time the console's sign-in starts, at the latest the next boot.</p>\
                 {save}</form>{why}</section>",
                off = if may { "" } else { " disabled" },
                save = save(may, chosen != current.as_deref().unwrap_or("")),
                why = why(&startup.may_login),
            )
        }
    };

    let next = match &startup.next {
        Some(next) if *next != startup.running => format!(
            "<dt>Next boot</dt><dd><code>{}</code></dd>",
            escape(next)
        ),
        _ => String::new(),
    };
    let switches = match (&startup.next, startup.rebuilds, &startup.may_cmdline) {
        (Some(_), true, Ok(())) => {
            let chosen = |name: &str| fields.get(name).to_string();
            let attempts: String = (0..=10)
                .map(|n| {
                    let label = if n == 0 { "Never: always try a full boot".to_string() } else { format!("After {n} that never counted as good") };
                    format!("<option value=\"{n}\"{s}>{l}</option>", s = if chosen("bootattempts") == n.to_string() { " selected" } else { "" }, l = escape(&label))
                })
                .collect();
            let quiet: String = [(0, "Everything"), (1, "Not over a sign-in prompt"), (2, "Only errors, and not over a sign-in prompt")]
                .iter()
                .map(|(n, label)| format!("<option value=\"{n}\"{s}>{label}</option>", s = if chosen("quiet") == n.to_string() { " selected" } else { "" }))
                .collect();
            let next = startup.next.as_deref().unwrap_or_default();
            let changed = chosen("bootattempts") != flag(next, ATTEMPTS).unwrap_or(DEFAULT_ATTEMPTS).to_string()
                || chosen("quiet") != flag(next, QUIET).unwrap_or(DEFAULT_QUIET).to_string();
            format!(
                "<form class=\"edit\" fx-submit=\"save-cmdline\">\
                 <label>Start in recovery<select name=\"bootattempts\">{attempts}</select></label>\
                 <p class=\"hint\">How many boots in a row may fail to count as good before the machine starts in recovery, a shell with no services.</p>\
                 <label>What peinit writes on the console<select name=\"quiet\">{quiet}</select></label>\
                 <p class=\"hint\">Applies at the next boot, once the boot image has been made again with the new line.</p>{save}</form>",
                save = save(true, changed),
            )
        }
        (Some(_), true, Err(why)) => format!("<p class=\"why\">{}</p>", escape(why)),
        _ => String::new(),
    };
    let how = if startup.rebuilds {
        "The boot image is made again when the command line changes, so a change applies at the next boot."
    } else {
        "The command line is part of the boot image, which is only made when Peios is installed or upgraded, so it is shown here and not changed."
    };
    let cmdline_card = format!(
        "<section class=\"card\" aria-label=\"Kernel command line\"><h2>Kernel command line</h2>\
         <dl class=\"facts\"><dt>This boot</dt><dd><code>{running}</code></dd>{next}</dl>\
         <p class=\"hint\">{how}</p>{switches}</section>",
        running = escape(&startup.running),
    );

    format!("{boot_card}{timeouts_card}{login_card}{cmdline_card}")
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

pub fn save_timeouts(startup: &Startup, fields: &Fields) -> Result<String, String> {
    let mut values = Vec::new();
    for ((name, label, default, ..), current) in TIMEOUTS.iter().zip(&startup.timeouts) {
        let typed = fields.get(name).trim();
        let n: u32 = typed.parse().map_err(|_| format!("{label}: give a whole number."))?;
        if n == 0 {
            return Err(format!("{label}: give a number above 0."));
        }
        if n == current.unwrap_or(*default) {
            continue;
        }
        values.push((*name, Some(Data::Dword(n))));
    }
    if values.is_empty() {
        return Ok("Nothing has changed.".into());
    }
    reg::set(BOOT_KEY, "how the machine starts", &values)?;
    Ok("Saved.".into())
}

pub fn save_autologon(startup: &Startup, fields: &Fields) -> Result<String, String> {
    let chosen = fields.get("autologon").trim();
    if !chosen.is_empty() && !startup.accounts.iter().any(|a| a == chosen) {
        return Err(format!("There is no local account called {chosen}."));
    }
    let mut arguments = vec!["--console".to_string()];
    if !chosen.is_empty() {
        arguments.push("--try-no-password".into());
        arguments.push(chosen.to_string());
    }
    reg::set(LOGIN_KEY, "signing in at the console", &[("Arguments", Some(Data::MultiSz(arguments)))])?;
    Ok(if chosen.is_empty() {
        "The console will ask who is signing in.".into()
    } else {
        format!("The console will sign {chosen} in without asking, if {chosen} signs in without a password.")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_account_signed_in_without_asking_is_read_from_logins_arguments() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(autologon_of(&args(&["--console", "--try-no-password", "peios"])), Some("peios".into()));
        assert_eq!(autologon_of(&args(&["--console"])), None);
        assert_eq!(autologon_of(&args(&["--console", "--try-no-password"])), None);
    }

    #[test]
    fn the_flags_are_read_and_written_and_nothing_else_moves() {
        let line = "loglevel=4 init=/bin/peinit2 peios.quiet=2 root=UUID=abc";
        assert_eq!(flag(line, QUIET), Some(2));
        assert_eq!(flag(line, ATTEMPTS), None);
        // Defaults leave no token; others go at the end.
        assert_eq!(with_flags(line, 3, 1), "loglevel=4 init=/bin/peinit2 root=UUID=abc");
        assert_eq!(with_flags(line, 0, 2), "loglevel=4 init=/bin/peinit2 root=UUID=abc peios.bootattempts=0 peios.quiet=2");
        // A token that merely starts with the name is not the flag.
        assert_eq!(with_flags("peios.quietly=1", 3, 1), "peios.quietly=1");
        assert_eq!(flag("peios.quietly=1", QUIET), None);
    }
}
