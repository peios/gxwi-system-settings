//! Time & date: what time it is here and how well it is kept, the time
//! zone, setting the clock by hand, and where the time comes from.
//!
//! What timed is doing is asked of it, over its socket, which anyone may
//! ask. The policy is `Machine\System\Time`, which timed watches: the window
//! writes it and timed acts within a second. Setting the clock is a request
//! to timed, which only it carries out, and only while `Automatic` is 0.

use std::os::unix::net::UnixStream;
use std::time::Duration;

use libgxwi::settings::{self, Glyph, Kind, More, Tile, Tone};
use libgxwi::{Fields, escape};
use libtimed::servers::parse_server;
use libtimed::zone::{self, Listed};
use libtimed::{
    ALLOW_UNAUTHENTICATED_VALUE, AUTOMATIC_VALUE, MAX_POLL_VALUE, MIN_POLL_VALUE, Reply, Request,
    SERVERS_VALUE, SourceInfo, SourceState, Status, TIME_KEY, TIME_ZONE_VALUE,
    USE_FROM_DHCP_VALUE,
};
use peios::registry::Data;

use crate::{reg, words};

/// The policy and what timed says, as last read.
#[derive(Debug, Clone, PartialEq)]
pub struct Time {
    pub status: Result<Status, String>,
    pub sources: Vec<SourceInfo>,
    pub policy: Policy,
    /// Whether this person may change the policy, and why not.
    pub may: Result<(), String>,
}

/// `Machine\System\Time`, as written.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Policy {
    pub automatic: bool,
    pub zone: Option<String>,
    pub servers: Vec<String>,
    pub allow_unauthenticated: bool,
    pub use_from_dhcp: bool,
    pub min_poll: Option<u32>,
    pub max_poll: Option<u32>,
}

/// The poll intervals timed allows, as powers of two seconds.
const POLLS: std::ops::RangeInclusive<u32> = 4..=17;
const DEFAULT_MIN_POLL: u32 = 6;
const DEFAULT_MAX_POLL: u32 = 10;

pub fn read() -> Time {
    let policy = Policy {
        automatic: reg::number(TIME_KEY, AUTOMATIC_VALUE).is_none_or(|n| n != 0),
        zone: reg::text(TIME_KEY, TIME_ZONE_VALUE).filter(|z| !z.trim().is_empty()),
        servers: reg::list(TIME_KEY, SERVERS_VALUE).unwrap_or_default(),
        allow_unauthenticated: reg::number(TIME_KEY, ALLOW_UNAUTHENTICATED_VALUE).is_some_and(|n| n != 0),
        use_from_dhcp: reg::number(TIME_KEY, USE_FROM_DHCP_VALUE).is_some_and(|n| n != 0),
        min_poll: reg::number(TIME_KEY, MIN_POLL_VALUE),
        max_poll: reg::number(TIME_KEY, MAX_POLL_VALUE),
    };
    Time {
        status: ask(Request::Status).and_then(|reply| match reply {
            Reply::Status(status) => Ok(status),
            Reply::Error(e) => Err(e),
            other => Err(format!("timed answered {other:?}")),
        }),
        sources: sources().unwrap_or_default(),
        policy,
        may: reg::may_change(TIME_KEY, "the time settings"),
    }
}

