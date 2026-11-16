// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Edge watching on Windows: polling `GetCursorPos`.
//!
//! It used to be the `watch-edge` mode of `oxdesk-agent`, a separate
//! process launched by a shortcut in `shell:startup`. It became a daemon
//! thread when `oxdesk` started running on Windows -- with both alive,
//! there would be two owners of the same HID++ receiver, exactly what the
//! single-instance lock exists to prevent (it already happened: three
//! simultaneous `watch-edge` tripling the radio traffic).
//!
//! That is why this thread claims the **same mutex name** as
//! `watch-edge`. Not a coincidence: it is what guarantees the old shortcut
//! and the new daemon never poll together, even if someone forgets to
//! remove the shortcut.
//!
//! ## Why it is simpler here than on Linux
//!
//! GNOME does not let you read the cursor position: you have to ask the
//! compositor for a barrier via `org.gnome.Mutter.InputCapture`, receive a
//! signal, and disarm the barrier by hand because mutter ignores the
//! cursor pullback. On Windows `GetCursorPos` answers directly -- a 30 ms
//! loop and the cursor is read as it is.
//!
//! ## Dwell is expressed differently on the two ends
//!
//! Mutter delivers discrete events, so on the Linux side dwell is
//! **density**: `hold_hits` activations within `hold_window_ms`. Here we
//! measure **continuous** contact, which is what polling actually sees.
//! The translation is: `hold_hits <= 1` means switch on first touch (the
//! reference deployment's choice), and above that we require staying on the edge for
//! `hold_window_ms`. Same intent, different signals.

use crate::config::Config;
use crate::focus::{self, FocusRequest};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(30);
const COOLDOWN: Duration = Duration::from_secs(10);
const LAN_COOLDOWN: Duration = Duration::from_millis(1500);
/// How close to an edge the cursor gets before the other side's screen
/// host is woken, so that reaching the edge costs no round trip.
const APPROACH: i32 = 150;
/// Presence is HID++: at 33 Hz it would hammer the radio for nothing. The
/// dead-man's switch window is tens of seconds, 1 Hz is enough.
const PRESENCE_EVERY: Duration = Duration::from_secs(1);
const GUARD_DEFAULT: u64 = 25;
/// Grace period after the devices arrive here.
///
/// With edges on both machines pointing at each other, without this a
/// natural movement right after arrival sends everything back at once.
/// Measured in the user's log: arrival and return in under a minute,
/// several times.
const GRACE: u64 = 7_000;


pub fn spawn(cfg: Arc<RwLock<Config>>) {
    std::thread::spawn(move || run(cfg));
}

/// The edges to watch and the dwell, or `None` while there is nothing to
/// watch (the edge is off, or no neighbor can take the input).
fn read_edges(cfg: &Arc<RwLock<Config>>, complain: bool) -> Option<(Vec<(String, String)>, Duration)> {
    let g = cfg.read().ok()?;
    let b = &g.input.border;
    let hold = if b.hold_hits <= 1 {
        Duration::ZERO
    } else {
        Duration::from_millis(if b.hold_window_ms == 0 { 300 } else { b.hold_window_ms })
    };
    // Comes from the grid when there is one (see `Config::border_edges`).
    // A machine in the middle has a neighbor on both sides, so these
    // are several edges, not one.
    let edges: Vec<(String, String)> = g
        .border_edges()
        .into_iter()
        .filter(|(e, _)| {
            let ok = matches!(e.as_str(), "left" | "right" | "top" | "bottom");
            if !ok && complain {
                gripe!("border: invalid edge '{e}' -- ignored");
            }
            ok
        })
        .collect();
    if edges.is_empty() {
        crate::edge_state::set(if b.enabled { "no-neighbor" } else { "off" }, None);
        return None;
    }
    Some((edges, hold))
}

