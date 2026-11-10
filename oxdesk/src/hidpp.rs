//! Inventory and channel switching, now over direct HID++.
//!
//! Spec: ../specs/005-input-switch.md
//!
//! Replaced the `solaar` backend, which was too expensive for the hot
//! path: `solaar show` cost 5.9 to 15.3 s and `change_host` called it 4x
//! per device -- a switch took 105 s, and the user concluded it had
//! failed. Over direct HID++ (the `hidpp` crate, on top of hidapi) a ping
//! costs ~25 ms and the full survey ~1.6 s.
//!
//! This module's public boundary did not change: `survey()` and
//! `change_host()` have the same shape, and `focus.rs` never knew about
//! the swap.

use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Instant;
use std::time::Duration;

/// Name per slot, learned while the device is online.
///
/// Needed because HID++ **does not read the name of whoever does not
/// answer**: without a cache, a device on another channel simply vanished
/// from the API and the GUI went blank -- a regression compared to the
/// solaar backend, which listed offline devices by name.
/// What we remember of each slot. Name is required; battery is the LAST
/// reading, with when it was -- because HID++ does not read the battery of
/// an absent device, and "5% 2 min ago" is worth more than "?" in a tray
/// you glance at.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Known {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_status: Option<String>,
    /// Epoch in seconds of the battery reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_at: Option<u64>,
}

static SEEN: Mutex<Option<BTreeMap<u8, Known>>> = Mutex::new(None);

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Where the cache lives between runs. It is disposable runtime, not
/// config: deleting it loses nothing beyond a re-read.
fn cache_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            std::path::PathBuf::from(home).join(".local/state")
        });
    base.join("oxdesk").join("slots.json")
}

fn load_cache() -> BTreeMap<u8, Known> {
    let Ok(text) = std::fs::read_to_string(cache_path()) else {
        return BTreeMap::new();
    };
    // New format.
    if let Ok(m) = serde_json::from_str::<BTreeMap<String, Known>>(&text) {
        return m
            .into_iter()
            .filter_map(|(k, v)| k.parse::<u8>().ok().map(|s| (s, v)))
            .collect();
    }
    // Old format ({slot: name}): migrates without losing what we knew.
    serde_json::from_str::<BTreeMap<String, String>>(&text)
        .map(|m| {
            m.into_iter()
                .filter_map(|(k, v)| k.parse::<u8>().ok().map(|s| {
                    (s, Known { name: v, battery_percent: None, battery_status: None, battery_at: None })
                }))
                .collect()
        })
        .unwrap_or_default()
}

fn save_cache(m: &BTreeMap<u8, Known>) {
    let as_str: BTreeMap<String, Known> =
        m.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    let Ok(text) = serde_json::to_string(&as_str) else { return };
    let p = cache_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, text);
}

fn with_cache<T>(f: impl FnOnce(&mut BTreeMap<u8, Known>) -> T) -> Option<T> {
    let mut guard = SEEN.lock().ok()?;
    if guard.is_none() {
        *guard = Some(load_cache());
    }
    let m = guard.as_mut()?;
    Some(f(m))
}

/// Stores the name and, when there is one, the battery reading.
fn remember(slot: u8, name: &str, bat: Option<(Option<u8>, Option<String>)>) {
    let changed = with_cache(|m| {
        let e = m.entry(slot).or_insert_with(|| Known {
            name: name.to_string(),
            battery_percent: None,
            battery_status: None,
            battery_at: None,
        });
        let mut dirty = false;
        if e.name != name {
            e.name = name.to_string();
            dirty = true;
        }
        if let Some((pct, lvl)) = bat {
            e.battery_percent = pct;
            e.battery_status = lvl;
            e.battery_at = Some(now_secs());
            // Writes even if the value did not change: what changed is
            // `battery_at`, and it is what the GUI uses to say "70%
            // age=0s" instead of presenting memory as a reading.
            dirty = true;
        }
        dirty
    });
    if changed == Some(true) {
        if let Some(snapshot) = with_cache(|m| m.clone()) {
            save_cache(&snapshot);
        }
    }
}

fn remembered(slot: u8) -> Option<String> {
    with_cache(|m| m.get(&slot).map(|k| k.name.clone())).flatten()
}

fn remembered_full(slot: u8) -> Option<Known> {
    with_cache(|m| m.get(&slot).cloned()).flatten()
}

