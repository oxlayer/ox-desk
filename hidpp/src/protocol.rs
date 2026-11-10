// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: MIT

//! The protocol between peers: one line of JSON per command.
//!
//! Who speaks: `oxdesk-agent` on Windows and `oxdesk` on any OS, each
//! invoked over SSH by the peer (see `oxdesk/src/agent.rs`). Who reads:
//! the `agent.rs` on the other side, which looks for the first line
//! starting with `{`.
//!
//! ## Why it lives in the shared crate
//!
//! This used to be `survey()` inside `agent/src/main.rs`. When `oxdesk`
//! started needing to answer the same commands -- so Windows could probe
//! Ubuntu the way Ubuntu already probed Windows -- the choice was copy or
//! share. Two copies of the **contract between the machines** would
//! drift, and the first drift would show up as "agent did not answer
//! JSON" in a log, far from the cause.
//!
//! Here, both binaries emit the same format by construction.
//!
//! JSON keys (`name`, `receiver`, `receivers`, ...) are the wire format
//! and stay as they are. The `error` texts stay pt-BR: they reach the
//! panel.

use crate::{endpoints, Receiver};
use hidapi::HidApi;
use serde_json::{json, Value};

/// Result of a protocol command.
pub enum Handled {
    /// Known command; this is the JSON to print.
    Json(Value),
    /// Not a protocol command -- the caller handles it (the agent has
    /// `watch-edge` and `guard`; oxdesk, serving HTTP).
    Unknown,
}

/// Runs a protocol command, if it is one of them.
///
/// `args` is the command line **without** the program name.
pub fn run(api: &HidApi, args: &[String]) -> Handled {
    let Some(cmd) = args.first().map(String::as_str) else {
        return Handled::Unknown;
    };
    match cmd {
        "devices" => Handled::Json(devices(api)),
        "host-info" => {
            let Some(slot) = args.get(1).and_then(|s| s.parse::<u8>().ok()) else {
                return Handled::Json(fail("uso: host-info <slot 1..6>"));
            };
            Handled::Json(host_info(api, slot))
        }
        "set-host" => {
            let slot = args.get(1).and_then(|s| s.parse::<u8>().ok());
            let host = args.get(2).and_then(|s| s.parse::<u8>().ok());
            let (Some(slot), Some(host)) = (slot, host) else {
                return Handled::Json(fail("uso: set-host <slot> <host 1-based> --confirm"));
            };
            let confirmed = args.iter().any(|a| a == "--confirm");
            Handled::Json(set_host(api, slot, host, confirmed))
        }
        "hosts" => {
            let Some(slot) = args.get(1).and_then(|s| s.parse::<u8>().ok()) else {
                return Handled::Json(fail("uso: hosts <slot 1..6>"));
            };
            Handled::Json(hosts(api, slot))
        }
        _ => Handled::Unknown,
    }
}

/// A device's hosts, READ FROM IT (0x1815 HOSTS_INFO).
///
/// Exists because I spent a morning deducing where channel 2 points
/// from what the RECEIVERS show, and got it wrong twice: the receiver
/// cannot name an absent device, and an empty slot looks the same as a
/// paired-but-away slot. The config is no proof either -- it is what
/// someone TYPED. The device is who knows.
///
/// The layout came from a real reply, not from guessing (see
/// `Receiver::raw` and the history in specs/005):
///
/// ```text
/// func 1 (getHostInfo),  param [i]:     i, ?, bus, ?, name_len, max
/// func 3 (getHostName),  param [i,off]: i, off, name bytes...
/// ```
///
/// `bus` came as 0x01 on hosts through a Unifying receiver and 0x04 on
/// the Bluetooth host -- that is what told apart, among the MX Keys'
/// three hosts, `linux-pc` and `WINDOWS-PC` (Unifying) from the
/// MacBook (Bluetooth).
///
/// Not a probe: `0x1815` is optional and the MX Anywhere 3 does not
/// implement it, so the reply says so instead of making something up.
pub fn hosts(api: &HidApi, slot: u8) -> Value {
    let link = match Receiver::open(api) {
        Ok(l) => l,
        Err(e) => return fail(e),
    };
    const F: u16 = 0x1815;
    const INFO: u8 = 1;
    const NAME: u8 = 3;
    /// Name bytes per reply, after the two header bytes.
    const PER_CHUNK: usize = 14;

    let mut list = Vec::new();
    for i in 0u8..8 {
        // End of list is an error, not a sentinel: the device rejects an
        // out-of-range index. Stop at the first rejection.
        let info = match link.raw(slot, F, INFO, &[i, 0, 0]) {
            Ok(r) if r.len() >= 6 => r,
            Ok(_) => break,
            Err(e) if i == 0 => return fail(format!("0x1815 em slot {slot}: {e}")),
            Err(_) => break,
        };
        let len = info[4] as usize;
        let bus = info[2];

        let mut name = Vec::new();
        let mut off = 0usize;
        while name.len() < len && off < 64 {
            match link.raw(slot, F, NAME, &[i, off as u8, 0]) {
                Ok(r) if r.len() > 2 => name.extend_from_slice(&r[2..]),
                _ => break,
            }
            off += PER_CHUNK;
        }
        name.truncate(len);
        // The name comes in UTF-8 (the Mac's carries the typographic
        // apostrophe in three bytes), so it cannot be treated byte by byte
        // as ASCII.
        let name = String::from_utf8_lossy(&name).trim_end_matches('\0').to_string();

        list.push(json!({
            "host": i + 1,
            "name": name,
            "bus": match bus { 0x01 => "unifying", 0x04 => "bluetooth", _ => "?" },
            "bus_byte": bus,
        }));
    }
    json!({ "ok": true, "slot": slot, "hosts": list })
}

