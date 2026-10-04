//! Time & date: what time it is here and how well it is kept, the time
//! zone, setting the clock by hand, and where the time comes from.
//!
//! What timed is doing is asked of it, over its socket, which anyone may
//! ask. The policy is `Machine\System\Time`, which timed watches: the window
//! writes it and timed acts within a second. Setting the clock is a request
//! to timed, which only it carries out, and only while `Automatic` is 0.

use std::os::unix::net::UnixStream;
use std::time::Duration;

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

/// Puts the policy in the fields, where the form edits it.
pub fn fill(policy: &Policy, fields: &mut Fields) {
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

pub fn render(time: &Time, zones: &[Listed], fields: &Fields, now: jiff::Timestamp) -> String {
    let zone = shown_zone(time);
    let here = now.to_zoned(zone.clone());
    let may = time.may.is_ok();
    let disabled = if may { "" } else { " disabled" };

    // Now: the time here, and how well it is known.
    let kept = match &time.status {
        Err(why) => format!("<p class=\"note bad\">{}</p>", escape(why)),
        Ok(status) => format!("<p class=\"note\">{}</p>", escape(&words::keeping(status))),
    };
    let zone_said = match &time.status {
        Ok(status) if status.zone != time.policy.zone => format!(
            "<p class=\"note bad\">{}</p>",
            escape(&format!(
                "{} is chosen, but timed couldn't use it: the machine is still on {}. Its log says why.",
                time.policy.zone.as_deref().unwrap_or("UTC"),
                status.zone.as_deref().unwrap_or("UTC")
            ))
        ),
        _ => String::new(),
    };
    let now_card = format!(
        "<section class=\"card now\" aria-label=\"Now\"><h2>Now</h2>\
         <p class=\"clock\"><time datetime=\"{iso}\">{time}</time></p>\
         <p class=\"date\">{date}</p>\
         <p class=\"zone\">{zone_name}</p>{kept}{zone_said}</section>",
        iso = escape(&now.to_string()),
        time = here.strftime("%H:%M"),
        date = here.strftime("%A %-d %B %Y"),
        zone_name = escape(&words::zone_line(&here, time.status.as_ref().ok().and_then(|s| s.zone.as_deref()))),
    );

    // The time zone.
    let chosen = fields.get("zone");
    let mut options = format!(
        "<option value=\"\"{}>UTC — no time zone</option>",
        if chosen.is_empty() { " selected" } else { "" }
    );
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
    // A zone that is chosen but not in the list of ones worth picking (an
    // old name, or one tzdata dropped) is still shown as chosen.
    if !chosen.is_empty() && !zones.iter().any(|z| z.name == chosen) {
        options.push_str(&format!("<option value=\"{v}\" selected>{v}</option>", v = escape(chosen)));
    }
    let zone_changed = chosen != time.policy.zone.as_deref().unwrap_or("");
    let zone_card = format!(
        "<section class=\"card\" aria-label=\"Time zone\"><h2>Time zone</h2>\
         <form class=\"edit\" fx-submit=\"save-zone\">\
         <label>Time zone<select name=\"zone\"{disabled}>{options}</select></label>\
         <p class=\"hint\">How every program shows the time. Times are kept in UTC; this is only how they are shown.</p>\
         {save}</form></section>",
        save = save_button(may, zone_changed, "Save"),
    );

    // Automatic, or by hand.
    let setting = if time.policy.automatic {
        let button = if may {
            "<div class=\"actions\"><button type=\"button\" fx-click=\"by-hand\">Set the time myself</button></div>"
        } else {
            ""
        };
        format!("<p class=\"note\">The clock is set automatically, from the time servers below.</p>{button}")
    } else if !may {
        "<p class=\"note\">The clock is set by hand, and nothing checks it: it keeps time as well as this machine's own clock does.</p>".to_string()
    } else {
        let date = if fields.get("date").is_empty() { here.strftime("%Y-%m-%d").to_string() } else { fields.get("date").to_string() };
        let clock = if fields.get("clock").is_empty() { here.strftime("%H:%M").to_string() } else { fields.get("clock").to_string() };
        format!(
            "<p class=\"note\">The clock is set by hand, and nothing checks it: it keeps time as well as this machine's own clock does.</p>\
             <form class=\"edit\" fx-submit=\"set-time\">\
             <div class=\"row\"><label>Date<input type=\"date\" name=\"date\" value=\"{date}\"{disabled}></label>\
             <label>Time<input type=\"time\" name=\"clock\" step=\"1\" value=\"{clock}\"{disabled}></label></div>\
             <p class=\"hint\">In {zone}.</p>\
             <div class=\"actions\"><button class=\"primary\"{disabled}>Set the clock</button>\
             <button type=\"button\" fx-click=\"automatic\"{disabled}>Set the time automatically</button></div></form>",
            date = escape(&date),
            clock = escape(&clock),
            zone = escape(&zone_in_force(time)),
        )
    };
    let setting_card = format!("<section class=\"card\" aria-label=\"Setting the clock\"><h2>Setting the clock</h2>{setting}</section>");

    // Where the time comes from.
    let servers_changed = fields.get("servers").trim() != time.policy.servers.join("\n").trim()
        || (fields.get("unauthenticated") == "on") != time.policy.allow_unauthenticated
        || (fields.get("dhcp") == "on") != time.policy.use_from_dhcp
        || fields.get("min-poll") != time.policy.min_poll.unwrap_or(DEFAULT_MIN_POLL).to_string()
        || fields.get("max-poll") != time.policy.max_poll.unwrap_or(DEFAULT_MAX_POLL).to_string();
    let polls = |name: &str| {
        POLLS
            .map(|p| {
                format!(
                    "<option value=\"{p}\"{s}>{l}</option>",
                    s = if fields.get(name) == p.to_string() { " selected" } else { "" },
                    l = escape(&words::seconds(1u64 << p))
                )
            })
            .collect::<String>()
    };
    let check = |name: &str| if fields.get(name) == "on" { " checked" } else { "" };
    let servers_card = format!(
        "<section class=\"card\" aria-label=\"Time servers\"><h2>Time servers</h2>\
         <form class=\"edit\" fx-submit=\"save-servers\">\
         <label>Servers<textarea name=\"servers\" rows=\"4\" spellcheck=\"false\" placeholder=\"0.time.peios.org&#10;1.time.peios.org&#10;2.time.peios.org&#10;3.time.peios.org\"{disabled}></textarea></label>\
         <p class=\"hint\">One a line, with a port after a colon if it isn't the usual, then <code>prefer</code> to lead with it or <code>unauthenticated</code> for one that doesn't speak NTS. Empty uses Peios's own four, run by three independent operators. Use at least three, so one wrong one is outvoted.</p>\
         <label class=\"check\"><input type=\"checkbox\" name=\"unauthenticated\"{u}{disabled}> Allow servers that don't prove who they are (plain NTP)</label>\
         <label class=\"check\"><input type=\"checkbox\" name=\"dhcp\"{d}{disabled}> Use the time servers the network offers, when none are named above</label>\
         <div class=\"row\"><label>Check at most every<select name=\"min-poll\"{disabled}>{min}</select></label>\
         <label>and at least every<select name=\"max-poll\"{disabled}>{max}</select></label></div>\
         {save}</form>{sources}</section>",
        u = check("unauthenticated"),
        d = check("dhcp"),
        min = polls("min-poll"),
        max = polls("max-poll"),
        save = save_button(may, servers_changed, "Save"),
        sources = sources_table(&time.sources),
    );

    let why = match &time.may {
        Err(why) => format!("<p class=\"why\">{}</p>", escape(why)),
        Ok(()) => String::new(),
    };
    format!("{why}{now_card}{zone_card}{setting_card}{servers_card}")
}

fn save_button(may: bool, changed: bool, label: &str) -> String {
    if !may {
        return String::new();
    }
    format!(
        "<div class=\"actions\"><button class=\"primary\"{}>{label}</button></div>",
        if changed { "" } else { " disabled" }
    )
}

fn sources_table(sources: &[SourceInfo]) -> String {
    if sources.is_empty() {
        return String::new();
    }
    let rows: String = sources
        .iter()
        .map(|s| {
            let note = s.note.as_deref().map(|n| format!("<small>{}</small>", escape(n))).unwrap_or_default();
            format!(
                "<tr class=\"{class}\"><th scope=\"row\">{name}{note}</th><td>{state}</td><td>{auth}</td><td class=\"num\">{offset}</td><td class=\"num\">{last}</td></tr>",
                class = if s.state == SourceState::SystemPeer { "peer" } else { "" },
                name = escape(&s.name),
                state = escape(words::source_state(s.state)),
                auth = if s.auth == libtimed::Auth::Nts { "NTS" } else { "None" },
                offset = escape(&words::offset(s.offset)),
                last = escape(&words::ago(s.last)),
            )
        })
        .collect();
    format!(
        "<table class=\"sources\"><caption>What timed hears from them</caption>\
         <thead><tr><th scope=\"col\">Server</th><th scope=\"col\">State</th><th scope=\"col\">Proves itself</th><th scope=\"col\" class=\"num\">Off by</th><th scope=\"col\" class=\"num\">Heard</th></tr></thead>\
         <tbody>{rows}</tbody></table>"
    )
}

/// Saves the chosen time zone.
pub fn save_zone(fields: &Fields) -> Result<String, String> {
    let chosen = fields.get("zone").trim();
    if chosen.is_empty() {
        reg::set(TIME_KEY, "the time zone", &[(TIME_ZONE_VALUE, None)])?;
        return Ok("The time zone is UTC.".into());
    }
    zone::installed(chosen)?;
    reg::set(TIME_KEY, "the time zone", &[(TIME_ZONE_VALUE, Some(Data::Sz(chosen.to_string())))])?;
    Ok(format!("The time zone is {chosen}."))
}

/// Turns setting the time automatically on or off.
pub fn set_automatic(on: bool) -> Result<String, String> {
    reg::set(TIME_KEY, "how the clock is set", &[(AUTOMATIC_VALUE, Some(Data::Dword(on as u32)))])?;
    Ok(if on {
        "The clock is set automatically again. The first time servers to agree will set it.".into()
    } else {
        "The clock is set by hand now.".into()
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
        Reply::Ok => Ok(format!("The clock is set to {}.", zoned.strftime("%H:%M:%S on %A %-d %B %Y"))),
        Reply::Error(why) if why == "not permitted" => Err("timed won't set the clock for you: its control right is needed, which as shipped only Administrators have.".into()),
        Reply::Error(why) => Err(format!("timed didn't set the clock: {why}.")),
        other => Err(format!("timed answered {other:?}.")),
    }
}

/// Saves the time servers and how they are used.
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
        return Err("Checking at least every so often can't be more often than at most.".into());
    }
    let on = |name: &str| Data::Dword((fields.get(name) == "on") as u32);
    reg::set(
        TIME_KEY,
        "the time servers",
        &[
            (SERVERS_VALUE, (!servers.is_empty()).then(|| Data::MultiSz(servers.clone()))),
            (ALLOW_UNAUTHENTICATED_VALUE, Some(on("unauthenticated"))),
            (USE_FROM_DHCP_VALUE, Some(on("dhcp"))),
            (MIN_POLL_VALUE, Some(Data::Dword(min))),
            (MAX_POLL_VALUE, Some(Data::Dword(max))),
        ],
    )?;
    Ok(if servers.is_empty() {
        "Peios's own time servers are used.".into()
    } else {
        format!("{} time server{} named.", servers.len(), if servers.len() == 1 { " is" } else { "s are" })
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
        let f = fields(&[("servers", "good.example\nbad.example prefered"), ("min-poll", "6"), ("max-poll", "10")]);
        let why = save_servers(&f).unwrap_err();
        assert!(why.contains("bad.example prefered"), "{why}");
    }

    #[test]
    fn polls_out_of_range_or_upside_down_are_refused() {
        let f = fields(&[("servers", ""), ("min-poll", "3"), ("max-poll", "10")]);
        assert!(save_servers(&f).is_err());
        let f = fields(&[("servers", ""), ("min-poll", "12"), ("max-poll", "8")]);
        assert!(save_servers(&f).unwrap_err().contains("more often"));
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