/// How many receiver slots to survey.
const MAX_SLOTS: u8 = 6;
/// Every how many surveys we do the full one.
///
/// Surveying all 6 slots costs ~1 s just in timeouts of the empty ones.
/// With the cache we know which exist, and survey only those -- but every
/// now and then we need to look at everything, or a device paired later
/// would never show up.
const FULL_SCAN_EVERY: u32 = 10;
static SCANS: Mutex<u32> = Mutex::new(0);
/// Attempts to confirm the device left, after ChangeHost.
const LEAVE_CHECKS: u8 = 8;
/// The device leaves almost instantly; 300 ms was dead waiting.
const LEAVE_WAIT: Duration = Duration::from_millis(120);

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub slot: u8,
    pub name: String,
    /// Empty: HID++ does not report "kind" directly. Kept for compat with
    /// the GUI, which already showed the column.
    pub kind: String,
    pub wpid: String,
    pub hidraw: Option<String>,
    /// `false` when the device is on another Easy-Switch channel.
    /// While offline, **no HID++ command reaches it**.
    pub online: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ping_ms: Option<f64>,
    /// Current channel, 1-based.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_host: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_hosts: Option<u8>,
    /// Percentage, when the device reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_percent: Option<u8>,
    /// Status DECLARED by the device -- do not trust it blindly: an MX
    /// Keys on the cable went from 5% to 50% saying "discharging".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_next_level: Option<u8>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub charging: bool,
    /// Which feature answered (0x1004 or 0x1000), for diagnostics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_source: Option<String>,
    /// Seconds since the battery reading. `0` = now; a high value means
    /// the device is absent and this is memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_age_s: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Receiver {
    pub name: String,
    pub hidraw: Option<String>,
    pub usb_id: Option<String>,
    pub serial: Option<String>,
    pub devices: Vec<Device>,
}

#[derive(Debug, Serialize)]
pub struct Survey {
    /// `uaccess` ACL on /dev/hidraw. Without it nothing works.
    pub hidraw_access: bool,
    /// Kept in the JSON so as not to break the GUI; now it is the real source.
    pub backend: &'static str,
    pub receivers: Vec<Receiver>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Seconds of reading pause left, when paused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paused_s: Option<u64>,
}

/// Do we have access to the HID hardware?
///
/// The name stayed for compatibility with the JSON the GUI reads, but the
/// question is answered by the platform: `uaccess` ACL on `/dev/hidraw`
/// on Linux, opening the receiver on Windows. See `sys::device_access`.
pub fn hidraw_access() -> bool {
    crate::sys::device_access()
}

/// Ping with retry.
///
/// A lost ping does NOT mean absence -- a device in power saving, or with
/// a low battery, overruns the 250 ms and answers on the next attempt.
/// `survey` already had retry; `remap` and `change_host` did not, and
/// failed with "does not answer" seconds after a successful reading.
/// Observed with the devices at 5%.
fn ping_retry(rx: &hidpp::Receiver, slot: u8, attempts: u8) -> Result<f64, String> {
    // Delegates to the crate: one retry implementation for both sides.
    if rx.present(slot, attempts) {
        rx.ping(slot)
    } else {
        Err("no answer after retry".into())
    }
}

fn no_access() -> Survey {
    // Three situations, three different actions. Saying "no access" for
    // all three sent the person to fix permissions when the problem was
    // something else -- which is what happened when the receiver moved to
    // another machine. These messages reach the tray and the panel, so
    // they stay pt-BR.
    #[cfg(target_os = "linux")]
    let error = {
        let (total, openable) = crate::sys::hidraw_count();
        if total == 0 {
            "nenhum no `/dev/hidraw*` neste sistema: nao ha receptor \
             Logitech conectado aqui (ou o kernel nao o enumerou)"
                .to_string()
        } else if openable == 0 {
            format!(
                "{total} no(s) `/dev/hidraw*` existem, mas nenhum abre: e \
                 permissao. Rode `sudo udevadm trigger \
                 --subsystem-match=hidraw --action=add` (a regra do Solaar \
                 so aplica ACL a nos criados depois dela)"
            )
        } else {
            "acesso ao hidraw ok, mas nenhuma interface HID++ respondeu".to_string()
        }
    };
    #[cfg(not(target_os = "linux"))]
    let error = "no access to the raw HID device".to_string();

    Survey {
        hidraw_access: false,
        backend: "hidpp",
        paused_s: None,
        receivers: Vec::new(),
        error: Some(error),
    }
}


