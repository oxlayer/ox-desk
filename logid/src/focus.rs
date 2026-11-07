//! Handing input focus to another machine.
//!
//! Spec: ../specs/005-input-switch.md
//!
//! Dry-run by default. The physical route is IRREVERSIBLE on the source
//! side (after ChangeHost this receiver no longer reaches the device),
//! so the pre-flight replaces the rollback -- there is nothing to undo.

use crate::agent::{self, AgentInfo};
use crate::peers;
use crate::config::Config;
use crate::hidpp;
use crate::probe;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Serializes channel switches.
///
/// It exists because we measured the race: the edge watcher and the API
/// can fire at the same time, and the second one arrived with the devices
/// already taken by the first -- "executed: true" with zero results. One
/// switch at a time, and eligibility is re-evaluated inside the lock.
static SWITCH_LOCK: Mutex<()> = Mutex::new(());

/// Shared with the reconciler ([`crate::follow`]): one switch at a time,
/// whether it comes from the edge, the API or from it.
pub fn switch_lock() -> std::sync::MutexGuard<'static, ()> {
    match SWITCH_LOCK.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    /// Easy-Switch channel change via HID++ 0x1814.
    Physical,
    /// Input events over the network (Deskflow).
    Lan,
    /// No viable route.
    None,
}

#[derive(Debug, Deserialize)]
pub struct FocusRequest {
    pub host: String,
    #[serde(default)]
    pub route: Option<Route>,
    /// Without this, only the plan is returned. Never executes by accident.
    #[serde(default)]
    pub confirm: bool,
    /// Uses the peer state from the cache instead of probing over SSH.
    ///
    /// Takes ~2.2 s off the path. The check then declares the age of the
    /// data instead of faking freshness. It is the mode the edge uses.
    #[serde(default)]
    pub fast: bool,
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    /// A blocking failure prevents execution.
    pub blocking: bool,
}

impl Check {
    fn pass(name: &str, detail: impl Into<String>) -> Self {
        Check { name: name.into(), ok: true, detail: detail.into(), blocking: false }
    }
    fn block(name: &str, detail: impl Into<String>) -> Self {
        Check { name: name.into(), ok: false, detail: detail.into(), blocking: true }
    }
    fn warn(name: &str, detail: impl Into<String>) -> Self {
        Check { name: name.into(), ok: false, detail: detail.into(), blocking: false }
    }
}

#[derive(Debug, Serialize)]
pub struct Plan {
    pub target: String,
    pub route: Route,
    pub checks: Vec<Check>,
    /// Every blocking check passed.
    pub ready: bool,
    /// If executed, there is no way to undo it from this side.
    pub irreversible: bool,
    /// What would be done, in order.
    pub actions: Vec<String>,
    /// How to recover if it goes wrong -- shown BEFORE, not after.
    pub recovery: Vec<String>,
    /// (name, slot here, target channel) already resolved by the plan's
    /// survey. Not in the JSON: it is an internal detail.
    #[serde(skip)]
    resolved: Resolved,
}

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub plan: Plan,
    pub executed: bool,
    pub results: Vec<String>,
    /// Some devices switched and others did not: mouse and keyboard ended
    /// up on different machines. **There is no rollback** for the ones that
    /// left (spec 005), so this is a state that needs a human -- the
    /// Easy-Switch button on the device that stayed, or asking the holder
    /// to send it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn is_local(h: &crate::config::HostCfg) -> bool {
    h.addr == "127.0.0.1" || h.addr == "localhost" || h.addr == "::1"
}

/// What the plan resolved: (name, slot here, target channel).
///
/// Kept so `execute` does NOT redo the survey: it cost ~1.6 s and had
/// already been paid here.
type Resolved = Vec<(String, u8, u8)>;

