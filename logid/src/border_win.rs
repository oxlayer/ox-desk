//! Edge watching on Windows: polling `GetCursorPos`.
//!
//! It used to be the `watch-edge` mode of `logid-agent`, a separate
//! process launched by a shortcut in `shell:startup`. It became a daemon
//! thread when `logid` started running on Windows -- with both alive,
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
//! The translation is: `hold_hits <= 1` means switch on first touch (what
//! the user asked for), and above that we require staying on the edge for
//! `hold_window_ms`. Same intent, different signals.

use crate::config::Config;
use crate::focus::{self, FocusRequest};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(30);
const COOLDOWN: Duration = Duration::from_secs(10);
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

fn run(cfg: Arc<RwLock<Config>>) {
    let (edges, hold) = {
        let Ok(g) = cfg.read() else { return };
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
                if !ok {
                    gripe!("border: invalid edge '{e}' -- ignored");
                }
                ok
            })
            .collect();
        if edges.is_empty() {
            return;
        }
        (edges, hold)
    };

    // `GetCursorPos` and `GetLastInputInfo` are PER SESSION. In session 0
    // -- the case of launching the daemon over SSH -- this thread would see
    // no input at all, would read a fake 1024x768 desktop and would still
    // compete for the receiver with the real instance. The rest of the
    // daemon (API, HID++) works there, only the edge does not: that is why
    // only this thread gives up.
    if !winput::in_console_session() {
        gripe!(
            "border: outside the console session -- cursor and input are not \
             visible from here. The daemon keeps serving the API; the edge does not."
        );
        return;
    }
    if !winput::claim_single_instance("logid-agent-watch-edge") {
        gripe!(
            "border: another watcher already exists (the daemon or the old \
             watch-edge shortcut). Two owners of the same receiver is a real \
             problem -- this thread gives up."
        );
        return;
    }

    let (vx, vy, vw, vh) = winput::virtual_screen();
    let summary: Vec<String> = edges.iter().map(|(e, t)| format!("{e} -> {t}")).collect();
    say!(
        "border: watching {} (area {vx},{vy} {vw}x{vh}, dwell {}ms)",
        summary.join(", "),
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
        .map(|(e, _)| winput::cursor().map(|p| winput::at_edge(e, p)).unwrap_or(false))
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
    //   08:33:06 border: edge 'left' reached -- switching to ubuntu-host
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

    loop {
        std::thread::sleep(TICK);

        let enabled = cfg.read().map(|g| g.input.border.enabled).unwrap_or(false);
        if !enabled {
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

        if last_presence.elapsed() >= PRESENCE_EVERY {
            last_presence = Instant::now();
            guard.tick(&cfg);
        }

        // An edge only makes sense on the machine that HAS the devices,
        // and only after the person has had time to leave the border they
        // came in through. Outside that it is not neutral: it fires, tries
        // to switch what it does not hold and fills the log -- and on
        // Linux it also burned the cooldown, leaving the edge dead just
        // when the user hit it on purpose.
        let just_arrived = crate::hidpp::arrived_ms_ago().is_some_and(|ms| ms < GRACE);
        if just_arrived || !crate::hidpp::devices_here() {
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

        let Some(p) = winput::cursor() else { continue };
        // Which edge is the cursor touching? None or one -- never two,
        // because they are opposite sides of the same area.
        let Some(i) = edges.iter().position(|(e, _)| winput::at_edge(e, p)) else {
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
        cooldown = Some(Instant::now() + COOLDOWN);
        say!("border: edge '{edge}' reached at ({},{}) -- switching to {target}", p.x, p.y);
        let dest = target.clone();
        switch_away(&cfg, &dest);
    }
}

/// The switch takes seconds (pre-flight + effect-based check). Running it
/// in the loop would stall the cursor reading; it goes to a thread.
fn switch_away(cfg: &Arc<RwLock<Config>>, target: &str) {
    let Ok(snap) = cfg.read().map(|g| g.clone()) else { return };
    let host = target.to_string();
    std::thread::spawn(move || {
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
        let secs = std::env::var("LOGID_GUARD_SECS")
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
