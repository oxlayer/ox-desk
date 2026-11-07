//! Edge detection via org.gnome.Mutter.InputCapture.
//!
//! Spec: ../specs/005-input-switch.md#edge-detection-mutter-directly-not-the-portal
//!
//! We do NOT use the portal: `org.freedesktop.portal.InputCapture` accepts
//! CreateSession and never answers, because it cannot show the consent
//! dialog for a process without a window. Mutter exposes the same
//! functionality synchronously, with no dialog.
//!
//! We use ONLY the detection: on Activated we hand the pointer back right
//! away with Release. We neither capture nor forward input -- what moves
//! focus is the HID++ channel switch.

use crate::config::Config;
use crate::focus::{self, FocusRequest};
use dbus::arg::OwnedFd;
use dbus::blocking::Connection;
use dbus::channel::MatchingReceiver;
use dbus::message::MatchRule;
use dbus::Path;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const DEST: &str = "org.gnome.Mutter.InputCapture";
const OBJ: &str = "/org/gnome/Mutter/InputCapture";
const IFACE: &str = "org.gnome.Mutter.InputCapture";
const IFACE_SESSION: &str = "org.gnome.Mutter.InputCapture.Session";
const CAP_POINTER: u32 = 2;
/// Grace period after the devices arrive here.
///
/// With edges on both machines pointing at each other, without this a
/// natural movement right after arrival sends everything back at once.
/// Measured in the user's log: arrival and return in under a minute,
/// several times.
const GRACE: u64 = 7_000;

const TIMEOUT: Duration = Duration::from_secs(10);

/// How many pixels inward to hand the pointer back.
///
/// Measured: handing it back on the edge itself re-fires the barrier
/// immediately -- we reached `activation=40` with the cursor still at
/// x=2560. Release accepts `cursor_position`, so we pull back.
/// MEASURED AND DISCARDED: mutter accepts `cursor_position` in `Release`
/// and **ignores** it. In 300+ activations the cursor stayed exactly at
/// x=2560.0, and we produced 690 lines of "in cooldown". Pulling the
/// pointer back is not an available option.
///
/// The strategy that works is to DISARM the barrier (`Disable`) during the
/// cooldown and re-arm it afterwards -- with no barrier there is no event,
/// wherever the pointer is.
const _PULLBACK_DOES_NOT_WORK: () = ();

/// Dwell required at the edge before switching.
///
/// Mutter **has no** concept of dwell: the barrier fires on contact. We
/// approximate it by the DENSITY of activations: touching and insisting
/// re-fires several times in a row (we measured `activation=54` with the
/// cursor still), while a passing graze produces just one.
///
/// While collecting, `Release` goes WITHOUT pullback -- on purpose: the
/// re-firing is the signal. We only pull back once we decide.
/// Defaults; the config rules (`[input.border] hold_hits/hold_window_ms`).
/// Lowered from 3/400ms to 2/300ms: the user felt the delay, and a passing
/// graze produces a single activation -- two already show intent.
const HOLD_WINDOW_DEFAULT: Duration = Duration::from_millis(300);
const HOLD_HITS_DEFAULT: usize = 2;

/// Window in which a second crossing is ignored. The action fired is
/// IRREVERSIBLE; without this, a double bump on the edge would become two
/// switches. 10 s because the switch + effect-based verification takes
/// ~5 s: with 3 s the second event came in before the first finished.
const COOLDOWN: Duration = Duration::from_secs(10);

/// (x1, y1, x2, y2) of the barrier line for the requested edge.
///
/// The line sits on the zone's BOUNDARY. Using the last inner column/row
/// (x+w-1) makes mutter refuse with
/// "AccessDenied: Line overlaps with monitor region".
fn barrier_line(edge: &str, zone: (u32, u32, i32, i32)) -> Option<(i32, i32, i32, i32)> {
    let (w, h, x, y) = zone;
    let (w, h) = (w as i32, h as i32);
    match edge {
        "right" => Some((x + w, y, x + w, y + h)),
        "left" => Some((x, y, x, y + h)),
        "top" => Some((x, y, x + w, y)),
        "bottom" => Some((x, y + h, x + w, y + h)),
        _ => None,
    }
}

/// Hands the pointer back to the user.
///
/// Without `cursor_position`: mutter ignores that option (measured). The
/// pointer comes back where it was, which is why the churn control has to
/// be the barrier's `Disable`, not pulling the cursor back.
fn release(sp: &dbus::blocking::Proxy<'_, &Connection>) {
    let empty: std::collections::HashMap<&str, dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>> =
        std::collections::HashMap::new();
    if let Err(e) = sp.method_call::<(), _, _, _>(IFACE_SESSION, "Release", (empty,)) {
        gripe!("border: Release failed: {e}");
    }
}