pub fn plan(cfg: &Config, req: &FocusRequest) -> Plan {
    let mut checks = Vec::new();
    let mut actions = Vec::new();
    let recovery = vec![
        "press the Easy-Switch button on the device itself (always works)".into(),
        "or plug in a USB keyboard".into(),
        "or log in over SSH from another machine".into(),
    ];

    let Some(target) = cfg.hosts.iter().find(|h| h.name == req.host) else {
        checks.push(Check::block("host", format!("'{}' is not in the config", req.host)));
        return Plan {
            target: req.host.clone(),
            route: Route::None,
            ready: false,
            irreversible: false,
            checks,
            actions,
            recovery,
            resolved: Vec::new(),
        };
    };

    if is_local(target) {
        checks.push(Check::block("host", "the target is this very machine"));
        return Plan {
            target: req.host.clone(),
            route: Route::None,
            ready: false,
            irreversible: false,
            checks,
            actions,
            recovery,
            resolved: Vec::new(),
        };
    }
    checks.push(Check::pass("host", format!("{} ({})", target.name, target.addr)));

    // Reachability: any expected service being up is enough to call it alive.
    let reachable = target
        .services
        .iter()
        .any(|s| probe::tcp_probe(&target.addr, s.port).0);
    if reachable {
        checks.push(Check::pass("reachability", format!("{} answers", target.addr)));
    } else {
        checks.push(Check::block("reachability", format!("{} does not answer", target.addr)));
    }

    let local_name = cfg
        .hosts
        .iter()
        .find(|h| is_local(h))
        .map(|h| h.name.clone())
        .unwrap_or_default();

    let mut resolved: Resolved = Vec::new();
    let want = req.route.unwrap_or(Route::Physical);
    let route = match want {
        Route::Lan => Route::Lan,
        _ => Route::Physical,
    };

    match route {
        Route::Physical => {
            // Local survey and agent probe IN PARALLEL: they were 1.6 s and
            // 2.7 s in sequence, and do not depend on each other.
            // In fast mode we do NOT probe the peer: we read the cache the
            // refresher keeps. In normal mode we probe in parallel with the
            // local survey -- they do not depend on each other.
            let (survey, agent_info, age_ms) = if req.fast {
                let c = peers::cached(&req.host);
                (
                    // Short survey cache: the survey costs ~360 ms and the
                    // refresher keeps it fresh every 1.5 s. If it is stale,
                    // we pay -- correctness before speed.
                    hidpp::survey_cached_or_fresh(std::time::Duration::from_millis(2500)),
                    c.as_ref().map(|(i, _)| AgentInfo {
                        reachable: i.reachable,
                        slots: i.slots.clone(),
                        error: i.error.clone(),
                    }),
                    c.map(|(_, age)| age),
                )
            } else {
                let r = std::thread::scope(|sc| {
                    let h = sc.spawn(|| {
                        if target.ssh.is_some() && target.agent.is_some() {
                            Some(agent::devices(target))
                        } else {
                            None
                        }
                    });
                    let survey = hidpp::survey_opts(false);
                    (survey, h.join().ok().flatten())
                });
                (r.0, r.1, None)
            };
            if survey.hidraw_access {
                checks.push(Check::pass("hidraw", "uaccess ACL present"));
            } else {
                checks.push(Check::block(
                    "hidraw",
                    survey.error.clone().unwrap_or_else(|| "no access".into()),
                ));
            }

            if cfg.devices.is_empty() {
                checks.push(Check::block("channel map", "no [[devices]] in the config"));
            }

            for dcfg in &cfg.devices {
                let Some(&chan) = dcfg.hosts.get(&req.host) else {
                    checks.push(Check::block(
                        "channel map",
                        format!("'{}' has no channel declared for '{}'", dcfg.name, req.host),
                    ));
                    continue;
                };
                let found = survey
                    .receivers
                    .iter()
                    .flat_map(|r| r.devices.iter())
                    .find(|d| d.name == dcfg.name);
                match found {
                    None => checks.push(Check::block(
                        "device ownership",
                        format!("'{}' is not paired with this receiver", dcfg.name),
                    )),
                    Some(d) if !d.online => checks.push(Check::block(
                        "device ownership",
                        format!(
                            "'{}' is offline here -- it is already on another channel, \
                             and HID++ does not reach an offline device",
                            dcfg.name
                        ),
                    )),
                    Some(d) => {
                        checks.push(Check::pass(
                            "device ownership",
                            format!("'{}' online on slot {}", d.name, d.slot),
                        ));
                        actions.push(format!(
                            "HID++ 0x1814 setCurrentHost: slot {} -> channel {} \
                             ({} on {})",
                            d.slot, chan, d.name, req.host
                        ));
                        resolved.push((d.name.clone(), d.slot, chan));
                    }
                }
                if !local_name.is_empty() && !dcfg.hosts.contains_key(&local_name) {
                    checks.push(Check::warn(
                        "return",
                        format!(
                            "'{}' has no channel declared for '{}': without it \
                             nobody knows how to bring it back",
                            dcfg.name, local_name
                        ),
                    ));
                }
            }

            // Dead-man's switch: whoever comes to hold the device is the
            // one who gives it back, because the source CANNOT undo (spec 005).
            match (&target.ssh, &target.agent, agent_info) {
                (Some(_), Some(_), Some(info)) => {
                    if info.reachable {
                        let online: Vec<String> = info
                            .slots
                            .iter()
                            .filter(|s| s.online)
                            .map(|s| {
                                format!(
                                    "slot {} (channel {}/{})",
                                    s.slot,
                                    s.current_host.unwrap_or(0),
                                    s.num_hosts.unwrap_or(0)
                                )
                            })
                            .collect();
                        let age = match age_ms {
                            Some(ms) => format!(" (cache from {ms}ms ago)"),
                            None => String::new(),
                        };
                        checks.push(Check::pass(
                            "dead-man's switch on target",
                            format!(
                                "agent answers{age}; devices there: {}",
                                if online.is_empty() { "none online".into() } else { online.join(", ") }
                            ),
                        ));
                    } else {
                        checks.push(Check::block(
                            "dead-man's switch on target",
                            format!(
                                "agent did not answer ({}). Without it nobody gives \
                                 the device back, and the source cannot undo.",
                                info.error.unwrap_or_else(|| "unknown reason".into())
                            ),
                        ));
                    }
                }
                _ if req.fast => checks.push(Check::block(
                    "dead-man's switch on target",
                    "fast mode with no peer state in the cache -- the refresher \
                     has not read it yet, or the host has no `ssh`/`agent`",
                )),
                _ => checks.push(Check::block(
                    "dead-man's switch on target",
                    "host has no `ssh` and/or `agent` in the config: there is no \
                     way to ask the target to give the device back if nothing gets there.",
                )),
            }
        }
        Route::Lan => {
            let up = probe::tcp_probe(&target.addr, 24800).0;
            if up {
                checks.push(Check::pass("deskflow", "server on 24800 answers"));
            } else {
                checks.push(Check::block("deskflow", "nothing on the target's 24800"));
            }
            // Honesty about what Deskflow allows:
            checks.push(Check::block(
                "programmatic switch",
                "Deskflow switches screens by cursor movement or by a hotkey \
                 configured ON THE SERVER -- not by external command. There is \
                 no way to force focus over the LAN route from here.",
            ));
        }
        Route::None => {}
    }

    let ready = !checks.iter().any(|c| c.blocking && !c.ok);
    Plan {
        target: req.host.clone(),
        route,
        irreversible: route == Route::Physical,
        ready,
        checks,
        actions,
        recovery,
        resolved,
    }
}