/// Full survey, with name and battery.
pub fn survey() -> Survey {
    survey_opts(true)
}

/// What the short cache keeps of each device.
///
/// It was a 4-tuple. It became a struct when the tray needed the battery:
/// the tuple was already at the edge of readable, and `(u8, String,
/// bool, Option<u8>, Option<u8>)` has two `Option<u8>` that only the order
/// tells apart -- swapping channel for battery would compile and lie.
#[derive(Clone)]
pub struct Brief {
    pub slot: u8,
    pub name: String,
    pub online: bool,
    pub current_host: Option<u8>,
    pub battery_percent: Option<u8>,
}

/// Last light survey, so the hot path does not pay ~360 ms.
static LAST: Mutex<Option<(std::time::Instant, Vec<Brief>)>> = Mutex::new(None);

/// Known devices from the short cache.
///
/// Returns `None` if there is no recent reading -- the caller decides
/// whether paying for the survey is worth it.
pub fn cached_devices(max_age: Duration) -> Option<Vec<Brief>> {
    let g = LAST.lock().ok()?;
    let (at, v) = g.as_ref()?;
    (at.elapsed() <= max_age).then(|| v.clone())
}

fn store_last(devs: &[Device]) {
    if let Ok(mut g) = LAST.lock() {
        *g = Some((
            std::time::Instant::now(),
            devs.iter()
                .map(|d| Brief {
                    slot: d.slot,
                    name: d.name.clone(),
                    online: d.online,
                    current_host: d.current_host,
                    battery_percent: d.battery_percent,
                })
                .collect(),
        ));
    }
}

/// Survey from the short cache when fresh; surveys if it is stale.
///
/// Correctness before speed: stale data about who holds the device would
/// lead to trying to switch what we do not have.
pub fn survey_cached_or_fresh(max_age: Duration) -> Survey {
    if let Some(devs) = cached_devices(max_age) {
        return Survey {
            hidraw_access: true,
            backend: "hidpp (cache)",
            paused_s: None,
            receivers: vec![Receiver {
                name: "Receptor Logitech".into(),
                hidraw: None,
                usb_id: None,
                serial: None,
                devices: devs
                    .into_iter()
                    .map(|b| Device {
                        slot: b.slot,
                        name: b.name,
                        kind: String::new(),
                        wpid: String::new(),
                        hidraw: None,
                        online: b.online,
                        ping_ms: None,
                        current_host: b.current_host,
                        num_hosts: None,
                        battery_percent: b.battery_percent,
                        battery_status: None,
                        battery_next_level: None,
                        charging: false,
                        battery_source: None,
                        battery_age_s: None,
                    })
                    .collect(),
            }],
            error: None,
        };
    }
    survey_opts(false)
}

/// Keeps the short cache warm, so the edge does not wait for a survey.
/// When the devices ARRIVED on this machine.
///
/// It exists because of a bug the user felt first-hand: with edges on
/// both machines pointing at each other, arriving on Windows and moving
/// the mouse to the left -- the natural movement, because you came from
/// there -- hits x=0 and the edge sends everything back at once. The log
/// showed the whole cycle in seconds.
///
/// Neither side was wrong in isolation. What was missing was a **grace
/// period**: after the devices arrive, the edge here needs some quiet
/// time for the person to get off the border.
struct Presence {
    here: bool,
    /// Consecutive surveys without seeing anyone.
    misses: u8,
    arrived: Option<Instant>,
}

static PRESENCE: Mutex<Presence> = Mutex::new(Presence { here: false, misses: 0, arrived: None });

/// How many consecutive empty surveys count as "they left".
///
/// One alone does not: `survey` does ONE ping per slot, without retry, and
/// a device in power saving loses pings. Without hysteresis the log fills
/// with ARRIVED/LEFT within seconds -- which is exactly what the user's
/// log showed -- and the edge would flicker between armed and disarmed.
const MISSES_TO_LEAVE: u8 = 2;

/// Are the devices on THIS machine?
pub fn devices_here() -> bool {
    PRESENCE.lock().map(|p| p.here).unwrap_or(false)
}