fn run(cfg: Arc<RwLock<Config>>) {
    // Wait for an edge instead of quitting without one.
    //
    // This thread used to return at boot when the edge was off, and
    // turning it on from the panel then saved the config and changed
    // nothing: the log said "border KVM ON from the GUI" at 20:39 and
    // "watching" only at 20:43, after a restart nobody had been told to do.
    // The panel's note even said to restart, which is a bug asking the
    // person to work around it.
    let mut said = false;
    let (edges, hold) = loop {
        if let Some(v) = read_edges(&cfg, true) {
            break v;
        }
        if !said {
            say!("border: nothing to watch yet (edge off, or no neighbor in the layout) -- it starts when there is");
            said = true;
        }
        std::thread::sleep(Duration::from_secs(1));
    };

    // `GetCursorPos` and `GetLastInputInfo` are PER SESSION. In session 0
    // -- the case of launching the daemon over SSH -- this thread would see
    // no input at all, would read a fake 1024x768 desktop and would still
    // compete for the receiver with the real instance. The rest of the
    // daemon (API, HID++) works there, only the edge does not: that is why
    // only this thread gives up.
    if !winput::in_console_session() {
        crate::edge_state::set("unavailable", Some("session".into()));
        gripe!(
            "border: outside the console session -- cursor and input are not \
             visible from here. The daemon keeps serving the API; the edge does not."
        );
        return;
    }
    if !winput::claim_single_instance("oxdesk-agent-watch-edge") {
        crate::edge_state::set("unavailable", Some("another-watcher".into()));
        gripe!(
            "border: another watcher already exists (the daemon or the old \
             watch-edge shortcut). Two owners of the same receiver is a real \
             problem -- this thread gives up."
        );
        return;
    }

    let (vx, vy, vw, vh) = winput::virtual_screen();
    let watching = crate::edge_state::summary(&edges);
    // The monitors, re-read every second: a monitor plugged in, unplugged
    // or rearranged changes where the edges are.
    let mut monitors = winput::monitors();
    let mut monitors_at = Instant::now();
    say!(
        "border: watching {watching} (area {vx},{vy} {vw}x{vh}, {} monitor(s), dwell {}ms)",
        monitors.len(),
        hold.as_millis()
    );

    let mut touching: Option<Instant> = None;
    let mut cooldown: Option<Instant> = None;
    let mut guard = Guard::new(&cfg);
    let mut last_presence = Instant::now() - PRESENCE_EVERY;
    // Already touching at start? Then it does not count as an arrival.
    // Without this, a cursor parked on the edge fires on the first tick
    // after boot. Per edge, because there are several.
    let mut armed = true;
    let mut was_at: Vec<bool> = edges
        .iter()
        .map(|(e, _)| winput::cursor().map(|p| on_edge(e, p, &monitors)).unwrap_or(false))
        .collect();
    if was_at.iter().any(|v| *v) {
        say!("border: cursor is already on a watched edge -- waiting for it to leave and come back");
    }

    // Read on every turn, not at boot.
    //
    // This loop is born with the edges the config had at the start and
    // runs forever; without this read, turning the edge off in the GUI
    // turned NOTHING off here. The log proved the damage, two seconds
    // after I turned it off through the API:
    //
    //   08:33:04 border: edge KVM turned OFF from the GUI
    //   08:33:04 border: armed -- the devices are here
    //   08:33:06 border: edge 'left' reached -- switching to linux-pc
    //
    // The user sent the devices to this machine, moved the mouse to the
    // left -- the natural movement, because Ubuntu is on that side -- and
    // everything went back. Ubuntu's panel said, rightly, that ITS edge
    // was off; this was another one.
    //
    // It applies to the dead-man's switch too (`guard.tick`): it exists to
    // undo an edge crossing when nobody showed up on the other side.
    // Without an edge, it has nothing to undo -- it only pulls.
    let mut off_announced = false;
    let mut touched_while_away = false;

    loop {
        std::thread::sleep(TICK);

        let enabled = cfg.read().map(|g| g.input.border.enabled).unwrap_or(false);
        if !enabled {
            crate::edge_state::set("off", None);
            if armed || !off_announced {
                armed = false;
                off_announced = true;
                say!("border: off in the config -- not watching or returning anything");
            }
            // Reset arrival on the edges: if it is turned back on, the
            // cursor will have to LEAVE an edge and come back before it counts.
            was_at.iter_mut().for_each(|v| *v = true);
            touching = None;
            cooldown = None;
            continue;
        }
        off_announced = false;

        if let Some(t) = cooldown {
            if Instant::now() < t {
                continue;
            }
            cooldown = None;
            say!("border: cooldown over, watching again");
        }

        // The input is already away (the LAN KVM is forwarding it): the
        // grab parks the cursor, and nothing here should fire.
        if let Some(t) = crate::kvm::target() {
            // The input should be away, so the cursor here is parked by the
            // grab. If it is on a watched edge anyway, the person is using
            // this machine while the daemon believes otherwise: say so,
            // once per session, because that is a stop with no other trace.
            if !touched_while_away {
                if let Some(p) = winput::cursor() {
                    if edges.iter().any(|(e, _)| on_edge(e, p, &monitors)) {
                        touched_while_away = true;
                        gripe!("border: edge touched while the input is on {t} -- the KVM is still running");
                    }
                }
            }
            crate::edge_state::set("sent", Some(t));
            touching = None;
            continue;
        }
        touched_while_away = false;

        // The dead-man's switch belongs to the HID++ route only: it undoes
        // a channel switch nobody answered. In LAN mode the devices never
        // leave this receiver, so an "arrival" is a device waking up, and
        // "giving it back" would be a channel switch to a receiver that
        // may not exist (spec 024).
        if last_presence.elapsed() >= PRESENCE_EVERY {
            last_presence = Instant::now();
            if lan_mode(&cfg) {
                guard.disarm();
            } else {
                guard.tick(&cfg);
            }
        }

        // An edge only makes sense on the machine that HAS the devices,
        // and only after the person has had time to leave the border they
        // came in through. Outside that it is not neutral: it fires, tries
        // to switch what it does not hold and fills the log -- and on
        // Linux it also burned the cooldown, leaving the edge dead just
        // when the user hit it on purpose.
        let just_arrived = crate::hidpp::arrived_ms_ago().is_some_and(|ms| ms < GRACE);
        if just_arrived || !crate::hidpp::devices_here() {
            crate::edge_state::set(if just_arrived { "arriving" } else { "away" }, None);
            if armed {
                armed = false;
                say!(
                    "border: disarmed -- {}",
                    if just_arrived { "they just arrived" } else { "they are not here" }
                );
            }
            // Reset arrival on the edges: when re-armed, the cursor will
            // have to LEAVE and come back, even if it is already touching.
            was_at.iter_mut().for_each(|v| *v = true);
            touching = None;
            continue;
        }
        if !armed {
            armed = true;
            say!("border: armed -- the devices are here");
        }
        crate::edge_state::set("watching", Some(watching.clone()));

        if monitors_at.elapsed() >= Duration::from_secs(1) {
            monitors = winput::monitors();
            monitors_at = Instant::now();
        }
        let Some(p) = winput::cursor() else { continue };
        // On its way to an edge: wake the other side now (`kvm::prewarm`).
        // It is a no-op while that host is known up.
        if lan_mode(&cfg) {
            for (e, t) in &edges {
                if edge_distance(e, p, &monitors).is_some_and(|d| d < APPROACH) {
                    crate::kvm::prewarm(&cfg, t);
                }
            }
        }
        // Which edge is the cursor touching? None or one -- never two,
        // because they are opposite sides of a monitor.
        let Some(i) = edges.iter().position(|(e, _)| on_edge(e, p, &monitors)) else {
            touching = None;
            was_at.iter_mut().for_each(|v| *v = false);
            continue;
        };
        let (edge, target) = (&edges[i].0, &edges[i].1);
        let was_at_edge = was_at[i];

        // Requires ARRIVAL, not presence.
        //
        // Mutter delivers crossing as an event, so on the Linux side this
        // comes for free. Here polling reads position: a cursor resting on
        // the edge would look "touching" every 30 ms, forever. The cost of
        // that is not a repeated log -- it is that, the instant the devices
        // ARRIVED on this machine, the edge would send them back without
        // anyone having touched anything.
        //
        // After firing, `was_at_edge` stays `true`: it only counts again
        // when the cursor leaves the edge and comes back.
        if was_at_edge && touching.is_none() {
            continue;
        }
        if !was_at_edge {
            was_at[i] = true;
            touching = Some(Instant::now());
        }
        let since = touching.unwrap_or_else(Instant::now);
        if since.elapsed() < hold {
            continue;
        }
        touching = None;
        // The long cooldown is the HID++ route's: a channel switch takes
        // seconds and must not ping-pong. The LAN KVM is instant both ways,
        // and with the way home by the other screen's edge, going, coming
        // back and going again within ten seconds is normal use.
        cooldown = Some(Instant::now() + if lan_mode(&cfg) { LAN_COOLDOWN } else { COOLDOWN });
        say!("border: edge '{edge}' reached at ({},{}) -- switching to {target}", p.x, p.y);
        let dest = target.clone();
        switch_away(&cfg, &dest);
    }
}

