//! Reconciler: the devices follow the leader's channel.
//!
//! Spec: ../specs/005-input-switch.md
//!
//! Solves the split state at the root -- mouse on one machine, keyboard on
//! another -- and works even when the switch was **manual**, on the
//! Easy-Switch button, which the edge watcher does not see.
//!
//! The hardware asymmetry drives the design: only whoever holds the device
//! can push it. So:
//!
//! | where the follower is | who acts |
//! |---|---|
//! | here | we do, over local HID++ |
//! | on a peer | we ask its agent, over SSH |
//! | on neither | nothing to do (unknown channel, e.g. MacBook) |
//!
//! ## 04/11: why it now CHECKS and GIVES UP
//!
//! This module left the user unable to work. The log:
//!
//! ```text
//! 08:18:30  MX Anywhere 3: asked windows-host to send it to channel 1
//! 08:18:56  ... to channel 1
//! 08:19:17  ... to channel 1      (seven times in two minutes)
//! ```
//!
//! Two failures added up, and the second is the serious one:
//!
//! 1. `hidpp::set_host` returns `Ok` when the device does not answer --
//!    and there no answer really is the normal case (the radio drops
//!    before the reply). So "asked the peer" NEVER meant "it went".
//! 2. It did not check the result and had no limit. Asking the same thing
//!    seven times is not persistence, it is a loop -- and a loop that
//!    touches someone's keyboard is worse than an error.
//!
//! Now: after acting it CHECKS whether the device moved, and three
//! attempts with no effect **turn the reconciler off** and say so in the
//! log and in the UI. A loud error is better than a silent retry -- the
//! same lesson as langfuse and `backfill_gate` in the other repo.

use crate::agent;
use crate::config::Config;
use crate::focus;
use crate::hidpp;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Wait after acting, so as not to fight the edge or itself.
const AFTER_ACTION: Duration = Duration::from_secs(8);

/// Grace period after the devices arrive here: do not reconcile over
/// state in transit. Same number as the edge, for the same reason.
const SETTLE: u64 = 7_000;

/// Attempts with no effect, on the same device, before turning itself off.
///
/// Three and not ten because each attempt touches someone's keyboard. If
/// two did not fix it, the third will not either -- what is wrong is our
/// model of the world, not luck.
const FAILURE_LIMIT: u32 = 3;

/// How long to wait between the action and the check.
///
/// A channel switch takes ~120 ms for the device to leave the receiver,
/// but up to ~1.5 s to show up on the other one. Checking before that
/// would read "did not arrive" for a switch that was on its way.
const CHECK_AFTER: Duration = Duration::from_millis(1_800);

/// Wait after an attempt that was NOT confirmed.
///
/// Longer than `AFTER_ACTION` on purpose: if it did not work, repeating
/// quickly only multiplies the damage.
const AFTER_FAILURE: Duration = Duration::from_secs(30);

struct Seen {
    /// local slot, if it is here
    local_slot: Option<u8>,
    /// channel the device is on, if we found out
    channel: Option<u8>,
    /// peer holding it (host name), if not here
    holder: Option<String>,
    /// slot on the peer that holds it
    holder_slot: Option<u8>,
}