/// How long ago they arrived here, in ms. `None` = no arrival seen.
pub fn arrived_ms_ago() -> Option<u64> {
    PRESENCE.lock().ok()?.arrived.map(|t| t.elapsed().as_millis() as u64)
}

fn note_presence(survey: &Survey) {
    let seen = survey.receivers.iter().any(|r| r.devices.iter().any(|d| d.online));
    let Ok(mut p) = PRESENCE.lock() else { return };
    if seen {
        p.misses = 0;
        if !p.here {
            p.here = true;
            p.arrived = Some(Instant::now());
            // Transition logged: the edge arms and disarms by this
            // reading, so when it disagrees with what the panel shows the
            // log has to let you see where the disagreement came from.
            say!("presence: devices ARRIVED on this machine");
        }
    } else {
        p.misses = p.misses.saturating_add(1);
        if p.here && p.misses >= MISSES_TO_LEAVE {
            p.here = false;
            say!("presence: devices LEFT this machine");
        }
    }
}

pub fn spawn_refresher() {
    std::thread::spawn(|| loop {
        // The periodic survey already existed to keep the cache warm; it
        // is the natural place to notice arrival, instead of a second
        // poller talking to the same radio.
        note_presence(&survey_opts(false));
        std::thread::sleep(Duration::from_millis(1500));
    });
}

/// `with_battery = false` for the hot path: the battery costs two round
/// trips per device and does not influence the switch decision.
/// Pause of the HID++ reading, for diagnostics.
///
/// ## Why this exists
///
/// The devices went back to channel 1 a few seconds after going to 2,
/// with edge and reconciler off on all three machines and NO log line
/// claiming the action. With all our code that switches channels ruled
/// out (only the edge and the API call `focus::execute`), two hypotheses
/// were left: our READING disturbing the radio, or the hardware.
///
/// Telling them apart would require taking the user's mouse and keyboard
/// for a few minutes. Instead of asking for that, the button stays with
/// him: with the reading paused, if the switch holds the fault is ours;
/// if it keeps going back, it is not.
///
/// The pause only applies to the periodic PROBING. `change_host` keeps
/// working -- without it there is no way to run the test.
///
/// In memory and not in the config: a pause is diagnostic state, and one
/// that survived a restart would be a trap for next month.
static PAUSE: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// Pauses for `minutes` (0 resumes). Returns the seconds left.
pub fn pause(minutes: u64) -> Option<u64> {
    let mut g = PAUSE.lock().unwrap_or_else(|e| e.into_inner());
    *g = (minutes > 0)
        .then(|| std::time::Instant::now() + Duration::from_secs(minutes * 60));
    pause_left(&g)
}

fn pause_left(g: &Option<std::time::Instant>) -> Option<u64> {
    let until = (*g)?;
    let now = std::time::Instant::now();
    (until > now).then(|| (until - now).as_secs())
}

/// Seconds of pause left, or `None` if reading is free.
pub fn pause_remaining_s() -> Option<u64> {
    let g = PAUSE.lock().unwrap_or_else(|e| e.into_inner());
    pause_left(&g)
}

/// What to return while reading is paused: the last known snapshot,
/// without touching the radio.
///
/// Returning empty would make the panel say "no device", which is a
/// different lie from the one we are investigating.
fn survey_paused(remaining: u64) -> Survey {
    let devs = cached_devices(Duration::MAX).unwrap_or_default();
    Survey {
        hidraw_access: true,
        backend: "hidpp (pausado)",
        paused_s: Some(remaining),
        error: None,
        receivers: vec![Receiver {
            name: "Receptor Logitech".into(),
            hidraw: None,
            usb_id: None,
            serial: None,
            devices: devs
                .into_iter()
                .map(|b| Device {
                    slot: b.slot,
                    name: b.name,
                    kind: String::new(),
                    wpid: String::new(),
                    hidraw: None,
                    online: b.online,
                    ping_ms: None,
                    current_host: b.current_host,
                    num_hosts: None,
                    battery_percent: b.battery_percent,
                    battery_status: None,
                    battery_next_level: None,
                    charging: false,
                    battery_source: None,
                    battery_age_s: None,
                })
                .collect(),
        }],
    }
}