struct Session {
    conn: Connection,
    path: Path<'static>,
    /// barrier id -> target host for that edge.
    targets: std::collections::HashMap<u32, String>,
    /// Kept alive on purpose: closing the fd may tear down the session.
    /// It has NOT been measured yet whether Enable works without it -- it
    /// went into the prototype to rule out a hypothesis and stayed as the
    /// known-good order.
    _eis: Option<OwnedFd>,
}

/// Opens the capture session with ONE barrier per watched edge.
///
/// Several barriers in the same session: mutter returns an id per
/// `AddBarrier` and delivers that id in the `Activated` signal, so the
/// `id -> target` map is what says where to send when that specific edge
/// is touched. It used to be just one, because the config had a single
/// `edge`; with the grid, a machine in the middle has a neighbor on both
/// sides.
fn open(edges: &[(String, String)]) -> Result<Session, String> {
    let conn = Connection::new_session().map_err(|e| format!("session bus: {e}"))?;

    let root = conn.with_proxy(DEST, OBJ, TIMEOUT);
    let (path,): (Path,) = root
        .method_call(IFACE, "CreateSession", (CAP_POINTER,))
        .map_err(|e| format!("CreateSession: {e}"))?;
    let path = path.into_static();

    let sp = conn.with_proxy(DEST, path.clone(), TIMEOUT);

    let (serial, zones): (u32, Vec<(u32, u32, i32, i32)>) = sp
        .method_call(IFACE_SESSION, "GetZones", ())
        .map_err(|e| format!("GetZones: {e}"))?;
    let zone = *zones.first().ok_or("the compositor reported no zone")?;

    let mut targets = std::collections::HashMap::new();
    for (edge, target) in edges {
        let Some(line) = barrier_line(edge, zone) else {
            gripe!("border: invalid edge '{edge}' (use left/right/top/bottom) -- ignored");
            continue;
        };
        match sp.method_call::<(u32,), _, _, _>(IFACE_SESSION, "AddBarrier", (serial, line)) {
            Ok((bid,)) => {
                say!("border: barrier {bid} on edge {edge} {line:?} -> {target}");
                targets.insert(bid, target.clone());
            }
            Err(e) => gripe!("border: AddBarrier at {line:?} failed: {e}"),
        }
    }
    if targets.is_empty() {
        return Err("no barrier could be created".into());
    }

    let eis: Option<OwnedFd> = match sp.method_call::<(OwnedFd,), _, _, _>(
        IFACE_SESSION,
        "ConnectToEIS",
        (),
    ) {
        Ok((fd,)) => Some(fd),
        Err(e) => {
            gripe!("border: ConnectToEIS failed, going on without it: {e}");
            None
        }
    };

    sp.method_call::<(), _, _, _>(IFACE_SESSION, "Enable", ())
        .map_err(|e| format!("Enable: {e}"))?;

    say!("border: {} barrier(s) in zone {zone:?}, eis={}", targets.len(),
         if eis.is_some() { "yes" } else { "no" });
    Ok(Session { conn, path, targets, _eis: eis })
}

