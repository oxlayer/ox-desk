// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Native tray, through StatusNotifierItem.
//!
//! Spec: ../../specs/001-control-plane.md
//!
//! ## Why leave Electron
//!
//! The tray lived in `gui/main.js`. It worked, but it carried two
//! consequences that bother in daily use:
//!
//! - **It depended on Electron being up.** Closing the window kept the
//!   process alive just for the icon to exist; killing it took the tray
//!   along, and with it the only way back from the KVM through the menu.
//! - **It talked to `oxdesk` over HTTP** to read state and send focus.
//!   Here the tray IS `oxdesk`: it calls `kvm::send` directly, without
//!   serializing anything.
//!
//! StatusNotifierItem is pure D-Bus -- it needs neither GTK nor a
//! graphical event loop, which lets it live inside a daemon that already
//! serves HTTP. On GNOME what draws it is the `ubuntu-appindicators`
//! extension, the same one Electron used underneath.
//!
//! ## A click opens the menu
//!
//! It was an explicit request: switch machines by clicking the tray, not
//! through the border. `activate` (left click) returns the menu, so the
//! gesture is one click and one choice -- without dragging the pointer to
//! any corner. The border stays in the code, turned off by config.

#![cfg(target_os = "linux")]

use std::sync::{Arc, Mutex};

use crate::config::Config;

/// What the tray needs to know, already resolved.
///
/// Snapshotted by a watcher on another thread instead of queried while
/// drawing the menu: reading HID++ costs ~360 ms, and a menu that freezes
/// for a third of a second when opening looks broken.
#[derive(Default, Clone)]
pub struct TrayState {
    /// "device: battery" lines, read-only.
    pub devices: Vec<String>,
    /// Remote hosts (name) the input can be sent to.
    pub remotes: Vec<String>,
    /// Name of the local machine, for the bring-back item.
    pub here: Option<String>,
    /// Where the input is going now, if not here.
    pub target: Option<String>,
}

static STATE: Mutex<Option<TrayState>> = Mutex::new(None);

pub fn update_state(f: TrayState) {
    if let Ok(mut g) = STATE.lock() {
        *g = Some(f);
    }
}

fn current_state() -> TrayState {
    STATE.lock().ok().and_then(|g| g.clone()).unwrap_or_default()
}

pub struct OxdeskTray {
    cfg: Arc<std::sync::RwLock<Config>>,
}

impl ksni::Tray for OxdeskTray {
    fn id(&self) -> String {
        "oxdesk".into()
    }

    fn title(&self) -> String {
        "OxDesk".into()
    }

    fn icon_name(&self) -> String {
        // A theme icon, not a file of ours: the tray has to follow the
        // system's light/dark theme, and a fixed PNG is wrong half the
        // time.
        "input-mouse".into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let f = current_state();
        let summary = if f.devices.is_empty() {
            "no devices".to_string()
        } else {
            f.devices.join(" | ")
        };
        let location = match &f.target {
            Some(a) => format!("input on {a}"),
            None => "input here".into(),
        };
        ksni::ToolTip {
            title: "OxDesk".into(),
            description: format!("{location}\n{summary}"),
            icon_name: String::new(),
            icon_pixmap: Vec::new(),
        }
    }