pub fn survey_opts(with_battery: bool) -> Survey {
    // Pause BEFORE any hardware access: the point is not to touch the
    // radio, and every reading path goes through here -- including the
    // open panel, which on its own probes every 10 s.
    if let Some(remaining) = pause_remaining_s() {
        return survey_paused(remaining);
    }
    if !hidraw_access() {
        return no_access();
    }
    let api = match hidpp::hidapi::HidApi::new() {
        Ok(a) => a,
        Err(e) => {
            return Survey {
                hidraw_access: true,
                backend: "hidpp",
                receivers: Vec::new(),
                paused_s: None,
                error: Some(format!("hidapi: {e}")),
            }
        }
    };
    let rx = match hidpp::Receiver::open(&api) {
        Ok(r) => r,
        Err(e) => {
            return Survey {
                hidraw_access: true,
                backend: "hidpp",
                receivers: Vec::new(),
                paused_s: None,
                error: Some(e),
            }
        }
    };

    // A single lost ping does NOT mean absence: a device in power saving
    // can overrun the 250 ms. Observed in real use -- the keyboard showed
    // offline in one reading and online in the next, steady at 26 ms.
    // Retry only for slots with a known name (i.e. paired): an empty slot
    // keeps failing fast.
    // Names are NOT re-read when already cached: they were ~1.1 s of the
    // 1.6 s of a survey, spent finding out what we already knew.
    let known: Vec<u8> = (1..=MAX_SLOTS).filter(|s| remembered(*s).is_some()).collect();
    let n = {
        let mut g = SCANS.lock().unwrap_or_else(|e| e.into_inner());
        *g = g.wrapping_add(1);
        *g
    };
    let full = known.is_empty() || n % FULL_SCAN_EVERY == 0;
    let slots: Vec<u8> = if full { (1..=MAX_SLOTS).collect() } else { known };
    let need_names = slots.iter().any(|s| remembered(*s).is_none());

    let mut raw: Vec<hidpp::Device> = slots
        .iter()
        .map(|&slot| match rx.ping(slot) {
            Ok(ms) => hidpp::Device {
                slot,
                name: if need_names { rx.device_name(slot).ok() } else { None },
                online: true,
                reply_ms: Some((ms * 10.0).round() / 10.0),
                host: rx.host_info(slot).ok(),
                battery: if with_battery { rx.battery(slot) } else { None },
            },
            Err(_) => hidpp::Device {
                slot,
                name: None,
                online: false,
                reply_ms: None,
                host: None,
                battery: None,
            },
        })
        .collect();
    for d in raw.iter_mut() {
        if d.online || remembered(d.slot).is_none() {
            continue;
        }
        std::thread::sleep(Duration::from_millis(150));
        if let Ok(ms) = rx.ping(d.slot) {
            d.online = true;
            d.reply_ms = Some((ms * 10.0).round() / 10.0);
            d.host = rx.host_info(d.slot).ok();
            if d.name.is_none() {
                d.name = rx.device_name(d.slot).ok();
            }
        }
    }

    let devices: Vec<Device> = raw
        .into_iter()
        .filter_map(|d| {
            // Offline with no known name = probably an empty slot: we do
            // not pollute the list with the receiver's 6 slots.
            let name = d.name.clone().or_else(|| remembered(d.slot))?;
            // Saving the battery must NOT depend on having read the name
            // now: with the name cached (the normal case) we never saved,
            // and the file stayed stuck in the old format.
            let bat = d
                .battery
                .as_ref()
                .map(|b| (b.percent, b.status.map(String::from)));
            if bat.is_some() || d.name.is_some() {
                remember(d.slot, &name, bat);
            }
            Some((d, name))
        })
        .map(|(d, name)| {
            // No reading now? Use the last known one, declaring its age.
            let cached = remembered_full(d.slot);
            let (pct, lvl, age) = match d.battery.as_ref() {
                Some(b) => (b.percent, b.status.map(String::from), Some(0)),
                None => (
                    cached.as_ref().and_then(|k| k.battery_percent),
                    cached.as_ref().and_then(|k| k.battery_status.clone()),
                    cached
                        .as_ref()
                        .and_then(|k| k.battery_at)
                        .map(|t| now_secs().saturating_sub(t)),
                ),
            };
            Device {
            slot: d.slot,
            battery_percent: pct,
            battery_status: lvl,
            battery_next_level: d.battery.as_ref().and_then(|b| b.next_level),
            charging: d.battery.as_ref().is_some_and(|b| b.charging),
            battery_source: d.battery.as_ref().map(|b| b.source.to_string()),
            battery_age_s: age,
            name,
            kind: String::new(),
            wpid: String::new(),
            hidraw: Some(rx.path.clone()),
            online: d.online,
            ping_ms: d.reply_ms,
            current_host: d.host.map(|h| h.current),
            num_hosts: d.host.map(|h| h.num_hosts),
            }
        })
        .collect();

    store_last(&devices);
    Survey {
        hidraw_access: true,
        backend: "hidpp",
        paused_s: None,
        receivers: vec![Receiver {
            name: format!("Receptor Logitech {:04x}", rx.product_id),
            hidraw: Some(rx.path.clone()),
            usb_id: Some(format!("046d:{:04X}", rx.product_id)),
            serial: None,
            devices,
        }],
        error: None,
    }
}

