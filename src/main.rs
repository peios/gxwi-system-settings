//! System Settings: this machine's time and time zone, language and
//! keyboard, name, and how it starts, as far as the person looking may see
//! and change them.
//!
//! It asks the daemons that own each of these — timed, peinit — over their
//! own sockets, and changes the registry keys they read, with no more
//! authority than the person has. A person's own settings are My Settings';
//! the desktop's are Desktop Settings'.

use std::sync::{Arc, Weak};
use std::time::Duration;

use libauthd_client::ident::Ident;
use libgxwi::{App, Surface};

mod about;
mod language;
mod reg;
mod settings;
mod startup;
mod time;
mod words;

use settings::{Read, Settings};

// What this program looks like, to whatever lists it. The icon itself is
// `gxwi-system-settings.svg` at the repo root, installed as the base theme's.
libgxwi::icon!(b"dev.peios.gxwi-system-settings");

/// How often the clock is looked at, and how many of those between reading
/// everything again. Nothing tells a program when a setting changes, so the
/// window looks.
const TICK: Duration = Duration::from_secs(1);
const READ_EVERY: u32 = 5;

/// Looks again every little while, for as long as the window is there. The
/// window is only changed when what it shows would change: the clock's
/// minute turning, or something read differing. Every change shows every
/// page the window again, which is not worth doing for nothing.
fn look_again(window: Weak<Surface<Settings>>) {
    let mut ticks = 0u32;
    loop {
        std::thread::sleep(TICK);
        ticks += 1;
        let Some(shown) = window.upgrade() else { return };
        let now = jiff::Timestamp::now();
        let read = if ticks % READ_EVERY == 0 {
            let ident = shown.look(|settings, _, _| settings.ident().clone());
            drop(shown);
            Some(Read::now(&ident))
        } else {
            drop(shown);
            None
        };
        let Some(shown) = window.upgrade() else { return };
        let differs = shown.look(|settings, _, _| settings.minute_turned(now) || read.as_ref().is_some_and(|read| settings.differs(read)));
        if differs {
            shown.update(|settings, fields| {
                settings.now = now;
                if let Some(read) = read {
                    settings.heard(read, fields);
                }
            });
        }
    }
}

fn main() {
    if std::env::args().nth(1).is_some() {
        eprintln!("gxwi-system-settings: usage: gxwi-system-settings (given {:?})", std::env::args().skip(1).collect::<Vec<_>>());
        std::process::exit(64);
    }
    let mut app = match App::connect() {
        Ok(app) => app,
        Err(e) => {
            eprintln!("gxwi-system-settings: no desktop to open on: {e}");
            std::process::exit(1);
        }
    };
    libgxwi::settings::stylesheet(&mut app);
    app.stylesheet("/gxwi-system-settings.css", include_str!("gxwi-system-settings.css"));
    let ident = Ident::new();
    let read = Read::now(&ident);
    let window = app.live("System Settings", Settings::new(ident, read));
    let aside = Arc::downgrade(&window);
    window.update(|settings, fields| {
        settings.window = aside.clone();
        settings.fill(fields);
    });
    std::thread::spawn(move || look_again(aside));
    if let Err(e) = app.run() {
        eprintln!("gxwi-system-settings: {e}");
        std::process::exit(1);
    }
}