/// Where each `[[devices]]` device is, asking here and the peers.
fn locate(cfg: &Config) -> std::collections::BTreeMap<String, Seen> {
    let mut out: std::collections::BTreeMap<String, Seen> = cfg
        .devices
        .iter()
        .map(|d| {
            (
                d.name.clone(),
                Seen { local_slot: None, channel: None, holder: None, holder_slot: None },
            )
        })
        .collect();

    // Local, over HID++.
    let local = hidpp::survey();
    for dev in local.receivers.iter().flat_map(|r| r.devices.iter()) {
        if let Some(s) = out.get_mut(&dev.name) {
            if dev.online {
                s.local_slot = Some(dev.slot);
                s.channel = dev.current_host;
            }
        }
    }

    // Peers, over SSH. The agent reports slot and channel, but NOT the
    // name -- so we match by channel, not by name: if the peer sees a
    // device on its channel, it is the one that is there.
    for h in cfg.hosts.iter().filter(|h| !h.is_local()) {
        // It only makes sense to ask hosts with a declared channel and agent.
        if h.ssh.is_none() || h.agent.is_none() { continue }
        let info = agent::devices(h);
        if !info.reachable {
            continue;
        }
        for slot in info.slots.iter().filter(|s| s.online) {
            // We match by NAME. Matching by channel was ambiguous: mouse
            // and keyboard declare the same channel for the same host.
            let Some(name) = slot.name.as_deref() else { continue };
            let Some(seen) = out.get_mut(name) else { continue };
            if seen.local_slot.is_some() {
                continue; // it is here; the peer cannot have it
            }
            seen.channel = slot.current_host;
            seen.holder = Some(h.name.clone());
            seen.holder_slot = Some(slot.slot);
        }
    }
    out
}

/// What came out of an attempt, so the breaker can decide.
pub enum Attempt {
    /// Checked: the device left where it was.
    Moved(String),
    /// We asked, nobody complained, and nothing happened. The case that
    /// became a loop.
    NoEffect(String),
    Error(String),
}

impl Attempt {
    fn text(&self) -> &str {
        match self {
            Attempt::Moved(t) | Attempt::NoEffect(t) | Attempt::Error(t) => t,
        }
    }
    fn ok(&self) -> bool {
        matches!(self, Attempt::Moved(_))
    }
}

/// Did the device really leave the peer's slot?
///
/// Asks its agent again. If the peer does not answer, we treat it as "did
/// not leave": when in doubt, do NOT count it as success -- counting a
/// false success is exactly what produced the loop of seven requests.
fn left_the_peer(h: &crate::config::HostCfg, slot: u8) -> bool {
    let info = agent::devices(h);
    if !info.reachable {
        return false;
    }
    !info.slots.iter().any(|s| s.slot == slot && s.online)
}

/// One reconciliation pass. Returns what it did, per device.
fn reconcile(cfg: &Config) -> Vec<(String, Attempt)> {
    let mut acted: Vec<(String, Attempt)> = Vec::new();
    let Some(leader_name) = cfg.input.follow.leader.clone() else {
        return acted;
    };
    let seen = locate(cfg);
    let Some(leader) = seen.get(&leader_name) else {
        return vec![(
            leader_name.clone(),
            Attempt::Error(format!("leader '{leader_name}' is not in [[devices]]")),
        )];
    };
    let Some(target_channel) = leader.channel else {
        // Without knowing where the leader is, we move nothing. Moving to
        // the wrong channel leaves the device unreachable from both machines.
        return acted;
    };

    for (name, s) in seen.iter() {
        if name == &leader_name || s.channel == Some(target_channel) {
            continue;
        }
        let Some(dcfg) = cfg.devices.iter().find(|d| &d.name == name) else { continue };

        if let Some(slot) = s.local_slot {
            // We hold it: we act directly. This path ALREADY checks -- the
            // `change_host` waits for the device to leave the receiver.
            let r = match hidpp::change_host(slot, target_channel) {
                Ok(detail) => Attempt::Moved(format!(
                    "{name} followed the leader to channel {target_channel}: {detail}"
                )),
                Err(e) => Attempt::Error(format!("{name}: FAILED to follow -- {e}")),
            };
            acted.push((name.clone(), r));
        } else if let (Some(holder), Some(hslot)) = (&s.holder, s.holder_slot) {
            // A peer holds it: we ask it -- and we CHECK, because the
            // `set_host` on the other side returns Ok with no radio reply.
            let Some(hc) = cfg.hosts.iter().find(|h| &h.name == holder) else { continue };
            if hc.ssh.is_none() || hc.agent.is_none() { continue }
            let r = match agent::set_host(hc, hslot, target_channel) {
                Ok(()) => {
                    std::thread::sleep(CHECK_AFTER);
                    if left_the_peer(hc, hslot) {
                        Attempt::Moved(format!(
                            "{name}: {holder} sent it to channel {target_channel}"
                        ))
                    } else {
                        Attempt::NoEffect(format!(
                            "{name}: asked {holder} for channel {target_channel} and it did NOT \
                             leave -- the request was accepted with no effect"
                        ))
                    }
                }
                Err(e) => Attempt::Error(format!("{name}: {holder} could not send it -- {e}")),
            };
            acted.push((name.clone(), r));
        } else {
            // Neither here nor on a known peer: it may be on the MacBook,
            // which has no daemon yet. Nothing to do.
            let _ = dcfg;
        }
    }
    acted
}