pub fn execute(cfg: &Config, req: &FocusRequest) -> Outcome {
    let plan = plan(cfg, req);

    if !req.confirm {
        return Outcome { plan, executed: false, results: vec![], partial: false, error: None };
    }
    if !plan.ready {
        let n = plan.checks.iter().filter(|c| c.blocking && !c.ok).count();
        return Outcome {
            plan,
            executed: false,
            results: vec![],
            partial: false,
            error: Some(format!("{n} blocking check(s) failed")),
        };
    }
    if plan.route != Route::Physical {
        return Outcome {
            plan,
            executed: false,
            results: vec![],
            partial: false,
            error: Some("only the physical route is executable".into()),
        };
    }

    // One switch at a time. We re-evaluate eligibility AFTER taking the
    // lock: whoever waited here may have lost the devices.
    let _guard = switch_lock();

    // Point of no return. We reuse what the plan resolved instead of
    // surveying again -- the survey costs ~1.6 s and was already paid.
    let mut results = Vec::new();
    let mut moved = 0usize;
    let mut stuck: Vec<String> = Vec::new();
    if req.fast {
        // The path the user feels: fire both IN PARALLEL and do not wait
        // for confirmation. It used to be 2x (ping + set + 120 ms wait) in
        // sequence; now it is one ping and one set per device, together.
        let resolved = plan.resolved.clone();
        let outs: Vec<(String, Result<(), String>)> = std::thread::scope(|sc| {
            let hs: Vec<_> = resolved
                .iter()
                .map(|(name, slot, chan)| {
                    let (n, s2, c2) = (name.clone(), *slot, *chan);
                    sc.spawn(move || (n, hidpp::change_host_now(s2, c2)))
                })
                .collect();
            hs.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        for ((name, r), (_, slot, chan)) in outs.into_iter().zip(resolved.iter()) {
            match r {
                Ok(()) => {
                    moved += 1;
                    results.push(format!("{name}: sent to channel {chan}"));
                    // Confirmation becomes a log line, not a wait.
                    hidpp::verify_left(*slot, *chan);
                }
                Err(e) => {
                    stuck.push(name.clone());
                    results.push(format!("{name}: FAILED -- {e}"));
                }
            }
        }
    } else {
    for (name, slot, chan) in plan.resolved.clone() {
        let d = (name, slot);
        let (name, slot) = (d.0.as_str(), d.1);
        let _ = name;
        match hidpp::change_host(slot, chan) {
            Ok(detail) => {
                moved += 1;
                results.push(format!("{name}: {detail}"));
            }
            Err(e) => {
                stuck.push(name.to_string());
                results.push(format!("{name}: FAILED -- {e}"));
            }
        }
    }
    }

    // The switch is not atomic: they are independent commands, seconds each.
    // If some went and some stayed, that has to shout.
    let partial = moved > 0 && !stuck.is_empty();
    let error = if partial {
        Some(format!(
            "SPLIT STATE: {} switched, but {} stayed here ({}). There is no \
             rollback for the ones that left -- use the Easy-Switch button on the one that stayed.",
            moved,
            stuck.len(),
            stuck.join(", ")
        ))
    } else if moved == 0 && !stuck.is_empty() {
        Some(format!("no device switched ({})", stuck.join(", ")))
    } else if moved == 0 {
        // It really happened: the pre-flight passed and, in the seconds
        // before execution, a concurrent edge crossing already took
        // everything. "executed: true" with empty results is worse than a
        // clear error.
        Some(
            "no eligible device at execution time -- another \
             switch (edge?) may have taken them all first"
                .into(),
        )
    } else {
        None
    };

    Outcome { plan, executed: true, results, partial, error }
}
