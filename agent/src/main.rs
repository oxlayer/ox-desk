// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! oxdesk-agent -- the Windows side.
//!
//! Spec: ../specs/005-input-switch.md
//!
//! It is a CLI, not a server, on purpose: `oxdesk` invokes it over SSH,
//! which we already have authenticated by key. Zero new ports, zero new
//! auth, zero network surface. JSON output for the control plane to
//! consume.
//!
//! The exception is `guard`, which has to stay resident -- and in the
//! interactive session, because GetLastInputInfo does not see input from
//! another session.
//!
//! The JSON `error`/`reason` values stay pt-BR: they travel to `oxdesk`
//! and can reach the panel. The log lines are English; `watch-edge START`
//! is the marker people grep for (specs/005).

use winput as input;

use hidpp::hidapi::HidApi;
use serde_json::json;
use std::process::ExitCode;

/// File log for the resident modes.
///
/// Launched from a Startup shortcut, stdout and stderr go nowhere -- and
/// that is exactly why I ended up diagnosing the Windows side in the
/// dark. The Linux side has the journal; this is the equivalent.
fn logline(msg: &str) {
    use std::io::Write;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("{ts} {msg}\n");
    eprint!("{line}");
    let Some(dir) = std::env::var_os("LOCALAPPDATA") else { return };
    let path = std::path::Path::new(&dir).join("oxdesk").join("watch-edge.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

fn out(v: serde_json::Value) -> ExitCode {
    println!("{v}");
    ExitCode::SUCCESS
}

fn fail(msg: impl AsRef<str>) -> ExitCode {
    println!("{}", json!({ "ok": false, "error": msg.as_ref() }));
    ExitCode::FAILURE
}

fn usage() -> ExitCode {
    eprintln!(
        "oxdesk-agent <command>

  devices                      HID++ interfaces and current host of each slot
  host-info <slot>             number of hosts and current channel (slot 1..6)
  set-host <slot> <host> --confirm
                               switches the channel. 1-based on the CLI.
                               IRREVERSIBLE: the device leaves this
                               receiver and no longer answers here.
  input-age                    ms since the last input in this session
  watch-edge <edge> <slots> <host> [hold_ms]
                               watches the edge and sends the devices to
                               <host> on touch. E.g.: watch-edge left 1,2 1
                               Runs in the interactive session.
  guard <slot> <host> <seconds>
                               dead-man's switch: if there is no input in
                               <seconds>, returns the device to
                               <host>. Run it in the interactive session."
    );
    ExitCode::FAILURE
}

/// Scans slots 1..6 collecting host info from whoever answers.
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        return usage();
    };

    if cmd == "input-age" {
        return match input::idle_ms() {
            Ok(ms) => out(json!({ "ok": true, "idle_ms": ms })),
            Err(e) => fail(e),
        };
    }

    let api = match HidApi::new() {
        Ok(a) => a,
        Err(e) => return fail(format!("hidapi: {e}")),
    };

    // The commands the PEER consumes -- devices, host-info, set-host -- come
    // from the shared crate. Their format is the contract between the two
    // machines, and two copies of it would drift (see hidpp/src/protocol.rs).
    if let hidpp::protocol::Handled::Json(v) = hidpp::protocol::run(&api, &args) {
        let ok = v.get("ok").and_then(|b| b.as_bool()) == Some(true);
        println!("{v}");
        return if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE };
    }

    match cmd {

        // Probing: prints the raw bytes so I can confirm a feature's
        // layout before writing a parser on top of a guess.
        "raw" => {
            let slot = args.get(1).and_then(|s| s.parse::<u8>().ok());
            let feat = args.get(2).and_then(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16).ok());
            let func = args.get(3).and_then(|s| s.parse::<u8>().ok());
            let (Some(slot), Some(feat), Some(func)) = (slot, feat, func) else {
                return fail("uso: raw <slot> <feature hex> <func> [p0 p1 p2]");
            };
            let p = |i: usize| args.get(i).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
            let link = match hidpp::Receiver::open(&api) {
                Ok(l) => l,
                Err(e) => return fail(e),
            };
            match link.raw(slot, feat, func, &[p(4), p(5), p(6)]) {
                Ok(r) => out(json!({ "ok": true,
                    "hex": r.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
                    "ascii": r.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }).collect::<String>(),
                    "bytes": r })),
                Err(e) => fail(e),
            }
        }

        "watch-edge" => {
            let edge = args.get(1).cloned().unwrap_or_default();
            let slots: Vec<u8> = args
                .get(2)
                .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
                .unwrap_or_default();
            let host = args.get(3).and_then(|s| s.parse::<u8>().ok());
            let hold_ms: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(250);
            let (Some(host), false) = (host, slots.is_empty()) else {
                return fail("uso: watch-edge <edge> <slot,slot> <host 1-based> [hold_ms]");
            };
            if host == 0 {
                return fail("host e 1-based");
            }
            if !matches!(edge.as_str(), "left" | "right" | "top" | "bottom") {
                return fail("edge: left | right | top | bottom");
            }

            if !input::in_console_session() {
                return fail(
                    "watch-edge needs the console session: GetCursorPos and \
                     GetLastInputInfo are per session. Launched like this it \
                     would see no input and would still compete for the \
                     receiver. Use the shortcut in shell:startup.",
                );
            }
            if !input::claim_single_instance("oxdesk-agent-watch-edge") {
                return fail(
                    "a watch-edge is already running -- three instances \
                     competing for the same receiver was a real problem",
                );
            }

            let (vx, vy, vw, vh) = input::virtual_screen();
            logline(&format!(
                "watch-edge START edge='{edge}' area=({vx},{vy} {vw}x{vh}) \
                 slots={slots:?} -> host {host} hold={hold_ms}ms"
            ));

            let mut touching_since: Option<std::time::Instant> = None;
            let mut cooldown_until: Option<std::time::Instant> = None;

            // Dead-man's switch. Spec 005 requires it and I had never
            // enabled it, because I found it CANNOT be launched over SSH:
            // GetLastInputInfo does not see the user's session from
            // outside it. In here it works -- this process is already
            // resident in the session and already polling.
            //
            // Logic: when the devices ARRIVE on this machine, we give a
            // window. If no input shows up in it, we send them back -- the
            // sender cannot undo, only the holder can.
            let guard_secs: u64 = std::env::var("OXDESK_GUARD_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(25);
            let mut present_before = false;
            let mut arrived_at: Option<std::time::Instant> = None;
            // The loop spins every 30 ms so the cursor responds fast, but
            // the presence check is HID++: at 33 Hz it would hammer the
            // radio for no reason. Once per second is enough -- the guard
            // window is tens of seconds.
            let mut last_check = std::time::Instant::now()
                - std::time::Duration::from_secs(2);
            loop {
                std::thread::sleep(std::time::Duration::from_millis(30));

                if let Some(t) = cooldown_until {
                    if std::time::Instant::now() < t {
                        continue;
                    }
                    cooldown_until = None;
                }

                // --- dead-man's switch: detect arrival and watch for input
                if last_check.elapsed() >= std::time::Duration::from_secs(1) {
                    last_check = std::time::Instant::now();
                if let Ok(link) = hidpp::Receiver::open(&api) {
                    let present = slots.iter().any(|s| link.present(*s, 2));
                    if present && !present_before {
                        let idle_on_arrival = input::idle_ms().unwrap_or(0);
                        arrived_at = Some(std::time::Instant::now());
                        logline(&format!(
                            "devices ARRIVED -- watching input for {guard_secs}s \
                             (idle on arrival: {idle_on_arrival}ms)"
                        ));
                    } else if !present && present_before {
                        arrived_at = None;
                        logline("devices LEFT -- watch ended");
                    }
                    present_before = present;

                    if let Some(t0) = arrived_at {
                        match input::idle_ms() {
                            // New input: idle dropped below the time since
                            // arrival, so there was activity after it.
                            Ok(idle) if idle < t0.elapsed().as_millis() as u64 => {
                                logline("input detected -- dead-man's switch disarmed");
                                arrived_at = None;
                            }
                            Ok(idle) if t0.elapsed().as_secs() >= guard_secs => {
                                logline(&format!(
                                    "NO INPUT in {guard_secs}s (idle {idle}ms) -- \
                                     returning to host {host}"
                                ));
                                for slot in &slots {
                                    if link.ping(*slot).is_err() {
                                        continue;
                                    }
                                    match link.set_host(*slot, host) {
                                        Ok(()) => logline(&format!(
                                            "rescue: slot {slot} -> host {host}"
                                        )),
                                        Err(e) => logline(&format!(
                                            "rescue of slot {slot} FAILED: {e}"
                                        )),
                                    }
                                }
                                arrived_at = None;
                                cooldown_until = Some(
                                    std::time::Instant::now()
                                        + std::time::Duration::from_secs(15),
                                );
                                continue;
                            }
                            _ => {}
                        }
                    }
                }
                }

                let Some(p) = input::cursor() else { continue };
                if !input::at_edge(&edge, p) {
                    touching_since = None;
                    continue;
                }

                // Require dwell: avoids switching on a graze of the edge.
                let since = *touching_since.get_or_insert_with(std::time::Instant::now);
                if (since.elapsed().as_millis() as u64) < hold_ms {
                    continue;
                }
                touching_since = None;

                let link = match hidpp::Receiver::open(&api) {
                    Ok(l) => l,
                    Err(e) => {
                        logline(&format!("no HID++ right now: {e}"));
                        cooldown_until =
                            Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
                        continue;
                    }
                };

                // Presence BEFORE trying. Without this, touching the edge
                // with the devices on another machine reported
                // "sem resposta para feature 0x00 func 0x00 ... switch_failed"
                // -- which is absence, not failure. It also covers diverging
                // slot numbers: the slot is per receiver and does not match
                // across machines.
                // `present` with retry, not `host_info`: see its doc in the
                // crate. This line is what made the edge "not work".
                let present_now: Vec<u8> = slots
                    .iter()
                    .copied()
                    .filter(|s| link.present(*s, 3))
                    .collect();
                if present_now.is_empty() {
                    logline(&format!(
                        "EDGE TOUCHED at ({},{}) but none of slots {slots:?} is \
                         on this receiver -- nothing to send",
                        p.x, p.y
                    ));
                    cooldown_until =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(10));
                    continue;
                }
                if present_now.len() < slots.len() {
                    logline(&format!(
                        "only {present_now:?} of {slots:?} are here -- sending those; \
                         the others stay where they are (SPLIT STATE)"
                    ));
                }

                logline(&format!(
                    "EDGE TOUCHED at ({},{}) -- sending {present_now:?} to host {host}",
                    p.x, p.y
                ));
                for slot in &present_now {
                    match link.set_host(*slot, host) {
                        Ok(()) => logline(&format!("slot {slot} -> host {host} sent")),
                        Err(e) => logline(&format!("slot {slot} FAILED: {e}")),
                    }
                }
                // The devices just left: there is no input here to fire
                // again, but if they come back we want to watch again.
                cooldown_until =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(10));
            }
        }

        "guard" => {
            let slot = args.get(1).and_then(|s| s.parse::<u8>().ok());
            let host = args.get(2).and_then(|s| s.parse::<u8>().ok());
            let secs = args.get(3).and_then(|s| s.parse::<u64>().ok());
            let (Some(slot), Some(host), Some(secs)) = (slot, host, secs) else {
                return fail("usage: guard <slot> <host 1-based> <seconds>");
            };
            let budget_ms = secs * 1000;
            let started = std::time::Instant::now();
            loop {
                std::thread::sleep(std::time::Duration::from_millis(400));
                match input::idle_ms() {
                    Ok(idle) if idle < budget_ms => {
                        // There was input: the user arrived, mission done.
                        return out(json!({ "ok": true, "action": "none",
                                           "reason": "input detectado",
                                           "idle_ms": idle }));
                    }
                    Ok(idle) => {
                        if started.elapsed().as_millis() as u64 >= budget_ms {
                            let link = match hidpp::Receiver::open(&api) {
                                Ok(l) => l,
                                Err(e) => return fail(format!("rescue failed to open: {e}")),
                            };
                            return match link.set_host(slot, host) {
                                Ok(()) => out(json!({ "ok": true, "action": "reverted",
                                                      "to_host": host, "idle_ms": idle })),
                                Err(e) => fail(format!("rescue failed: {e}")),
                            };
                        }
                    }
                    Err(e) => return fail(e),
                }
            }
        }

        _ => usage(),
    }
}