/// How far the cursor is from an outer edge in that direction, per monitor
/// (`edge_state::monitor_edge_distance`); the box around all monitors only
/// when Windows could not list them.
fn edge_distance(edge: &str, p: winput::Point, monitors: &[(i32, i32, i32, i32)]) -> Option<i32> {
    if monitors.is_empty() {
        return crate::edge_state::distance_to_edge(edge, (p.x, p.y), winput::virtual_screen());
    }
    crate::edge_state::monitor_edge_distance(edge, (p.x, p.y), monitors)
}

fn on_edge(edge: &str, p: winput::Point, monitors: &[(i32, i32, i32, i32)]) -> bool {
    edge_distance(edge, p, monitors) == Some(0)
}

/// Is the input moved over the network (`[input] mode = "lan"`, the
/// default) rather than by switching the devices' radio channel?
fn lan_mode(cfg: &Arc<RwLock<Config>>) -> bool {
    cfg.read().map(|g| g.input.mode != "hidpp").unwrap_or(true)
}

/// The switch takes seconds (pre-flight + effect-based check). Running it
/// in the loop would stall the cursor reading; it goes to a thread.
///
/// The route follows `[input] mode`. In LAN mode (the default) the edge
/// starts the LAN KVM, the same door the panel and the tray use. It used
/// to switch the HID++ channel whatever the mode said, and with the only
/// receiver on this machine that sends the keyboard and mouse to a
/// channel nobody listens on: the pre-flight's dead-man's switch check was
/// the one thing that stopped it (spec 024).
fn switch_away(cfg: &Arc<RwLock<Config>>, target: &str) {
    let Ok(snap) = cfg.read().map(|g| g.clone()) else { return };
    let host = target.to_string();
    std::thread::spawn(move || {
        if snap.input.mode != "hidpp" {
            match crate::kvm::send(&snap, &host) {
                Ok(msg) => say!("border: {msg}"),
                Err(e) => say!("border: did NOT switch -- {e}"),
            }
            return;
        }
        // The full pre-flight runs in here: a bad config must not leave
        // you without a keyboard just because the mouse touched the edge.
        let req = FocusRequest { host, route: None, confirm: true, fast: true };
        let outcome = focus::execute(&snap, &req);
        if outcome.executed {
            for r in &outcome.results {
                say!("border: {r}");
            }
        } else {
            let blockers: Vec<&str> = outcome
                .plan
                .checks
                .iter()
                .filter(|c| c.blocking && !c.ok)
                .map(|c| c.name.as_str())
                .collect();
            say!(
                "border: did NOT switch -- {} (blockers: {})",
                outcome.error.unwrap_or_else(|| "plan not ready".into()),
                blockers.join(", ")
            );
        }
    });
}