/// What to do after a pass.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Nothing was done; just wait for the interval.
    Nothing,
    /// Acted and confirmed. Short grace so as not to fight itself.
    Wait,
    /// Acted, not confirmed. Long grace: repeating quickly multiplies the
    /// damage.
    WaitLonger,
    /// Limit exceeded: turn the reconciler off and say why.
    TurnOff { dev: String, attempts: u32 },
}

/// Counts the failures and decides. Pure on purpose: the breaker is the
/// part that cannot be wrong, and a loop with a `sleep` inside is not
/// testable.
///
/// Mutates `failures` because the count is PER DEVICE and spans passes: a
/// success resets that device without forgiving the others.
fn evaluate(
    acted: &[(String, Attempt)],
    failures: &mut std::collections::BTreeMap<String, u32>,
) -> Action {
    if acted.is_empty() {
        return Action::Nothing;
    }
    let mut any_failure = false;
    let mut tripped: Option<(String, u32)> = None;
    for (dev, r) in acted {
        if r.ok() {
            failures.remove(dev);
            continue;
        }
        any_failure = true;
        let n = failures.entry(dev.clone()).or_insert(0);
        *n += 1;
        if *n >= FAILURE_LIMIT && tripped.is_none() {
            tripped = Some((dev.clone(), *n));
        }
    }
    match tripped {
        Some((dev, attempts)) => Action::TurnOff { dev, attempts },
        None if any_failure => Action::WaitLonger,
        None => Action::Wait,
    }
}