    /// Left click opens the menu, which is the requested gesture.
    fn activate(&mut self, _x: i32, _y: i32) {}

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};
        let f = current_state();
        let mut items: Vec<MenuItem<Self>> = Vec::new();

        if f.devices.is_empty() {
            items.push(
                StandardItem {
                    label: "no known device".into(),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
        } else {
            for d in &f.devices {
                items.push(
                    StandardItem { label: d.clone(), enabled: false, ..Default::default() }.into(),
                );
            }
        }

        if !f.remotes.is_empty() {
            items.push(MenuItem::Separator);
            for name in &f.remotes {
                let active = f.target.as_deref() == Some(name.as_str());
                let target = name.clone();
                items.push(
                    StandardItem {
                        // The dot marks where the input is going. Without
                        // it the menu cannot tell "I can send" from "it is
                        // already there".
                        label: format!("{}Send to {name}", if active { "● " } else { "" }),
                        activate: Box::new(move |b: &mut OxdeskTray| b.send_to(&target)),
                        ..Default::default()
                    }
                    .into(),
                );
            }
            // "Bring back" shows up when the input is elsewhere, period.
            // The Electron menu made it conditional on "some device
            // offline", a signal that only holds for HID++ -- in LAN mode
            // the device never leaves here, and the item vanished precisely
            // in the mode where it is the only way back.
            if let (Some(here), Some(_)) = (&f.here, &f.target) {
                let name = here.clone();
                let label = format!("Bring it to {here}");
                items.push(
                    StandardItem {
                        label: label,
                        activate: Box::new(move |b: &mut OxdeskTray| b.send_to(&name)),
                        ..Default::default()
                    }
                    .into(),
                );
            }
        }

        // A large file selection waiting for a destination.
        //
        // Above the cap we do not send to everyone on our own: copying
        // 1.4 GB to two machines nobody is looking at wastes someone
        // else's bandwidth and disk. So the menu asks where, one
        // destination at a time. See [`crate::clip_files::on_copy`].
        if let Some((paths, bytes)) = crate::clip_files::offer() {
            items.push(MenuItem::Separator);
            items.push(
                StandardItem {
                    label: format!(
                        "{} file(s) copied, {}",
                        paths.len(),
                        crate::clip_files::human_size(bytes)
                    ),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
            for name in &f.remotes {
                let target = name.clone();
                items.push(
                    StandardItem {
                        label: format!("Send files to {name}"),
                        activate: Box::new(move |b: &mut OxdeskTray| b.send_files(&target)),
                        ..Default::default()
                    }
                    .into(),
                );
            }
        }

        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Open the panel".into(),
                activate: Box::new(|b: &mut OxdeskTray| b.open_panel()),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Set up this machine".into(),
                activate: Box::new(|b: &mut OxdeskTray| b.open_route("/setup")),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

impl OxdeskTray {
    /// Sends the large file selection to a chosen destination.
    ///
    /// On a thread because it can be 1.4 GB: blocking here would freeze
    /// the whole menu -- and the tray runs on the same thread that answers
    /// DBusMenu, so the menu would stop opening.
    fn send_files(&self, host: &str) {
        let Ok(g) = self.cfg.read() else { return };
        let snap = g.clone();
        drop(g);
        let target = host.to_string();
        std::thread::spawn(move || match crate::clip_files::send_offer(&snap, &target) {
            Ok(m) => crate::say!("tray: {m}"),
            Err(e) => crate::say!("tray: {e}"),
        });
    }

    fn send_to(&self, host: &str) {
        let snap = match self.cfg.read() {
            Ok(g) => g.clone(),
            Err(_) => return,
        };
        // Directly, without going through HTTP: the tray IS oxdesk.
        let r = if snap.input.mode == "lan" {
            crate::kvm::send(&snap, host).map(|d| format!("input to {host} ({d})"))
        } else {
            let req = crate::focus::FocusRequest {
                host: host.to_string(),
                route: None,
                confirm: true,
                fast: true,
            };
            let o = crate::focus::execute(&snap, &req);
            match o.error {
                Some(e) => Err(e),
                None => Ok(format!("focus to {host}")),
            }
        };
        match r {
            Ok(m) => crate::say!("tray: {m}"),
            // "error" keeps the line classified as an error by `logs::classify`.
            Err(e) => crate::say!("tray: error sending to {host}: {e}"),
        }
    }

    fn open_panel(&self) {
        self.open_route("/");
    }

    /// Opens a panel route in the default browser.
    ///
    /// `xdg-open` and not the Electron window: the native tray lives
    /// inside the daemon and has no way to talk to the shell -- it may not
    /// even be running. The browser always is.
    fn open_route(&self, route: &str) {
        let port = self.cfg.read().map(|g| g.port).unwrap_or(8787);
        let _ = crate::sys::child("xdg-open")
            .arg(format!("http://127.0.0.1:{port}{route}"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

/// Starts the tray and returns the handle to update the menu.
///
/// Failing here is **not** fatal: without the AppIndicator extension (or
/// in an environment with no tray at all) the item simply does not
/// appear, and the daemon keeps serving the panel and the API. Taking
/// `oxdesk` down because there is nowhere to draw an icon would trade an
/// inconvenience for an outage.
/// Brings the tray up as soon as there is somewhere to draw it, and then
/// keeps the menu fresh.
///
/// Why a loop and not one attempt: the icon is drawn by whoever owns
/// `org.kde.StatusNotifierWatcher`, and on GNOME that is the AppIndicator
/// extension -- which loads after the shell, while `oxdesk.service` starts
/// with the session. The first attempt therefore loses the race on a cold
/// login and fails with `ServiceUnknown`. The measurement is in
/// [`crate::wait`].
///
/// It asks for as long as the daemon lives, because the other reason the
/// watcher can be missing -- no tray at all, or the extension turned off --
/// can stop being true at any moment, and asking again every 30 s costs
/// nothing measurable. Only the transitions are logged: one line for "not
/// yet", one for "up". A line per attempt would bury the log of a machine
/// that genuinely has no tray.
/// Whether this daemon has published its tray item.
///
/// The wizard asks. It used to measure a RUNNING ELECTRON PROCESS, which
/// stopped being the tray when this module replaced it -- so it answered
/// "not running" on a machine whose icon was on the bus and clickable.
static PUBLISHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn is_published() -> bool {
    PUBLISHED.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn start_when_drawable(cfg: Arc<std::sync::RwLock<Config>>) {
    std::thread::spawn(move || {
        let mut attempt = 0u32;
        loop {
            match spawn_item(Arc::clone(&cfg)) {
                Ok(handle) => {
                    PUBLISHED.store(true, std::sync::atomic::Ordering::Relaxed);
                    crate::say!("native tray up");
                    watch(handle, cfg);
                    return;
                }
                Err(e) => {
                    if attempt == 0 {
                        crate::say!(
                            "native tray not up yet ({e}); asking again while the daemon lives"
                        );
                    }
                    std::thread::sleep(crate::wait::retry_delay(attempt));
                    attempt = attempt.saturating_add(1);
                }
            }
        }
    });
}

/// One attempt to publish the item. The error comes back as a string
/// because the only thing the caller does with it is print it once.
fn spawn_item(
    cfg: Arc<std::sync::RwLock<Config>>,
) -> Result<ksni::blocking::Handle<OxdeskTray>, String> {
    use ksni::blocking::TrayMethods;
    OxdeskTray { cfg }.spawn().map_err(|e| e.to_string())
}

/// Keeps the menu fresh on its own thread.
///
/// The snapshot is rebuilt every 5 s and only pushed to the tray when it
/// CHANGES. Calling `update` on every loop would make the shell redraw
/// the menu for no reason -- and, on an open menu, that flickers in the
/// face of whoever is choosing.
pub fn watch(handle: ksni::blocking::Handle<OxdeskTray>, cfg: Arc<std::sync::RwLock<Config>>) {
    std::thread::spawn(move || {
        let mut previous = String::new();
        loop {
            let fresh = build_state(&cfg);
            // A simple signature: what the menu shows, concatenated.
            // The file offer goes into the signature, and not only into
            // building the menu: the tray host CACHES the menu we push.
            // Without it the "Send files to ..." item would only show up the
            // next time something else changed -- or never, on an idle
            // machine.
            let key = format!(
                "{}|{}|{:?}|{:?}|{:?}",
                fresh.devices.join(","),
                fresh.remotes.join(","),
                fresh.here,
                fresh.target,
                crate::clip_files::offer().map(|(c, b)| (c.len(), b))
            );
            if key != previous {
                previous = key;
                update_state(fresh);
                handle.update(|_| {});
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    });
}

fn build_state(cfg: &Arc<std::sync::RwLock<Config>>) -> TrayState {
    // Survey from the cache: a fresh HID++ read costs ~360 ms, and the
    // tray is informative, not an authority. What needs freshness is the
    // trigger.

    let s = crate::hidpp::survey_cached_or_fresh(std::time::Duration::from_secs(20));
    let devices = s
        .receivers
        .iter()
        .flat_map(|r| r.devices.iter())
        .map(|d| match d.battery_percent {
            Some(p) => format!("{}: {p}%", d.name),
            None => format!("{}: ?", d.name),
        })
        .collect();

    let (remotes, here) = match cfg.read() {
        Ok(g) => {
            let local = |h: &crate::config::HostCfg| {
                h.local.unwrap_or(false)
                    || matches!(h.addr.as_str(), "127.0.0.1" | "localhost" | "::1")
            };
            (
                g.hosts.iter().filter(|h| !local(h)).map(|h| h.name.clone()).collect(),
                g.hosts.iter().find(|h| local(h)).map(|h| h.name.clone()),
            )
        }
        Err(_) => (Vec::new(), None),
    };

    TrayState { devices, remotes, here, target: crate::kvm::target() }
}