/// Channel switch WITHOUT waiting for confirmation.
///
/// For the path the user feels: he perceives the switch when the device
/// actually changes radio, not when we confirm. The check becomes a
/// background log line.
///
/// We still ping once first -- it is the only way to know we hold the
/// device. Without it `set_host` treats "no answer" as success (because in
/// a real switch the radio drops before the reply) and a no-op would look
/// like it worked.
pub fn change_host_now(slot: u8, host: u8) -> Result<(), String> {
    if host == 0 {
        return Err("channel is 1-based".into());
    }
    let api = hidpp::hidapi::HidApi::new().map_err(|e| format!("hidapi: {e}"))?;
    let rx = hidpp::Receiver::open(&api)?;
    rx.ping(slot)
        .map_err(|_| format!("slot {slot} does not answer -- we do not hold this device"))?;
    rx.set_host_nowait(slot, host)
}

/// Confirms, in the background, that the device left. Only logs.
pub fn verify_left(slot: u8, host: u8) {
    std::thread::spawn(move || {
        let Ok(api) = hidpp::hidapi::HidApi::new() else { return };
        let Ok(rx) = hidpp::Receiver::open(&api) else { return };
        for i in 1..=LEAVE_CHECKS {
            std::thread::sleep(LEAVE_WAIT);
            if !rx.present(slot, 1) {
                say!(
                    "check: slot {slot} left (channel {host}) in {}ms",
                    i as u32 * LEAVE_WAIT.as_millis() as u32
                );
                return;
            }
        }
        gripe!("check: slot {slot} STILL answers -- the switch to channel {host} did not take");
    });
}

/// Sends the device in slot `slot` to channel `host` (1-based).
///
/// IRREVERSIBLE FROM THIS SIDE: once it switches, this receiver no longer
/// reaches it. The pre-flight has to run BEFORE.
///
/// We verify by EFFECT -- the device stops answering --, not by return
/// code. With solaar this was mandatory because it hung and lied; over
/// HID++ no answer is already the expected case, and the ping confirms in
/// ~25 ms.
pub fn change_host(slot: u8, host: u8) -> Result<String, String> {
    if host == 0 {
        return Err("channel is 1-based".into());
    }
    let api = hidpp::hidapi::HidApi::new().map_err(|e| format!("hidapi: {e}"))?;
    let rx = hidpp::Receiver::open(&api)?;

    if ping_retry(&rx, slot, 3).is_err() {
        return Err(format!(
            "slot {slot} does not answer on this receiver (3 attempts) -- HID++ \
             does not reach a device on another channel"
        ));
    }

    rx.set_host(slot, host)?;

    for attempt in 1..=LEAVE_CHECKS {
        std::thread::sleep(LEAVE_WAIT);
        if rx.ping(slot).is_err() {
            return Ok(format!(
                "slot {slot} left this receiver (channel {host}), confirmed in {}ms",
                attempt as u32 * LEAVE_WAIT.as_millis() as u32
            ));
        }
    }
    Err(format!(
        "slot {slot} still answers here after the command -- the switch \
         to channel {host} did not happen"
    ))
}