pub fn spawn(cfg: Arc<RwLock<Config>>) {
    std::thread::spawn(move || {
        // The loop ALWAYS runs and reads the state on every pass, instead
        // of exiting when it is off at boot. Before, turning it on from the
        // UI had no effect until the daemon restarted -- and the API
        // response said "takes effect on next boot", which made the user
        // distrust the switch exactly when he needed it.
        let mut failures: std::collections::BTreeMap<String, u32> = Default::default();
        let mut announced = false;

        loop {
            let interval = cfg
                .read()
                .map(|g| g.input.follow.interval_seconds.max(2))
                .unwrap_or(5);
            std::thread::sleep(Duration::from_secs(interval));

            let Ok(snap) = cfg.read().map(|g| g.clone()) else { continue };
            if !snap.input.follow.enabled {
                // Off: the previous state no longer counts. Turning it back
                // on after fixing the problem starts from zero, instead of
                // inheriting three old failures and turning off on the first.
                failures.clear();
                announced = false;
                continue;
            }
            let Some(leader) = snap.input.follow.leader.clone() else {
                if !announced {
                    gripe!("follow: on without `leader`; nothing to do");
                    announced = true;
                }
                continue;
            };
            if !announced {
                say!("follow: reconciling every {interval}s; leader = {leader}");
                announced = true;
            }

            // Right after an arrival the state is in transit, and the
            // reconciler would read a photo that is already stale. The
            // user's log showed the damage: `follow: asked windows-host to
            // send it to channel 1` eleven seconds before the edge sent
            // everything to channel 2, and the two undoing each other
            // while he tried to work.
            if let Some(ms) = crate::hidpp::arrived_ms_ago() {
                if ms < SETTLE {
                    continue;
                }
            }

            // Same lock as the switches: never compete with the edge or the API.
            let acted = {
                let _g = focus::switch_lock();
                reconcile(&snap)
            };
            if acted.is_empty() {
                continue;
            }

            for (_, r) in &acted {
                say!("follow: {}", r.text());
            }

            // Breaker: the reconciler turns itself off instead of insisting.
            //
            // Not abstract caution -- it was measured. Seven requests in two
            // minutes, none with effect, and the user unable to use the
            // keyboard. Turning off loses automatic reconciliation;
            // insisting loses the machine.
            match evaluate(&acted, &mut failures) {
                Action::Nothing => continue,
                Action::Wait => std::thread::sleep(AFTER_ACTION),
                Action::WaitLonger => std::thread::sleep(AFTER_FAILURE),
                Action::TurnOff { dev, attempts } => {
                    gripe!(
                        "follow: {attempts} attempts with no effect on '{dev}' -- TURNING OFF \
                         the reconciler. Fix the cause and turn it back on in the Disposicao section."
                    );
                    if let Ok(mut g) = cfg.write() {
                        g.input.follow.enabled = false;
                    }
                    if let Err(e) = crate::config::save_follow(false, None) {
                        gripe!("follow: could not save the shutdown: {e}");
                    }
                    failures.clear();
                    announced = false;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moved(d: &str) -> (String, Attempt) {
        (d.into(), Attempt::Moved("ok".into()))
    }
    fn no_effect(d: &str) -> (String, Attempt) {
        (d.into(), Attempt::NoEffect("accepted and nothing happened".into()))
    }

    #[test]
    fn success_does_not_count_as_failure() {
        let mut f = Default::default();
        assert_eq!(evaluate(&[moved("mouse")], &mut f), Action::Wait);
        assert!(f.is_empty());
    }

    #[test]
    fn three_with_no_effect_turn_it_off() {
        // The real case: seven requests accepted, none with effect. On the
        // third attempt the reconciler has to turn itself off, not keep
        // asking.
        let mut f = Default::default();
        assert_eq!(evaluate(&[no_effect("mouse")], &mut f), Action::WaitLonger);
        assert_eq!(evaluate(&[no_effect("mouse")], &mut f), Action::WaitLonger);
        assert_eq!(
            evaluate(&[no_effect("mouse")], &mut f),
            Action::TurnOff { dev: "mouse".into(), attempts: 3 }
        );
    }

    #[test]
    fn success_in_between_resets_the_count() {
        // Otherwise two failures hours apart, with a success between them,
        // would turn the reconciler off for no reason.
        let mut f = Default::default();
        evaluate(&[no_effect("mouse")], &mut f);
        evaluate(&[no_effect("mouse")], &mut f);
        evaluate(&[moved("mouse")], &mut f);
        assert_eq!(evaluate(&[no_effect("mouse")], &mut f), Action::WaitLonger);
    }

    #[test]
    fn the_count_is_per_device() {
        // A problematic device does not condemn the other: the success of
        // one does not forgive the other, and the failure of one does not
        // accuse the other.
        let mut f = Default::default();
        evaluate(&[no_effect("mouse"), moved("keyboard")], &mut f);
        evaluate(&[no_effect("mouse"), moved("keyboard")], &mut f);
        assert_eq!(
            evaluate(&[no_effect("mouse"), moved("keyboard")], &mut f),
            Action::TurnOff { dev: "mouse".into(), attempts: 3 }
        );
    }

    #[test]
    fn empty_pass_touches_nothing() {
        let mut f = std::collections::BTreeMap::from([("mouse".to_string(), 2u32)]);
        assert_eq!(evaluate(&[], &mut f), Action::Nothing);
        assert_eq!(f.get("mouse"), Some(&2), "must not forgive on its own");
    }
}
