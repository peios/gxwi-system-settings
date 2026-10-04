//! About: what this machine is and runs, and its name.
//!
//! Everything here is read from files anyone may read — os-release,
//! `/proc`, `/sys` — except the name, which is
//! `Machine\System\Network Hostname`, which netd applies as it changes.

use libgxwi::{Fields, escape};
use libnetd::NETWORK_KEY;
use libnetd::hostname::{self, HOSTNAME_VALUE};
use peios::registry::Data;

use crate::{reg, words};

#[derive(Debug, Clone, PartialEq)]
pub struct About {
    /// `PRETTY_NAME`, `VERSION_ID` and `VARIANT` from os-release.
    pub system: Option<String>,
    pub version: Option<String>,
    pub edition: Option<String>,
    pub kernel: String,
    pub uptime: Option<u64>,
    /// The name the machine has now, and the one it is configured to have.
    pub name: String,
    pub configured: Option<String>,
    pub may_name: Result<(), String>,
    pub processor: Option<String>,
    pub processors: usize,
    pub memory: Option<u64>,
    pub maker: Option<String>,
    pub model: Option<String>,
    pub disks: Vec<Disk>,
    /// The system's filesystem: its size and what is free.
    pub root: Option<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Disk {
    pub name: String,
    pub size: u64,
    pub model: Option<String>,
    pub removable: bool,
}

pub fn read() -> About {
    let release = std::fs::read_to_string("/usr/lib/os-release").unwrap_or_default();
    let field = |name: &str| {
        release.lines().find_map(|line| {
            let value = line.strip_prefix(name)?.strip_prefix('=')?;
            Some(value.trim().trim_matches('"').to_string())
        })
    };
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    // SAFETY: uname fills a struct of plain arrays.
    let (kernel, name) = unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut u) == 0 {
            (c_text(&u.release), c_text(&u.nodename))
        } else {
            (String::new(), String::new())
        }
    };
    About {
        system: field("PRETTY_NAME").or_else(|| field("NAME")),
        version: field("VERSION_ID"),
        edition: field("VARIANT"),
        kernel,
        uptime: std::fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|t| t.split_whitespace().next()?.parse::<f64>().ok())
            // To the minute, which is all that is shown of it.
            .map(|s| s as u64 / 60 * 60),
        name,
        configured: reg::text(NETWORK_KEY, HOSTNAME_VALUE).filter(|n| !n.is_empty()),
        may_name: reg::may_change(NETWORK_KEY, "the machine's name"),
        processor: cpuinfo.lines().find_map(|l| {
            let (key, value) = l.split_once(':')?;
            (key.trim() == "model name").then(|| value.trim().to_string())
        }),
        processors: cpuinfo.lines().filter(|l| l.split(':').next().is_some_and(|k| k.trim() == "processor")).count(),
        memory: meminfo.lines().find_map(|l| {
            let rest = l.strip_prefix("MemTotal:")?;
            Some(rest.trim().trim_end_matches("kB").trim().parse::<u64>().ok()? * 1024)
        }),
        maker: dmi("sys_vendor"),
        model: dmi("product_name"),
        disks: disks(),
        root: statvfs("/"),
    }
}

fn c_text(chars: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = chars.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A DMI string the firmware gives, where it gives a real one.
fn dmi(name: &str) -> Option<String> {
    let text = std::fs::read_to_string(format!("/sys/class/dmi/id/{name}")).ok()?;
    let text = text.trim();
    let placeholder = ["", "To be filled by O.E.M.", "System manufacturer", "System Product Name", "Default string", "None"];
    (!placeholder.contains(&text)).then(|| text.to_string())
}

fn disks() -> Vec<Disk> {
    let Ok(entries) = std::fs::read_dir("/sys/block") else {
        return Vec::new();
    };
    let mut disks: Vec<Disk> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            // Only real devices: no loop, RAM or compressed-RAM devices.
            if ["loop", "ram", "zram"].iter().any(|p| name.starts_with(p)) {
                return None;
            }
            let path = entry.path();
            let read = |file: &str| std::fs::read_to_string(path.join(file)).ok().map(|t| t.trim().to_string());
            let size = read("size")?.parse::<u64>().ok()? * 512;
            // Nothing smaller than a megabyte holds anything worth listing:
            // an empty drive reads as nothing, and a floppy as a few KB.
            if size < 1_000_000 {
                return None;
            }
            Some(Disk {
                name,
                size,
                model: read("device/model").filter(|m| !m.is_empty()),
                removable: read("removable").as_deref() == Some("1"),
            })
        })
        .collect();
    disks.sort_by(|a, b| a.name.cmp(&b.name));
    disks
}