fn connect() -> Result<UnixStream, String> {
    let stream = UnixStream::connect(libtimed::SOCKET_PATH).map_err(|e| format!("timed can't be reached: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
    Ok(stream)
}

fn ask(request: Request) -> Result<Reply, String> {
    libtimed::call(&mut connect()?, &request).map_err(|e| e.to_string())
}

fn sources() -> Result<Vec<SourceInfo>, String> {
    let mut stream = connect()?;
    libtimed::send(&mut stream, &Request::Sources.encode()).map_err(|e| e.to_string())?;
    let mut replies = Vec::new();
    loop {
        let reply = Reply::decode(&libtimed::recv(&mut stream).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let more = matches!(&reply, Reply::Sources { more: true, .. });
        replies.push(reply);
        if !more {
            break;
        }
    }
    libtimed::sources_of(replies)
}

/// Puts the policy in the fields.
pub fn fill(policy: &Policy, fields: &mut Fields) {
    fields.set("automatic", if policy.automatic { "on" } else { "" });
    fields.set("zone", policy.zone.as_deref().unwrap_or(""));
    fields.set("servers", &policy.servers.join("\n"));
    fields.set("unauthenticated", if policy.allow_unauthenticated { "on" } else { "" });
    fields.set("dhcp", if policy.use_from_dhcp { "on" } else { "" });
    fields.set("min-poll", &policy.min_poll.unwrap_or(DEFAULT_MIN_POLL).to_string());
    fields.set("max-poll", &policy.max_poll.unwrap_or(DEFAULT_MAX_POLL).to_string());
}

/// The zone the clock is shown in: the one timed says is in force, or, where
/// timed can't be asked, whatever `/etc/localtime` holds now.
pub fn shown_zone(time: &Time) -> jiff::tz::TimeZone {
    match &time.status {
        Ok(status) => status
            .zone
            .as_deref()
            .and_then(|name| jiff::tz::TimeZone::get(name).ok())
            .unwrap_or(jiff::tz::TimeZone::UTC),
        Err(_) => std::fs::read("/etc/localtime")
            .ok()
            .and_then(|bytes| jiff::tz::TimeZone::tzif("localtime", &bytes).ok())
            .unwrap_or(jiff::tz::TimeZone::UTC),
    }
}

/// The name of the zone times are given in when setting the clock.
fn zone_in_force(time: &Time) -> String {
    match &time.status {
        Ok(status) => status.zone.clone().unwrap_or_else(|| "UTC".into()),
        Err(_) => "the time zone this machine is in".into(),
    }
}

/// What the side says of this section.
pub fn now(time: &Time) -> String {
    let zone = time.policy.zone.as_deref().map(words::zone_place).unwrap_or_else(|| "UTC".into());
    format!("{zone} · {}", if time.policy.automatic { "set automatically" } else { "set by hand" })
}

pub fn render(time: &Time, zones: &[Listed], fields: &Fields, now: jiff::Timestamp, editing_servers: bool) -> String {
    let here = now.to_zoned(shown_zone(time));
    let may = time.may.is_ok();
    // One key holds all of it, so why it can't be changed is said once.
    let banner = match &time.may {
        Err(why) => settings::banner(why),
        Ok(()) => String::new(),
    };

    // Now: the time here, and how well it is known.
    let (pill, tone, kept) = match &time.status {
        Ok(status) => {
            let (pill, tone) = words::keeping_short(status);
            (pill, tone, words::keeping(status))
        }
        Err(why) => ("timed isn't answering".to_string(), Tone::Bad, why.clone()),
    };
    let hero = settings::hero(
        &settings::big(
            &here.strftime("%H:%M").to_string(),
            &format!(
                "{} · {}",
                here.strftime("%A %-d %B %Y"),
                words::zone_line(&here, time.status.as_ref().ok().and_then(|s| s.zone.as_deref()))
            ),
        ),
        &settings::pill(&pill, tone),
    );

    // The time zone.
    let zone_select = format!(
        r#"<select class="st-select" name="zone" aria-label="Time zone"{}>{}</select>"#,
        if may { "" } else { " disabled" },
        zone_options(zones, fields.get("zone"))
    );
    let not_used = match &time.status {
        Ok(status) if status.zone != time.policy.zone => settings::note(&format!(
            "{} is chosen, but timed couldn't use it: the machine is still on {}. Its log says why.",
            time.policy.zone.as_deref().unwrap_or("UTC"),
            status.zone.as_deref().unwrap_or("UTC")
        )),
        _ => String::new(),
    };
    let zone_group = settings::group(
        "Time Zone",
        &settings::row("Time Zone", "Used by every program to show the time. The clock itself keeps UTC.", &zone_select),
        &not_used,
    );

    // Automatic, or by hand.
    let mut setting = settings::row("Set Time Automatically", &kept, &settings::switch("automatic", "Set Time Automatically", may));
    if !time.policy.automatic {
        if may {
            let date = if fields.get("date").is_empty() { here.strftime("%Y-%m-%d").to_string() } else { fields.get("date").to_string() };
            let clock = if fields.get("clock").is_empty() { here.strftime("%H:%M").to_string() } else { fields.get("clock").to_string() };
            setting.push_str(&settings::more(
                More::Form,
                &format!(
                    r#"<form fx-submit="set-time"><p>A clock set by hand isn't checked against a time server. Times are in {zone}.</p>
                       <div class="fields"><label>Date<input class="st-input wide" type="date" name="date" value="{date}"></label>
                       <label>Time<input class="st-input wide" type="time" name="clock" step="1" value="{clock}"></label></div>{actions}</form>"#,
                    zone = escape(&zone_in_force(time)),
                    date = escape(&date),
                    clock = escape(&clock),
                    actions = settings::actions(&settings::submit("Set Clock", Kind::Primary, true)),
                ),
            ));
        }
    }
    let setting_group = settings::group("Clock", &setting, "");

    // Where the time comes from.
    let summary = if time.policy.servers.is_empty() {
        "System Default: the Peios time servers.".to_string()
    } else {
        time.policy.servers.join(", ")
    };
    let mut servers = settings::row(
        "Servers",
        &summary,
        &if editing_servers || !may { String::new() } else { settings::button("Change…", "edit-servers", &[], Kind::Plain, true) },
    );
    if editing_servers && may {
        servers.push_str(&settings::more(
            More::Form,
            &format!(
                r#"<form fx-submit="save-servers"><textarea class="st-input" name="servers" rows="4" spellcheck="false" aria-label="Time servers" placeholder="0.time.peios.org&#10;1.time.peios.org&#10;2.time.peios.org&#10;3.time.peios.org" fx-autofocus></textarea>
                   <p>One server per line, optionally with <code>:port</code>, then <code>prefer</code> to favour it or <code>unauthenticated</code> for a server without NTS. Leave empty for the System Default. Use at least three, so one wrong server is outvoted.</p>{}</form>"#,
                settings::actions(&format!(
                    "{}{}",
                    settings::button("Cancel", "cancel-servers", &[], Kind::Plain, true),
                    settings::submit("Save", Kind::Primary, true)
                ))
            ),
        ));
    }
    servers.push_str(&settings::row(
        "Allow Unauthenticated Servers",
        "Plain NTP, without NTS. Anyone on the network path could send the wrong time.",
        &settings::switch("unauthenticated", "Allow Unauthenticated Servers", may),
    ));
    servers.push_str(&settings::row(
        "Use Network Time Servers",
        "Servers offered by the network (DHCP), when none are listed above.",
        &settings::switch("dhcp", "Use Network Time Servers", may),
    ));
    let polls: Vec<(String, String)> = POLLS.map(|p| (p.to_string(), words::seconds(1u64 << p))).collect();
    servers.push_str(&settings::row(
        "Minimum Poll Interval",
        "The most often the servers are asked.",
        &settings::select("min-poll", "Minimum Poll Interval", &polls, may),
    ));
    servers.push_str(&settings::row(
        "Maximum Poll Interval",
        "The longest the servers go unasked.",
        &settings::select("max-poll", "Maximum Poll Interval", &polls, may),
    ));
    let servers_group = settings::group("Time Servers", &servers, "");

    let heard = if time.sources.is_empty() {
        String::new()
    } else {
        let rows: String = time
            .sources
            .iter()
            .map(|s| {
                let mut lines = vec![(
                    format!(
                        "{} · offset {} · last heard {}",
                        if s.auth == libtimed::Auth::Nts { "Authenticated (NTS)" } else { "Unauthenticated" },
                        words::offset(s.offset),
                        words::ago(s.last).to_lowercase()
                    ),
                    false,
                )];
                if let Some(note) = &s.note {
                    lines.push((note.clone(), false));
                }
                let lines: Vec<(&str, bool)> = lines.iter().map(|(t, m)| (t.as_str(), *m)).collect();
                let tone = match s.state {
                    SourceState::SystemPeer => Tone::Good,
                    SourceState::Candidate => Tone::Plain,
                    _ => Tone::Warn,
                };
                settings::item(
                    &settings::icon(Glyph::Server, Tile::Slate),
                    &s.name,
                    &lines,
                    &settings::pill(words::source_state(s.state), tone),
                )
            })
            .collect();
        settings::group("Server Status", &rows, "")
    };

    format!(
        "{}{banner}{hero}{zone_group}{setting_group}{servers_group}{heard}",
        settings::head(Glyph::Clock, Tile::Blue, "Time & Date", "The system clock and how times are shown.")
    )
}

/// The zones, by region, with UTC first and a chosen zone that isn't
/// listed (an old name, one tzdata dropped) still there as chosen.
fn zone_options(zones: &[Listed], chosen: &str) -> String {
    let mut options = format!(r#"<option value=""{}>UTC</option>"#, if chosen.is_empty() { " selected" } else { "" });
    let mut region = "";
    for listed in zones {
        let this_region = listed.name.split('/').next().unwrap_or("");
        if this_region != region {
            if !region.is_empty() {
                options.push_str("</optgroup>");
            }
            region = this_region;
            options.push_str(&format!("<optgroup label=\"{}\">", escape(region)));
        }
        let label = match &listed.comment {
            Some(comment) => format!("{} — {}", words::zone_place(&listed.name), comment),
            None => words::zone_place(&listed.name),
        };
        options.push_str(&format!(
            "<option value=\"{v}\"{s}>{l}</option>",
            v = escape(&listed.name),
            s = if chosen == listed.name { " selected" } else { "" },
            l = escape(&label)
        ));
    }
    if !region.is_empty() {
        options.push_str("</optgroup>");
    }
    if !chosen.is_empty() && !zones.iter().any(|z| z.name == chosen) {
        options.push_str(&format!("<option value=\"{v}\" selected>{v}</option>", v = escape(chosen)));
    }
    options
}

/// Saves the chosen time zone.
pub fn save_zone(fields: &Fields) -> Result<String, String> {
    let chosen = fields.get("zone").trim();
    if chosen.is_empty() {
        reg::set(TIME_KEY, "the time zone", &[(TIME_ZONE_VALUE, None)])?;
        return Ok("Time zone set to UTC.".into());
    }
    zone::installed(chosen)?;
    reg::set(TIME_KEY, "the time zone", &[(TIME_ZONE_VALUE, Some(Data::Sz(chosen.to_string())))])?;
    Ok(format!("Time zone set to {chosen}."))
}

/// Turns setting the time automatically on or off.
pub fn set_automatic(on: bool) -> Result<String, String> {
    reg::set(TIME_KEY, "how the clock is set", &[(AUTOMATIC_VALUE, Some(Data::Dword(on as u32)))])?;
    Ok(if on {
        "Time is set automatically.".into()
    } else {
        "Automatic time is off. Set the clock below.".into()
    })
}

/// Asks timed to set the clock to the date and time in the fields, in the
/// zone in force.
pub fn set_clock(time: &Time, fields: &Fields) -> Result<String, String> {
    let date: jiff::civil::Date = fields.get("date").trim().parse().map_err(|_| "Give a date.".to_string())?;
    let clock = fields.get("clock").trim();
    let clock: jiff::civil::Time = clock.parse().map_err(|_| "Give a time.".to_string())?;
    let zoned = date
        .to_datetime(clock)
        .to_zoned(shown_zone(time))
        .map_err(|e| format!("That time doesn't exist here: {e}."))?;
    let at = zoned.timestamp();
    let request = Request::Set {
        seconds: at.as_second(),
        nanos: at.subsec_nanosecond().max(0) as u32,
    };
    match ask(request)? {
        Reply::Ok => Ok(format!("Clock set to {}.", zoned.strftime("%H:%M:%S on %A %-d %B %Y"))),
        Reply::Error(why) if why == "not permitted" => Err("timed won't set the clock for you: its control right is needed, which as shipped only Administrators have.".into()),
        Reply::Error(why) => Err(format!("timed didn't set the clock: {why}.")),
        other => Err(format!("timed answered {other:?}.")),
    }
}

/// Saves the named time servers.
pub fn save_servers(fields: &Fields) -> Result<String, String> {
    let servers: Vec<String> = fields
        .get("servers")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    for server in &servers {
        parse_server(server).map_err(|why| format!("“{server}”: {why}."))?;
    }
    reg::set(TIME_KEY, "the time servers", &[(SERVERS_VALUE, (!servers.is_empty()).then(|| Data::MultiSz(servers.clone())))])?;
    Ok(if servers.is_empty() {
        "Time servers set to the System Default.".into()
    } else {
        format!("{} time server{} saved.", servers.len(), if servers.len() == 1 { "" } else { "s" })
    })
}

/// Saves how the time servers are used: the two switches and how often.
pub fn save_options(fields: &Fields) -> Result<String, String> {
    let poll = |name: &str| -> Result<u32, String> {
        fields
            .get(name)
            .parse()
            .ok()
            .filter(|p| POLLS.contains(p))
            .ok_or_else(|| "Choose how often to check.".to_string())
    };
    let (min, max) = (poll("min-poll")?, poll("max-poll")?);
    if max < min {
        return Err("The maximum poll interval can't be shorter than the minimum.".into());
    }
    let on = |name: &str| Data::Dword((fields.get(name) == "on") as u32);
    reg::set(
        TIME_KEY,
        "how the time servers are used",
        &[
            (ALLOW_UNAUTHENTICATED_VALUE, Some(on("unauthenticated"))),
            (USE_FROM_DHCP_VALUE, Some(on("dhcp"))),
            (MIN_POLL_VALUE, Some(Data::Dword(min))),
            (MAX_POLL_VALUE, Some(Data::Dword(max))),
        ],
    )?;
    Ok("Saved.".into())
}

/// Applies a field that applies as it changes.
pub fn apply(name: &str, fields: &Fields) -> Option<Result<String, String>> {
    Some(match name {
        "zone" => save_zone(fields),
        "automatic" => set_automatic(fields.get("automatic") == "on"),
        "unauthenticated" | "dhcp" | "min-poll" | "max-poll" => save_options(fields),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(pairs: &[(&str, &str)]) -> Fields {
        let mut fields = Fields::default();
        for (name, value) in pairs {
            fields.set(name, value);
        }
        fields
    }

    #[test]
    fn the_policy_round_trips_through_the_fields() {
        let policy = Policy {
            automatic: true,
            zone: Some("Europe/London".into()),
            servers: vec!["a.example".into(), "b.example prefer".into()],
            allow_unauthenticated: true,
            use_from_dhcp: false,
            min_poll: None,
            max_poll: Some(12),
        };
        let mut f = Fields::default();
        fill(&policy, &mut f);
        assert_eq!(f.get("automatic"), "on");
        assert_eq!(f.get("zone"), "Europe/London");
        assert_eq!(f.get("servers"), "a.example\nb.example prefer");
        assert_eq!(f.get("unauthenticated"), "on");
        assert_eq!(f.get("dhcp"), "");
        assert_eq!(f.get("min-poll"), "6");
        assert_eq!(f.get("max-poll"), "12");
    }

    #[test]
    fn a_bad_server_is_refused_before_anything_is_written() {
        // parse_server refuses it, so save_servers returns before it opens
        // the registry at all.
        let f = fields(&[("servers", "good.example\nbad.example prefered")]);
        let why = save_servers(&f).unwrap_err();
        assert!(why.contains("bad.example prefered"), "{why}");
    }

    #[test]
    fn polls_out_of_range_or_upside_down_are_refused() {
        let f = fields(&[("min-poll", "3"), ("max-poll", "10")]);
        assert!(save_options(&f).is_err());
        let f = fields(&[("min-poll", "12"), ("max-poll", "8")]);
        assert!(save_options(&f).unwrap_err().contains("shorter than the minimum"));
    }

    #[test]
    fn only_choices_apply_as_they_change() {
        let f = fields(&[("min-poll", "12"), ("max-poll", "8")]);
        assert!(apply("max-poll", &f).is_some_and(|r| r.is_err()));
        assert!(apply("servers", &f).is_none());
        assert!(apply("date", &f).is_none());
    }

    #[test]
    fn a_time_that_isnt_one_is_refused_before_timed_is_asked() {
        let time = Time {
            status: Err("not asked".into()),
            sources: Vec::new(),
            policy: Policy::default(),
            may: Ok(()),
        };
        assert_eq!(set_clock(&time, &fields(&[("date", ""), ("clock", "12:00")])), Err("Give a date.".into()));
        assert_eq!(set_clock(&time, &fields(&[("date", "2026-10-04"), ("clock", "25:00")])), Err("Give a time.".into()));
    }
}