pub fn fail(msg: impl Into<String>) -> Value {
    json!({ "ok": false, "error": msg.into() })
}

/// Names seen per slot, on disk.
///
/// ## Why it exists
///
/// Reading the name requires the device to ANSWER, so a paired slot
/// whose device is on another machine comes out of the inventory the
/// same as an empty slot: `{"slot": 1, "online": false}`. That is no
/// detail -- it is what made me claim, wrongly, that the keyboard was not
/// paired to the Windows receiver, and lose an hour investigating a
/// pairing problem that did not exist. The device was paired, just away.
///
/// The Linux side already kept names inside `oxdesk`; the agent had
/// nothing. Now it lives here, in the crate both use, because the
/// question ("empty or absent?") is the same for both.
fn cache_path() -> Option<std::path::PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)?.join("oxdesk")
    } else {
        match std::env::var_os("XDG_CACHE_HOME") {
            Some(v) => std::path::PathBuf::from(v),
            None => std::path::PathBuf::from(std::env::var_os("HOME")?).join(".cache"),
        }
        .join("oxdesk")
    };
    Some(base.join("slot-names.json"))
}

fn read_names() -> std::collections::BTreeMap<String, String> {
    cache_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Writes what was seen now, keeping the rest.
///
/// Fails silently on purpose: the cache is a convenience, and an
/// inventory that refuses to answer because it could not write to disk
/// would be worse than an inventory without the name of an absent slot.
fn write_names(seen: &std::collections::BTreeMap<String, String>) {
    let Some(p) = cache_path() else { return };
    let mut current = read_names();
    current.extend(seen.clone());
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(t) = serde_json::to_string(&current) {
        let _ = std::fs::write(p, t);
    }
}

/// Inventory of the local receiver's slots.
pub fn devices(api: &HidApi) -> Value {
    let interfaces: Vec<_> = endpoints(api)
        .into_iter()
        .map(|(path, pid, usage_page, usage)| {
            json!({ "path": path, "product_id": format!("{pid:04x}"),
                    "usage_page": format!("{usage_page:#06x}"), "usage": usage })
        })
        .collect();

    // ALL receivers, not the first. With two on the same machine, picking
    // one made the agent list the slots of one and declare the devices of
    // the other absent.
    let links = Receiver::open_all(api);
    if links.is_empty() {
        return json!({ "ok": false, "interfaces": interfaces,
                       "error": "no Logitech HID++ receiver connected here" });
    }

    // Efficient path: 250 ms ping first, and host_info (more expensive)
    // only for those that answered. We used to call host_info on all 6
    // slots -- measured, 6.4 s per probe.
    // With names: the reconciler matches devices by NAME, not by channel
    // -- the channel is ambiguous when mouse and keyboard declare the same.
    let known = read_names();
    let mut seen = std::collections::BTreeMap::new();
    let slots: Vec<_> = links
        .iter()
        .enumerate()
        .flat_map(|(i, link)| {
            let rid = format!("{:04x}@{i}", link.product_id);
            link.devices(6, true, false).into_iter().map(move |d| (rid.clone(), d))
        })
        .map(|(rid, d)| {
            if d.online {
                if let Some(n) = d.name.clone() {
                    seen.insert(format!("{rid}/{}", d.slot), n);
                }
                json!({
                    "slot": d.slot,
                    "receiver": rid,
                    "name": d.name,
                    "online": true,
                    "paired": true,
                    "num_hosts": d.host.map(|h| h.num_hosts),
                    // Already 1-based: the crate converts from the protocol.
                    "current_host": d.host.map(|h| h.current),
                    "ping_ms": d.reply_ms,
                })
            } else {
                // Name from the cache: separates "empty slot" from "paired,
                // but the device is on another machine". Without this both
                // cases are the same JSON, and the reader concludes wrong.
                let remembered = known.get(&format!("{rid}/{}", d.slot));
                json!({
                    "slot": d.slot,
                    "receiver": rid,
                    "name": remembered,
                    "online": false,
                    "paired": remembered.is_some(),
                })
            }
        })
        .collect();
    write_names(&seen);
    json!({ "ok": true, "interfaces": interfaces,
            "receivers": links.len(), "slots": slots })
}

pub fn host_info(api: &HidApi, slot: u8) -> Value {
    let link = match Receiver::open(api) {
        Ok(l) => l,
        Err(e) => return fail(e),
    };
    match link.host_info(slot) {
        Ok(hi) => json!({ "ok": true, "slot": slot,
                          "num_hosts": hi.num_hosts, "current_host": hi.current }),
        Err(e) => fail(e),
    }
}

pub fn set_host(api: &HidApi, slot: u8, host: u8, confirmed: bool) -> Value {
    if host == 0 {
        return fail("host e 1-based");
    }
    // Same stance as /api/focus: an irreversible action does not happen
    // by accident or by distracted typing.
    if !confirmed {
        return fail(
            "--confirm is missing: this action cannot be undone \
             (the device leaves this receiver)",
        );
    }
    let link = match Receiver::open(api) {
        Ok(l) => l,
        Err(e) => return fail(e),
    };
    match link.set_host(slot, host) {
        Ok(()) => json!({ "ok": true, "slot": slot, "host": host }),
        Err(e) => fail(e),
    }
}