fn statvfs(path: &str) -> Option<(u64, u64)> {
    let path = std::ffi::CString::new(path).ok()?;
    // SAFETY: a live path and a struct statvfs fills.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let block = s.f_frsize as u64;
    Some((s.f_blocks as u64 * block, s.f_bavail as u64 * block))
}

pub fn fill(about: &About, fields: &mut Fields) {
    fields.set("hostname", about.configured.as_deref().unwrap_or(&about.name));
}

pub fn render(about: &About, fields: &Fields) -> String {
    let row = |label: &str, value: &str| format!("<dt>{}</dt><dd>{}</dd>", escape(label), escape(value));
    let mut system = String::new();
    if let Some(name) = &about.system {
        system.push_str(&row("System", name));
    }
    if let Some(edition) = &about.edition {
        system.push_str(&row("Edition", edition));
    }
    if let Some(version) = &about.version {
        system.push_str(&row("Version", version));
    }
    system.push_str(&row("Kernel", &about.kernel));
    if let Some(up) = about.uptime {
        system.push_str(&row("Up for", &words::uptime(up)));
    }

    let may = about.may_name.is_ok();
    let typed = fields.get("hostname");
    let changed = typed.trim() != about.configured.as_deref().unwrap_or(&about.name);
    let problem = if changed && !typed.trim().is_empty() {
        hostname::check(typed).err().map(|why| format!("<p class=\"note bad\">{}</p>", escape(&why))).unwrap_or_default()
    } else {
        String::new()
    };
    let differs = match &about.configured {
        Some(configured) if *configured != about.name => format!(
            "<p class=\"note\">{}</p>",
            escape(&format!("It is called {} now; netd hasn't made it {configured} yet.", about.name))
        ),
        _ => String::new(),
    };
    let name_card = format!(
        "<section class=\"card\" aria-label=\"Name\"><h2>Name</h2>\
         <form class=\"edit\" fx-submit=\"save-name\">\
         <label>This machine's name<input name=\"hostname\" autocomplete=\"off\" spellcheck=\"false\" maxlength=\"{max}\"{d}></label>\
         <p class=\"hint\">How it is known on the network: letters, digits and hyphens, one word.</p>{problem}{differs}{save}</form>{why}</section>",
        max = hostname::MAX,
        d = if may { "" } else { " disabled" },
        save = if may {
            format!("<div class=\"actions\"><button class=\"primary\"{}>Save</button></div>", if changed && problem.is_empty() { "" } else { " disabled" })
        } else {
            String::new()
        },
        why = match &about.may_name {
            Ok(()) => String::new(),
            Err(why) => format!("<p class=\"why\">{}</p>", escape(why)),
        },
    );

    let mut hardware = String::new();
    match (&about.maker, &about.model) {
        (Some(maker), Some(model)) => hardware.push_str(&row("Computer", &format!("{maker} {model}"))),
        (Some(one), None) | (None, Some(one)) => hardware.push_str(&row("Computer", one)),
        (None, None) => {}
    }
    if let Some(processor) = &about.processor {
        let count = if about.processors > 1 { format!(", {} processors", about.processors) } else { String::new() };
        hardware.push_str(&row("Processor", &format!("{processor}{count}")));
    }
    if let Some(memory) = about.memory {
        hardware.push_str(&row("Memory", &words::bytes(memory)));
    }

    let mut storage = String::new();
    for disk in &about.disks {
        let what = match &disk.model {
            Some(model) => format!("{}, {model}{}", words::bytes(disk.size), if disk.removable { ", removable" } else { "" }),
            None => format!("{}{}", words::bytes(disk.size), if disk.removable { ", removable" } else { "" }),
        };
        storage.push_str(&row(&disk.name, &what));
    }
    if let Some((size, free)) = about.root {
        storage.push_str(&row("The system's filesystem", &format!("{} free of {}", words::bytes(free), words::bytes(size))));
    }

    format!(
        "<section class=\"card\" aria-label=\"This machine\"><h2>This machine</h2><dl class=\"facts\">{system}</dl></section>\
         {name_card}\
         <section class=\"card\" aria-label=\"Hardware\"><h2>Hardware</h2><dl class=\"facts\">{hardware}</dl></section>\
         <section class=\"card\" aria-label=\"Storage\"><h2>Storage</h2><dl class=\"facts\">{storage}</dl>\
         </section>"
    )
}

pub fn save_name(fields: &Fields) -> Result<String, String> {
    let name = hostname::check(fields.get("hostname"))?;
    reg::set(NETWORK_KEY, "the machine's name", &[(HOSTNAME_VALUE, Some(Data::Sz(name.clone())))])?;
    Ok(format!("This machine is called {name}."))
}
