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
//! - **It talked to `logid` over HTTP** to read state and send focus.
//!   Here the tray IS `logid`: it calls `kvm::mandar` directly, without
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

pub struct LogidTray {
    cfg: Arc<std::sync::RwLock<Config>>,
}

impl ksni::Tray for LogidTray {
    fn id(&self) -> String {
        "logid".into()
    }

    fn title(&self) -> String {
        "logid".into()
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
            "sem dispositivos".to_string()
        } else {
            f.devices.join(" | ")
        };
        let location = match &f.target {
            Some(a) => format!("input em {a}"),
            None => "input aqui".into(),
        };
        ksni::ToolTip {
            title: "logid".into(),
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
                    label: "nenhum dispositivo conhecido".into(),
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
                        label: format!("{}Mandar para {name}", if active { "● " } else { "" }),
                        activate: Box::new(move |b: &mut LogidTray| b.send_to(&target)),
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
                let label = format!("Trazer para {here}");
                items.push(
                    StandardItem {
                        label: label,
                        activate: Box::new(move |b: &mut LogidTray| b.send_to(&name)),
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
        // destination at a time. See [`crate::clip_arq::ao_copiar`].
        if let Some((paths, bytes)) = crate::clip_arq::oferta() {
            items.push(MenuItem::Separator);
            items.push(
                StandardItem {
                    label: format!(
                        "{} arquivo(s) copiado(s), {}",
                        paths.len(),
                        crate::clip_arq::humano(bytes)
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
                        label: format!("Mandar arquivos para {name}"),
                        activate: Box::new(move |b: &mut LogidTray| b.send_files(&target)),
                        ..Default::default()
                    }
                    .into(),
                );
            }
        }

        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Abrir painel".into(),
                activate: Box::new(|b: &mut LogidTray| b.open_panel()),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Configurar esta máquina".into(),
                activate: Box::new(|b: &mut LogidTray| b.open_route("/setup")),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

impl LogidTray {
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
        std::thread::spawn(move || match crate::clip_arq::mandar_oferta(&snap, &target) {
            Ok(m) => crate::say!("tray: {m}"),
            Err(e) => crate::say!("tray: {e}"),
        });
    }

    fn send_to(&self, host: &str) {
        let snap = match self.cfg.read() {
            Ok(g) => g.clone(),
            Err(_) => return,
        };
        // Directly, without going through HTTP: the tray IS logid.
        let r = if snap.input.mode == "lan" {
            crate::kvm::mandar(&snap, host).map(|d| format!("input to {host} ({d})"))
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
/// `logid` down because there is nowhere to draw an icon would trade an
/// inconvenience for an outage.
pub fn start(cfg: Arc<std::sync::RwLock<Config>>) -> Option<ksni::blocking::Handle<LogidTray>> {
    use ksni::blocking::TrayMethods;
    let b = LogidTray { cfg };
    match b.spawn() {
        Ok(h) => {
            crate::say!("native tray up");
            Some(h)
        }
        Err(e) => {
            crate::say!("native tray did not come up ({e}); going on without it");
            None
        }
    }
}

/// Keeps the menu fresh on its own thread.
///
/// The snapshot is rebuilt every 5 s and only pushed to the tray when it
/// CHANGES. Calling `update` on every loop would make the shell redraw
/// the menu for no reason -- and, on an open menu, that flickers in the
/// face of whoever is choosing.
pub fn watch(handle: ksni::blocking::Handle<LogidTray>, cfg: Arc<std::sync::RwLock<Config>>) {
    std::thread::spawn(move || {
        let mut previous = String::new();
        loop {
            let fresh = build_state(&cfg);
            // A simple signature: what the menu shows, concatenated.
            // The file offer goes into the signature, and not only into
            // building the menu: the tray host CACHES the menu we push.
            // Without it the "Mandar arquivos" item would only show up the
            // next time something else changed -- or never, on an idle
            // machine.
            let key = format!(
                "{}|{}|{:?}|{:?}|{:?}",
                fresh.devices.join(","),
                fresh.remotes.join(","),
                fresh.here,
                fresh.target,
                crate::clip_arq::oferta().map(|(c, b)| (c.len(), b))
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

    TrayState { devices, remotes, here, target: crate::kvm::alvo() }
}