/// Starts edge watching on a thread. An error here does not bring the
/// daemon down: the rest of logid stays useful without detection.
pub fn spawn(cfg: Arc<RwLock<Config>>) {
    std::thread::spawn(move || {
        let (edges, hold_hits, hold_window) = {
            let Ok(g) = cfg.read() else { return };
            let b = &g.input.border;
            let hits = if b.hold_hits == 0 { HOLD_HITS_DEFAULT } else { b.hold_hits };
            let win = if b.hold_window_ms == 0 {
                HOLD_WINDOW_DEFAULT
            } else {
                Duration::from_millis(b.hold_window_ms)
            };
            // Comes from the grid when there is one; otherwise from the
            // hand-written `edge`/`target`. See `Config::border_edges`.
            let edges = g.border_edges();
            if edges.is_empty() {
                return;
            }
            (edges, hits, win)
        };

        let sess = match open(&edges) {
            Ok(s) => s,
            Err(e) => {
                gripe!("border: off -- {e}");
                return;
            }
        };

        let last = Arc::new(RwLock::new(None::<Instant>));
        let hits = Arc::new(RwLock::new(Vec::<Instant>::new()));
        // When to re-arm the barrier. The main loop is the one that calls
        // Enable: the session belongs to THIS D-Bus connection, no other
        // controls it.
        let reenable = Arc::new(RwLock::new(None::<Instant>));
        // Shared: the handler is what DISABLES on firing, and the loop is
        // what re-enables. Without this the loop thought the barrier was
        // still armed and tried to disable it again on every tick --
        // `Disable failed: Session not enabled`, twice a second.
        let armed = Arc::new(RwLock::new(true));
        let rule = {
            let mut r = MatchRule::new_signal(IFACE_SESSION, "Activated");
            r.path = Some(sess.path.clone());
            r
        };

        let cfg2 = Arc::clone(&cfg);
        let path2 = sess.path.clone();
        let targets = sess.targets.clone();
        let last2 = Arc::clone(&last);
        let hits2 = Arc::clone(&hits);
        let reenable2 = Arc::clone(&reenable);
        let armed2 = Arc::clone(&armed);

        let res = sess.conn.add_match(
            rule,
            move |(barrier_id, activation_id, cursor): (u32, u32, (f64, f64)), conn, _msg| {
                say!(
                    "border: ACTIVATED barrier={barrier_id} activation={activation_id} cursor={cursor:?}"
                );

                let sp = conn.with_proxy(DEST, path2.clone(), TIMEOUT);
                release(&sp);

                // Guard: with the barrier disarmed no event should arrive.
                if last2.read().ok().and_then(|g| *g).is_some_and(|t| t.elapsed() < COOLDOWN) {
                    return true;
                }

                // Post-arrival grace. Mutter only emits on crossing, so
                // there is no still cursor firing here -- but arriving and
                // crossing straight back is exactly the cycle the user
                // saw.
                if crate::hidpp::arrived_ms_ago().is_some_and(|ms| ms < GRACE) {
                    say!("border: devices just arrived -- ignoring the crossing");
                    return true;
                }

                // Dwell by activation density.
                let hits = {
                    let Ok(mut v) = hits2.write() else { return true };
                    let now = Instant::now();
                    v.retain(|t: &Instant| now.duration_since(*t) < hold_window);
                    v.push(now);
                    v.len()
                };
                if hits < hold_hits {
                    say!("border: dwell {hits}/{hold_hits}, waiting");
                    return true;
                }

                // Decided. DISARM the barrier: it is the only way to stop
                // the churn, since mutter ignores the cursor pullback.
                match sp.method_call::<(), _, _, _>(IFACE_SESSION, "Disable", ()) {
                    Ok(()) => {
                        if let Ok(mut g) = armed2.write() {
                            *g = false;
                        }
                    }
                    Err(e) => gripe!("border: Disable failed: {e}"),
                }
                if let Ok(mut v) = hits2.write() {
                    v.clear();
                }
                let now = Instant::now();
                if let Ok(mut g) = last2.write() {
                    *g = Some(now);
                }
                if let Ok(mut g) = reenable2.write() {
                    *g = Some(now + COOLDOWN);
                }
                say!("border: barrier disarmed; switching and re-arming in {}s", COOLDOWN.as_secs());

                // The switch takes seconds (solaar + effect-based check).
                // Running it here blocked the D-Bus loop: measured, 35 s
                // without processing any signal. It goes to a thread.
                let Ok(snap) = cfg2.read().map(|g| g.clone()) else { return true };
                // Switch read HERE, not at boot.
                //
                // The barriers are created once, with the edges the config
                // had at boot. Without this line, turning the edge off in
                // the UI turned nothing off: the thread stayed alive and
                // kept switching machines on every touch, while the screen
                // showed the switch as "off". The user could not work
                // because of it -- a switch that lies is worse than none.
                if !snap.input.border.enabled {
                    return true;
                }
                // WHICH edge was touched decides where it goes. With the
                // grid, a machine in the middle has a neighbor on both sides.
                let Some(target3) = targets.get(&barrier_id).cloned() else {
                    gripe!("border: barrier {barrier_id} has no known target");
                    return true;
                };
                std::thread::spawn(move || {
                    // The full pre-flight runs in here: a bad config must
                    // not leave you without a keyboard just because the
                    // mouse touched the edge.
                    let req = FocusRequest {
                        host: target3,
                        route: None,
                        confirm: true,
                        // An edge crossing does not wait on SSH: it reads
                        // the peer cache. That was ~2.2 s of a 3.9 s switch.
                        fast: true,
                    };
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
                true
            },
        );
        if let Err(e) = res {
            gripe!("border: add_match failed: {e}");
            return;
        }

        // Diagnostics: a bus match for ALL signals of the session
        // interface, plus a local receiver that logs whatever arrives.
        // It tells "mutter did not emit" from "our matcher did not match"
        // -- an ambiguity that cost an inconclusive test.
        if let Err(e) = sess
            .conn
            .add_match_no_cb(&format!("type='signal',interface='{IFACE_SESSION}'"))
        {
            gripe!("border: diagnostic match failed: {e}");
        }
        sess.conn.start_receive(
            MatchRule::new(),
            Box::new(|msg, _| {
                if msg
                    .interface()
                    .is_some_and(|i| i.contains("InputCapture"))
                {
                    say!(
                        "border[signal] {}.{}",
                        msg.interface().map(|i| i.to_string()).unwrap_or_default(),
                        msg.member().map(|m| m.to_string()).unwrap_or_default()
                    );
                }
                true
            }),
        );

        let summary: Vec<String> =
            edges.iter().map(|(e, t)| format!("{e} -> {t}")).collect();
        say!(
            "border: watching {} (dwell {hold_hits} in {}ms)",
            summary.join(", "),
            hold_window.as_millis()
        );
        let mut ticks: u64 = 0;
        loop {
            if let Err(e) = sess.conn.process(Duration::from_millis(500)) {
                gripe!("border: bus loop died: {e}");
                return;
            }
            ticks += 1;

            // Armed or not -- decided here, because the session belongs to
            // this D-Bus connection.
            //
            // The rule is: an edge only makes sense on the machine that HAS
            // the devices. This used to re-arm blindly 10 s after each
            // firing, and the damage was large: mutter does not pull the
            // cursor back, so it stays at x=2560 after the switch, and each
            // re-arm fired on its own over the resting cursor. That pushed
            // again (sometimes only one of the two, splitting the state and
            // waking the reconciler) and burned the cooldown -- and with the
            // barrier disabled during those 10 s, hitting the edge on
            // purpose did nothing. The user described both symptoms: "it
            // comes back on its own" and "it gets stuck and won't go no
            // matter what".
            let cooldown_over = reenable
                .read()
                .ok()
                .and_then(|g| *g)
                .is_none_or(|t| Instant::now() >= t);
            let just_arrived =
                crate::hidpp::arrived_ms_ago().is_some_and(|ms| ms < GRACE);
            let want_armed =
                cooldown_over && !just_arrived && crate::hidpp::devices_here();

            let is_armed = armed.read().map(|g| *g).unwrap_or(false);
            if want_armed != is_armed {
                let sp = sess.conn.with_proxy(DEST, sess.path.clone(), TIMEOUT);
                let method = if want_armed { "Enable" } else { "Disable" };
                match sp.method_call::<(), _, _, _>(IFACE_SESSION, method, ()) {
                    Ok(()) => {
                        if let Ok(mut g) = armed.write() {
                            *g = want_armed;
                        }
                        if want_armed {
                            say!("border: armed -- the devices are here");
                            if let Ok(mut g) = reenable.write() {
                                *g = None;
                            }
                        } else {
                            say!("border: disarmed -- the devices are not here");
                        }
                    }
                    Err(e) => {
                        // The compositor is the truth: if it says it is
                        // already in the requested state, we note it and stop
                        // insisting -- that was what filled the journal.
                        let text = e.to_string();
                        if text.contains("not enabled") || text.contains("already") {
                            if let Ok(mut g) = armed.write() {
                                *g = want_armed;
                            }
                        } else {
                            gripe!("border: {method} failed: {e}");
                        }
                    }
                }
            }

            // Proof of life every ~60 s: without it, silence in the journal
            // is ambiguous between "nothing happened" and "the thread died".
            if ticks % 120 == 0 {
                say!(
                    "border: alive, {} min watching {} edge(s), {}",
                    ticks / 120,
                    edges.len(),
                    if is_armed { "armed" } else { "disarmed (devices away)" }
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2560x1440 zone at the origin, like this machine's.
    const Z: (u32, u32, i32, i32) = (2560, 1440, 0, 0);

    #[test]
    fn barrier_sits_on_the_boundary_not_inside() {
        // x+w, not x+w-1: with 2559 mutter refuses with
        // "AccessDenied: Line overlaps with monitor region".
        assert_eq!(barrier_line("right", Z), Some((2560, 0, 2560, 1440)));
        assert_eq!(barrier_line("left", Z), Some((0, 0, 0, 1440)));
        assert_eq!(barrier_line("top", Z), Some((0, 0, 2560, 0)));
        assert_eq!(barrier_line("bottom", Z), Some((0, 1440, 2560, 1440)));
    }

    #[test]
    fn barrier_honors_zone_offset() {
        // Secondary monitor starting at x=2560.
        let z = (1920, 1080, 2560, 0);
        assert_eq!(barrier_line("right", z), Some((4480, 0, 4480, 1080)));
        assert_eq!(barrier_line("left", z), Some((2560, 0, 2560, 1080)));
    }

    #[test]
    fn invalid_edge_is_refused() {
        assert_eq!(barrier_line("diagonal", Z), None);
        assert_eq!(barrier_line("", Z), None);
    }
}
