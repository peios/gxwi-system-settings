//! What the window says, in words.

use libtimed::{SourceState, Status, Sync};

/// How well the clock is being kept, in a sentence.
pub fn keeping(status: &Status) -> String {
    if status.manual {
        return "Set by hand. Nothing checks it against a time server.".into();
    }
    match status.sync {
        Sync::Synchronised | Sync::Settling => {
            let from = status.system_peer.as_deref().unwrap_or("a time server");
            let within = plain(status.root_distance);
            if status.sync == Sync::Settling {
                format!("Kept from {from}, within {within}, and still settling.")
            } else {
                format!("Kept from {from}, within {within}.")
            }
        }
        Sync::Spike => "A time server says the clock is far out; timed is waiting to be sure before it moves it.".into(),
        Sync::Unsynchronised if status.sources == 0 => "Not kept: there is no time server to keep it from.".into(),
        Sync::Unsynchronised => format!(
            "Not kept yet: none of the {} time server{} can be relied on so far.",
            status.sources,
            if status.sources == 1 { "" } else { "s" }
        ),
    }
}

/// The line under the date: the zone, and how far it is from UTC.
pub fn zone_line(here: &jiff::Zoned, zone: Option<&str>) -> String {
    let offset = here.offset();
    let seconds = offset.seconds();
    let utc = if seconds == 0 {
        "the same as UTC".to_string()
    } else {
        let (sign, s) = if seconds < 0 { ("behind", -seconds) } else { ("ahead of", seconds) };
        let (h, m) = (s / 3600, s % 3600 / 60);
        let span = if m == 0 { format!("{h} hour{}", if h == 1 { "" } else { "s" }) } else { format!("{h}:{m:02}") };
        format!("{span} {sign} UTC")
    };
    match zone {
        Some(name) => format!("{} ({}), {utc}", zone_place(name), here.strftime("%Z")),
        None => "UTC".into(),
    }
}

/// A zone's name as a place: `America/Argentina/Buenos_Aires` is
/// "Argentina, Buenos Aires".
pub fn zone_place(name: &str) -> String {
    let mut parts: Vec<String> = name.split('/').map(|p| p.replace('_', " ")).collect();
    if parts.len() > 1 {
        parts.remove(0);
    }
    parts.join(", ")
}

/// A span of seconds as a person says it.
pub fn seconds(n: u64) -> String {
    match n {
        n if n < 120 => format!("{n} seconds"),
        n if n < 7200 => format!("{} minutes", n / 60),
        n => format!("{} hours", n / 3600),
    }
}

pub fn source_state(state: SourceState) -> &'static str {
    match state {
        SourceState::SystemPeer => "Followed",
        SourceState::Candidate => "Agrees",
        SourceState::Outlier => "Agrees, but noisy",
        SourceState::Falseticker => "Disagrees",
        SourceState::Unreachable => "Not answering",
        SourceState::Unusable => "Not usable",
    }
}

/// An offset in seconds, at whatever unit reads clearly.
pub fn offset(seconds: f64) -> String {
    let sign = if seconds < 0.0 { "−" } else { "+" };
    format!("{sign}{}", plain(seconds.abs()))
}

/// A small positive span in seconds, at whatever unit reads clearly.
pub fn plain(seconds: f64) -> String {
    let s = seconds.abs();
    if s < 1e-3 {
        format!("{:.0} µs", s * 1e6)
    } else if s < 1.0 {
        format!("{:.1} ms", s * 1e3)
    } else if s < 120.0 {
        format!("{s:.1} s")
    } else {
        format!("{:.0} min", s / 60.0)
    }
}

/// How long ago, from seconds; negative is never.
pub fn ago(seconds: f64) -> String {
    if seconds < 0.0 {
        return "Never".into();
    }
    match seconds as u64 {
        s if s < 60 => format!("{s} s ago"),
        s if s < 3600 => format!("{} min ago", s / 60),
        s => format!("{} h ago", s / 3600),
    }
}