/// Dead-man's switch: whoever holds the device is the only one who can
/// give it back.
///
/// Spec 005 requires this and for a long time it did not really exist: I
/// tried launching the guard over SSH and `GetLastInputInfo` does not see
/// the user's session from outside it. In here it works -- this thread
/// already lives in the session and already polls.
///
/// Logic: when the devices ARRIVE on this machine, we open a window. If no
/// input shows up in it, we give them back.
struct Guard {
    secs: u64,
    peer: Option<String>,
    present_before: bool,
    arrived: Option<Instant>,
}

impl Guard {
    fn new(cfg: &Arc<RwLock<Config>>) -> Self {
        let secs = std::env::var("OXDESK_GUARD_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(GUARD_DEFAULT);
        let peer = cfg.read().ok().and_then(|g| g.peer_host().map(|h| h.name.clone()));
        if peer.is_none() {
            gripe!(
                "border: no remote host in the config -- the dead-man's switch \
                 does not know where to give the devices back"
            );
        }
        Guard { secs, peer, present_before: false, arrived: None }
    }

    /// Forgets an arrival in progress, without acting on it.
    fn disarm(&mut self) {
        self.arrived = None;
    }

    fn tick(&mut self, cfg: &Arc<RwLock<Config>>) {
        let Some(peer) = self.peer.clone() else { return };
        let wanted: Vec<String> = match cfg.read() {
            Ok(g) => g.devices.iter().map(|d| d.name.clone()).collect(),
            Err(_) => return,
        };
        if wanted.is_empty() {
            return;
        }

        // Reuses the daemon's survey instead of reopening the receiver:
        // the refresher keeps it fresh and the cache avoids double HID++
        // traffic.
        let survey = crate::hidpp::survey_cached_or_fresh(Duration::from_millis(2500));
        let present = survey.receivers.iter().any(|r| {
            r.devices.iter().any(|d| d.online && wanted.iter().any(|w| w == &d.name))
        });

        if present && !self.present_before {
            self.arrived = Some(Instant::now());
            say!(
                "border: devices ARRIVED -- watching input for {}s",
                self.secs
            );
        } else if !present && self.present_before {
            self.arrived = None;
            say!("border: devices LEFT -- watch ended");
        }
        self.present_before = present;

        let Some(t0) = self.arrived else { return };
        let Ok(idle) = winput::idle_ms() else { return };
        // New input: idle dropped below the time since arrival, so there
        // was activity after it.
        if idle < t0.elapsed().as_millis() as u64 {
            say!("border: input detected -- dead-man's switch stood down");
            self.arrived = None;
            return;
        }
        if t0.elapsed().as_secs() >= self.secs {
            say!(
                "border: NO INPUT in {}s (idle {idle}ms) -- giving back to {peer}",
                self.secs
            );
            self.arrived = None;
            switch_away(cfg, &peer);
        }
    }
}