#[derive(Debug, Serialize)]
pub struct ControlInfo {
    pub cid: String,
    pub name: &'static str,
    pub reprogrammable: bool,
    pub divertable: bool,
    pub flags: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remapped_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remapped_to_name: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct DeviceControls {
    pub slot: u8,
    pub device: String,
    pub controls: Vec<ControlInfo>,
}

#[derive(Debug, Serialize)]
pub struct Controls {
    pub devices: Vec<DeviceControls>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn open_rx<'a>(api: &'a hidpp::hidapi::HidApi) -> Result<hidpp::Receiver, String> {
    hidpp::Receiver::open(api)
}

/// Controls of each ONLINE device.
///
/// An absent device does not show up: HID++ does not reach whoever is on
/// another channel, and listing a name without being able to read or
/// write would be misleading. The panel shows this, so its text stays
/// pt-BR.
pub fn controls() -> Controls {
    let api = match hidpp::hidapi::HidApi::new() {
        Ok(a) => a,
        Err(e) => return Controls { devices: vec![], error: Some(format!("hidapi: {e}")) },
    };
    let rx = match open_rx(&api) {
        Ok(r) => r,
        Err(e) => return Controls { devices: vec![], error: Some(e) },
    };

    let mut out = Vec::new();
    for slot in 1..=MAX_SLOTS {
        if ping_retry(&rx, slot, 2).is_err() {
            continue;
        }
        let name = remembered(slot)
            .or_else(|| rx.device_name(slot).ok())
            .unwrap_or_else(|| format!("slot {slot}"));
        match rx.controls(slot) {
            Ok(cs) => out.push(DeviceControls {
                slot,
                device: name,
                controls: cs
                    .into_iter()
                    .map(|c| ControlInfo {
                        cid: format!("0x{:04x}", c.cid),
                        name: hidpp::control_name(c.cid),
                        reprogrammable: c.reprogrammable,
                        divertable: c.divertable,
                        flags: format!("0x{:02x}", c.flags),
                        remapped_to: c.remapped_to.map(|t| format!("0x{t:04x}")),
                        remapped_to_name: c.remapped_to.map(hidpp::control_name),
                    })
                    .collect(),
            }),
            Err(e) => out.push(DeviceControls {
                slot,
                device: format!("{name} (error: {e})"),
                controls: vec![],
            }),
        }
    }
    Controls { devices: out, error: None }
}

/// Applies a remap. `to = 0` goes back to the default. The panel shows
/// the result, so its text stays pt-BR.
pub fn remap(slot: u8, cid: u16, to: u16) -> Result<String, String> {
    let api = hidpp::hidapi::HidApi::new().map_err(|e| format!("hidapi: {e}"))?;
    let rx = open_rx(&api)?;
    if ping_retry(&rx, slot, 4).is_err() {
        return Err(format!(
            "slot {slot} nao responde (4 tentativas) -- o dispositivo esta em \
             outro canal, ou com bateria baixa demais para responder"
        ));
    }
    // Restoring the default is remapping the control TO ITSELF, not to
    // zero. Measured: with `to = 0` the write returns ok and the device
    // keeps reporting the old remap.
    let target = if to == 0 { cid } else { to };
    rx.set_control_remap(slot, cid, target)?;
    Ok(format!(
        "0x{cid:04x} ({}) -> {}",
        hidpp::control_name(cid),
        if target == cid {
            "padrao".to_string()
        } else {
            format!("0x{target:04x} ({})", hidpp::control_name(target))
        }
    ))
}

#[cfg(test)]
mod pause_tests {
    use super::*;

    #[test]
    fn pause_and_resume() {
        // Serialized with the test below by the global state itself: both
        // touch the same `PAUSE`, so each cleans up at the end.
        assert!(pause(1).is_some_and(|s| s > 0 && s <= 60));
        assert!(pause_remaining_s().is_some());
        assert_eq!(pause(0), None, "zero resumes");
        assert_eq!(pause_remaining_s(), None);
    }

    #[test]
    fn expired_pause_no_longer_counts() {
        // A deadline in the past counts as free: the pause expires on its
        // own, without anyone having to remember to resume -- a
        // diagnostic left on by mistake would be worse than the bug it
        // investigates.
        {
            let mut g = PAUSE.lock().unwrap_or_else(|e| e.into_inner());
            *g = Some(std::time::Instant::now() - Duration::from_secs(1));
        }
        assert_eq!(pause_remaining_s(), None);
        pause(0);
    }
}