/// An amount of memory or storage, from bytes.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} bytes")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// How long the machine has been up.
pub fn uptime(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, seconds % 86_400 / 3600, seconds % 3600 / 60);
    let part = |n: u64, unit: &str| format!("{n} {unit}{}", if n == 1 { "" } else { "s" });
    match (days, hours) {
        (0, 0) => part(minutes.max(1), "minute"),
        (0, h) => format!("{}, {}", part(h, "hour"), part(minutes, "minute")),
        (d, h) => format!("{}, {}", part(d, "day"), part(h, "hour")),
    }
}

/// Which way this boot started, and why.
pub fn boot_mode(boot: &peinit::client::Boot) -> String {
    let mode = match boot.mode.as_str() {
        "full" => "Normally",
        "safe" => "In safe mode: only what the machine needs to start",
        "recovery" => "In recovery",
        other => other,
    };
    match boot.reason.as_str() {
        "requested" => format!("{mode}, because the boot asked for it (peios.safemode=1)"),
        "safe_mode_downgrade" if boot.downgrade.is_empty() => format!("{mode}, because a full boot couldn't be planned"),
        "safe_mode_downgrade" => format!("{mode}, because a full boot couldn't be planned: {}", boot.downgrade.join("; ")),
        _ => mode.to_string(),
    }
}

/// Whether this boot has counted as good yet, and how many before it didn't.
pub fn boot_health(boot: &peinit::client::Boot) -> String {
    let good = if boot.confirmed {
        "This boot counts as good.".to_string()
    } else if let Some(why) = &boot.confirm_error {
        format!("This boot can't be counted as good, so the next boot will count it as failed: {why}.")
    } else if !boot.waiting_on.is_empty() {
        format!(
            "Not yet counted as good: waiting for {} to keep running for {} seconds.",
            boot.waiting_on.join(", "),
            boot.grace_seconds
        )
    } else {
        format!("Not yet counted as good: its essential services have to keep running for {} seconds.", boot.grace_seconds)
    };
    let before = match (boot.attempts, boot.max_attempts) {
        (0, _) => String::new(),
        (n, 0) => format!(" {n} boot{} before it never counted as good.", if n == 1 { "" } else { "s" }),
        (n, max) => format!(
            " {n} boot{} before it never counted as good; after {max} in a row, the machine starts in recovery.",
            if n == 1 { "" } else { "s" }
        ),
    };
    format!("{good}{before}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones_read_as_places() {
        assert_eq!(zone_place("Europe/London"), "London");
        assert_eq!(zone_place("America/Argentina/Buenos_Aires"), "Argentina, Buenos Aires");
        assert_eq!(zone_place("UTC"), "UTC");
    }

    #[test]
    fn amounts_read_plainly() {
        assert_eq!(bytes(512), "512 bytes");
        assert_eq!(bytes(4_200_000_000), "4.2 GB");
        assert_eq!(bytes(64_000_000_000), "64 GB");
        assert_eq!(seconds(64), "64 seconds");
        assert_eq!(seconds(1024), "17 minutes");
        assert_eq!(seconds(1 << 17), "36 hours");
        assert_eq!(uptime(30), "1 minute");
        assert_eq!(uptime(3 * 3600 + 120), "3 hours, 2 minutes");
        assert_eq!(uptime(86_400 + 3600), "1 day, 1 hour");
        assert_eq!(offset(-0.0042), "−4.2 ms");
        assert_eq!(ago(-1.0), "Never");
    }

    #[test]
    fn a_status_says_how_the_clock_is_kept() {
        let manual = Status { manual: true, ..Status::default() };
        assert!(keeping(&manual).starts_with("Set by hand"));
        let none = Status::default();
        assert!(keeping(&none).contains("no time server"));
        let kept = Status {
            sync: Sync::Synchronised,
            system_peer: Some("1.time.peios.org".into()),
            root_distance: 0.012,
            sources: 4,
            ..Status::default()
        };
        assert_eq!(keeping(&kept), "Kept from 1.time.peios.org, within 12.0 ms.");
    }
}
