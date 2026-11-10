//! HID++ 2.0 over raw HID -- enough to inventory Logitech devices and
//! switch the Easy-Switch channel, on Linux and Windows.
//!
//! Spec: ../../specs/005-input-switch.md
//!
//! Replaces the `solaar` backend on Linux. Measured: `solaar show` costs
//! **5.9 to 15.3 s**, and `change_host` called it 4x per device -- one
//! switch took 105 s. Over HID++ directly, one call costs **26 to 90 ms**.
//!
//! ## Protocol, in short
//!
//! ```text
//! request (report 0x11, 20 bytes):
//!   [0x11, device_index, feature_index, func<<4 | sw_id, p0..p16]
//! reply: same echo of device_index/feature_index/func
//! error: feature_index = 0xFF, code in byte 4
//! ```
//!
//! Feature 0x0000 (ROOT) is ALWAYS at index 0; through it you discover
//! the index of any other feature, which varies per device.
//!
//! ## Which endpoint to open
//!
//! The receiver exposes one HID collection per report type (usage 1 =
//! short 0x10, 2 = long 0x11, 4 = very long). We ALWAYS use the long one
//! on **usage 2**, to write and read on the same collection -- the
//! alternative means opening two and correlating between them.
//!
//! On Linux each paired device has its own hidraw node, but it answers
//! on the **receiver slot index** (measured: /dev/hidraw2 answers on
//! idx=1), not on 0xFF. Since the receiver node answers every slot, that
//! is the one we open.

pub mod protocol;

/// Re-exported so consumers of this crate need not declare the
/// dependency again -- and to guarantee the SAME version everywhere.
pub use hidapi;

use hidapi::{HidApi, HidDevice};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Feature index by (slot, feature). Stable per device, and discovering
/// it costs a round trip -- on the hot path that matters.
static FEATURE_CACHE: Mutex<Option<BTreeMap<(u8, u16), u8>>> = Mutex::new(None);
use std::time::{Duration, Instant};

pub const LOGITECH_VID: u16 = 0x046D;
/// Vendor-defined usage page where Logitech exposes HID++.
pub const HIDPP_USAGE_PAGE: u16 = 0xFF00;
/// Collection of the long report (0x11).
const USAGE_LONG: u16 = 0x02;

const REPORT_LONG: u8 = 0x11;
const LONG_LEN: usize = 20;
/// Marks OUR requests in the replies, telling them apart from
/// spontaneous device notifications. 1..15; 0 is reserved.
const SW_ID: u8 = 0x0A;

pub const FEATURE_ROOT: u16 = 0x0000;
pub const FEATURE_DEVICE_NAME: u16 = 0x0005;
pub const FEATURE_CHANGE_HOST: u16 = 0x1814;
/// Modern battery, in percent. The MX Anywhere 3 has this one.
/// Reprogrammable controls. This is how buttons are remapped.
pub const FEATURE_REPROG_CONTROLS: u16 = 0x1B04;
pub const FEATURE_UNIFIED_BATTERY: u16 = 0x1004;
/// Old battery, in bands. The MX Keys has this one -- hence both.
pub const FEATURE_BATTERY_STATUS: u16 = 0x1000;

const REPLY_TIMEOUT: Duration = Duration::from_millis(700);
/// Presence ping timeout. Measured: a present device answers in 26 to
/// 90 ms. 250 ms is generous and keeps a scan of 6 empty slots at
/// ~1.5 s instead of 4.4 s.
const PING_TIMEOUT: Duration = Duration::from_millis(250);
const READ_SLICE: i32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostInfo {
    pub num_hosts: u8,
    /// 1-based, as the config and Solaar show it.
    pub current: u8,
}

/// Battery. The two features report differently, and devices of the
/// same family use different features -- measured: the MX Anywhere 3
/// exposes 0x1004 (percent) and the MX Keys 0x1000 (band).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Battery {
    /// Percent, when the device reports it.
    pub percent: Option<u8>,
    /// Status **as declared by the device**, not inferred by us.
    ///
    /// Worth distrusting: measured, an MX Keys on the cable went from 5%
    /// to 50% reporting `descarregando` (discharging) the whole time --
    /// and `upower`, which reads through the kernel driver, reports the
    /// same. The lie is the firmware's; the field name makes clear whose
    /// claim it is.
    pub status: Option<&'static str>,
    /// Next discharge step the device predicts (0x1000).
    pub next_level: Option<u8>,
    /// Charging. For 0x1004 we use chargingStatus **or** externalPower,
    /// which is more reliable than the former alone.
    pub charging: bool,
    /// Which feature answered, so diagnosis does not become guesswork.
    pub source: &'static str,
}

#[derive(Debug, Clone)]
pub struct Device {
    /// Slot on the receiver, 1..6.
    pub slot: u8,
    /// Name read from the device itself (feature 0x0005). `None` if it
    /// does not expose the feature.
    pub name: Option<String>,
    pub online: bool,
    #[allow(dead_code)]
    pub reply_ms: Option<f64>,
    pub host: Option<HostInfo>,
    pub battery: Option<Battery>,
}

pub struct Receiver {
    dev: HidDevice,
    pub path: String,
    pub product_id: u16,
}

impl Receiver {
    /// Every plugged-in HID++ receiver, not just the first.
    ///
    /// Exists because the user put TWO receivers on the same machine
    /// (moved the Ubuntu one to Windows) and `open` silently picked one:
    /// the agent listed the slots of one receiver and reported the
    /// devices of the other as absent. The panel stayed consistent with
    /// itself and wrong about the world.
    ///
    /// Stable order by device path, so two consecutive reads do not swap
    /// the receivers' places in the list.
    pub fn open_all(api: &HidApi) -> Vec<Self> {
        let mut paths: Vec<_> = api
            .device_list()
            .filter(|d| {
                d.vendor_id() == LOGITECH_VID
                    && d.usage_page() == HIDPP_USAGE_PAGE
                    && d.usage() == USAGE_LONG
                    && (0xC500..=0xC5FF).contains(&d.product_id())
            })
            .map(|d| (d.path().to_string_lossy().to_string(), d.product_id()))
            .collect();
        paths.sort();
        paths
            .into_iter()
            .filter_map(|(path, product_id)| {
                let dev = api.open_path(&std::ffi::CString::new(path.clone()).ok()?).ok()?;
                dev.set_blocking_mode(false).ok()?;
                Some(Self { dev, path, product_id })
            })
            .collect()
    }

    /// Opens the long-report collection of the first Logitech receiver.
    pub fn open(api: &HidApi) -> Result<Self, String> {
        let info = api
            .device_list()
            .filter(|d| {
                d.vendor_id() == LOGITECH_VID
                    && d.usage_page() == HIDPP_USAGE_PAGE
                    && d.usage() == USAGE_LONG
            })
            // The receiver has a C5xx-family PID; we prefer it over the
            // node of an individual device, because it answers every slot.
            .min_by_key(|d| if (0xC500..=0xC5FF).contains(&d.product_id()) { 0 } else { 1 })
            .ok_or("no Logitech HID++ interface (vid 046d, usage page ff00, usage 2)")?;
        let path = info.path().to_string_lossy().to_string();
        let product_id = info.product_id();
        let dev = info
            .open_device(api)
            .map_err(|e| format!("abrir {path}: {e}"))?;
        dev.set_blocking_mode(false)
            .map_err(|e| format!("non-blocking mode: {e}"))?;
        Ok(Self { dev, path, product_id })
    }

    fn request(
        &self,
        slot: u8,
        feature_index: u8,
        func: u8,
        params: &[u8],
    ) -> Result<(Vec<u8>, f64), String> {
        self.request_to(slot, feature_index, func, params, REPLY_TIMEOUT)
    }

    fn request_to(
        &self,
        slot: u8,
        feature_index: u8,
        func: u8,
        params: &[u8],
        timeout: Duration,
    ) -> Result<(Vec<u8>, f64), String> {
        let head = (func << 4) | SW_ID;
        let mut out = [0u8; LONG_LEN];
        out[0] = REPORT_LONG;
        out[1] = slot;
        out[2] = feature_index;
        out[3] = head;
        let n = params.len().min(LONG_LEN - 4);
        out[4..4 + n].copy_from_slice(&params[..n]);

        let t0 = Instant::now();
        self.dev.write(&out).map_err(|e| format!("write: {e}"))?;

        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; 64];
        while Instant::now() < deadline {
            let got = self
                .dev
                .read_timeout(&mut buf, READ_SLICE)
                .map_err(|e| format!("read: {e}"))?;
            if got < 5 {
                continue;
            }
            let r = &buf[..got];
            if r[1] != slot {
                continue; // reply from another device on the same receiver
            }
            if r[2] == 0xFF {
                return Err(format!("HID++ error 0x{:02x} (func 0x{func:02x})", r[4]));
            }
            if r[2] == feature_index && r[3] == head {
                return Ok((r[4..].to_vec(), t0.elapsed().as_secs_f64() * 1000.0));
            }
        }
        Err("no answer".into())
    }

    /// ROOT.getFeature -- index of the feature on this device.
    ///
    /// Memoized: the index does not change for a given device, and on
    /// the hot path every round trip avoided is ~25 ms.
    pub fn feature_index(&self, slot: u8, feature: u16) -> Result<u8, String> {
        if let Ok(g) = FEATURE_CACHE.lock() {
            if let Some(m) = g.as_ref() {
                if let Some(&i) = m.get(&(slot, feature)) {
                    return Ok(i);
                }
            }
        }
        let idx = self.feature_index_uncached(slot, feature)?;
        if let Ok(mut g) = FEATURE_CACHE.lock() {
            g.get_or_insert_with(BTreeMap::new).insert((slot, feature), idx);
        }
        Ok(idx)
    }

    fn feature_index_uncached(&self, slot: u8, feature: u16) -> Result<u8, String> {
        let p = [(feature >> 8) as u8, (feature & 0xFF) as u8, 0];
        let (r, _) = self.request(slot, 0x00, 0x00, &p)?;
        let idx = *r.first().ok_or("empty answer from getFeature")?;
        if idx == 0 {
            return Err(format!("feature 0x{feature:04x} not supported"));
        }
        Ok(idx)
    }

    /// Does the device on this slot answer? Returns the latency.
    ///
    /// This is the "online" test: ~26 to 90 ms, versus 5.9 s for a
    /// `solaar show`.
    pub fn ping(&self, slot: u8) -> Result<f64, String> {
        let p = [
            (FEATURE_CHANGE_HOST >> 8) as u8,
            (FEATURE_CHANGE_HOST & 0xFF) as u8,
            0,
        ];
        let (_, ms) = self.request_to(slot, 0x00, 0x00, &p, PING_TIMEOUT)?;
        Ok(ms)
    }

    /// Raw call, to probe a feature whose layout is not known yet.
    ///
    /// This is how 0x1815 (HOSTS_INFO) was decoded: print the bytes and
    /// build the parser on the real reply, instead of guessing the layout.
    pub fn raw(
        &self,
        slot: u8,
        feature: u16,
        func: u8,
        params: &[u8],
    ) -> Result<Vec<u8>, String> {
        let fi = self.feature_index(slot, feature)?;
        self.request(slot, fi, func, params).map(|(r, _)| r)
    }

    /// Is the device present? With retry.
    ///
    /// Use THIS for presence, not `host_info`: that one costs two round
    /// trips with a 700 ms timeout each, and a single lost ping does not
    /// mean absence. Measured in real use: with the devices at 5%
    /// battery, the check without retry reported "no device on this
    /// receiver" 25 times in 5 minutes while the user was moving the
    /// mouse on that machine -- and the edge did not work.
    pub fn present(&self, slot: u8, attempts: u8) -> bool {
        for i in 0..attempts.max(1) {
            if i > 0 {
                std::thread::sleep(Duration::from_millis(160));
            }
            if self.ping(slot).is_ok() {
                return true;
            }
        }
        false
    }

    /// Number of hosts and current channel (1-based).
    ///
    /// **Validates the range before returning.** 0x1814 answers garbage
    /// during reconnection: the monitor logged an MX Keys -- which has
    /// three hosts -- declaring `current_host = 5`. An impossible number,
    /// and returning it as fact made everything that reads the channel
    /// decide on fiction. The reconciler acted on it.
    ///
    /// Out of range becomes an error, not a number: callers already know
    /// how to handle "don't know" (`survey` puts `None`), and do not know
    /// how to handle a channel that does not exist.
    pub fn host_info(&self, slot: u8) -> Result<HostInfo, String> {
        let fi = self.feature_index(slot, FEATURE_CHANGE_HOST)?;
        let (r, _) = self.request(slot, fi, 0x00, &[0, 0, 0])?;
        if r.len() < 2 {
            return Err("short answer from getHostInfo".into());
        }
        let num_hosts = r[0];
        let current = r[1] + 1;
        // A device with no host at all does not exist, and a channel above
        // the total it declared itself is a corrupted reply.
        if num_hosts == 0 || num_hosts > 16 || current == 0 || current > num_hosts {
            return Err(format!(
                "getHostInfo incoerente: canal {current} de {num_hosts} host(s)"
            ));
        }
        Ok(HostInfo { num_hosts, current })
    }

    /// Device name via feature 0x0005, read in chunks.
    ///
    /// Resolves by NAME instead of slot number: the slot is per receiver
    /// and **does not match across machines** (measured -- on Windows
    /// slot 2 was empty while here it exists).
    pub fn device_name(&self, slot: u8) -> Result<String, String> {
        let fi = self.feature_index(slot, FEATURE_DEVICE_NAME)?;
        let (r, _) = self.request(slot, fi, 0x00, &[0, 0, 0])?;
        let len = *r.first().ok_or("getCount came back empty")? as usize;
        let mut name = String::new();
        let mut at = 0usize;
        while at < len && at < 64 {
            let (chunk, _) = self.request(slot, fi, 0x01, &[at as u8, 0, 0])?;
            for &b in &chunk {
                if name.len() >= len {
                    break;
                }
                if b == 0 {
                    break;
                }
                name.push(b as char);
            }
            if chunk.is_empty() {
                break;
            }
            at += chunk.len().min(16);
        }
        let trimmed = name.trim().to_string();
        if trimmed.is_empty() {
            Err("empty name".into())
        } else {
            Ok(trimmed)
        }
    }

    /// Battery, trying 0x1004 (percent) and falling back to 0x1000 (band).
    pub fn battery(&self, slot: u8) -> Option<Battery> {
        // 0x1004 UNIFIED_BATTERY, func 1 = getStatus
        if let Ok(fi) = self.feature_index(slot, FEATURE_UNIFIED_BATTERY) {
            if let Ok((r, _)) = self.request(slot, fi, 0x01, &[0, 0, 0]) {
                if r.len() >= 3 {
                    // r[0]=stateOfCharge(%), r[1]=level, r[2]=chargingStatus,
                    // r[3]=externalPowerStatus. Measured on the MX Anywhere 3
                    // on the cable: 05 01 01 01.
                    // The status strings stay pt-BR: they reach the panel.
                    let ext = r.get(3).copied().unwrap_or(0);
                    return Some(Battery {
                        percent: Some(r[0]),
                        status: Some(if r[2] != 0 || ext != 0 {
                            "carregando"
                        } else {
                            "descarregando"
                        }),
                        next_level: None,
                        charging: r[2] != 0 || ext != 0,
                        source: "0x1004",
                    });
                }
            }
        }
        // 0x1000 BATTERY_STATUS, func 0 = getBatteryLevelStatus
        let fi = self.feature_index(slot, FEATURE_BATTERY_STATUS).ok()?;
        let (r, _) = self.request(slot, fi, 0x00, &[0, 0, 0]).ok()?;
        if r.len() < 3 {
            return None;
        }
        // r[0]=discharge level (%), r[1]=next step, r[2]=status.
        // Measured on the MX Keys while charging: 32 14 00 -- 50%, next
        // 20%, and status 0 ("descarregando", discharging) despite being
        // on the cable.
        let status = match r[2] {
            0 => "discharging",
            1 => "charging",
            2 => "almost full",
            3 => "full",
            4 => "slow charge",
            5 => "invalid",
            6 => "error",
            _ => "unknown",
        };
        Some(Battery {
            percent: if r[0] > 0 { Some(r[0]) } else { None },
            status: Some(status),
            next_level: if r[1] > 0 { Some(r[1]) } else { None },
            charging: matches!(r[2], 1 | 2),
            source: "0x1000",
        })
    }

    /// Channel switch WITHOUT waiting for a reply.
    ///
    /// On a successful switch the device leaves the radio **before**
    /// replying, so waiting is waiting for nothing: measured, the 700 ms
    /// `REPLY_TIMEOUT` was paid in full on every switch. Here we write
    /// the report and return.
    ///
    /// With the feature index memoized, this costs one write -- on the
    /// order of 1 ms instead of 700.
    pub fn set_host_nowait(&self, slot: u8, host: u8) -> Result<(), String> {
        if host == 0 {
            return Err("host e 1-based".into());
        }
        let fi = self.feature_index(slot, FEATURE_CHANGE_HOST)?;
        let head = (0x01u8 << 4) | SW_ID;
        let mut out = [0u8; LONG_LEN];
        out[0] = REPORT_LONG;
        out[1] = slot;
        out[2] = fi;
        out[3] = head;
        out[4] = host - 1;
        self.dev.write(&out).map(|_| ()).map_err(|e| format!("write: {e}"))
    }

    /// CHANGE_HOST.setCurrentHost. `host` is 1-based.
    ///
    /// IRREVERSIBLE: the device leaves this receiver and no longer answers
    /// here. **Do not wait for a reply** -- the radio drops before it.
    pub fn set_host(&self, slot: u8, host: u8) -> Result<(), String> {
        if host == 0 {
            return Err("host e 1-based".into());
        }
        let fi = self.feature_index(slot, FEATURE_CHANGE_HOST)?;
        match self.request(slot, fi, 0x01, &[host - 1, 0, 0]) {
            Ok(_) => Ok(()),
            // No reply is the EXPECTED case here.
            Err(e) if e == "no answer" => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Scans slots 1..=max.
    ///
    /// `with_names` and `with_battery` are optional because **they cost
    /// round trips**: the name is ~2 requests and the battery 2 more.
    /// With the name cached on the caller's side, asking again is pure
    /// waste -- it was 1.1 s of a 1.6 s scan.
    pub fn devices(&self, max: u8, with_names: bool, with_battery: bool) -> Vec<Device> {
        (1..=max)
            .map(|slot| match self.ping(slot) {
                Ok(ms) => Device {
                    slot,
                    name: if with_names { self.device_name(slot).ok() } else { None },
                    online: true,
                    reply_ms: Some((ms * 10.0).round() / 10.0),
                    host: self.host_info(slot).ok(),
                    battery: if with_battery { self.battery(slot) } else { None },
                },
                Err(_) => Device {
                    slot,
                    name: None,
                    online: false,
                    reply_ms: None,
                    host: None,
                    battery: None,
                },
            })
            .collect()
    }
}

/// Readable name of a controlID. Partial table, MX families.
///
/// The numbers come from the hardware (0x1B04 func 1); the names match
/// what Solaar showed for the same devices. The names stay pt-BR: they
/// are shown in the panel.
pub fn control_name(cid: u16) -> &'static str {
    match cid {
        0x0050 => "Botao esquerdo",
        0x0051 => "Botao direito",
        0x0052 => "Botao do meio",
        0x0053 => "Voltar",
        0x0056 => "Avancar",
        0x005b => "Gesto",
        0x006e => "Show desktop",
        0x00c3 => "Roda horizontal",
        0x00c4 => "Smart Shift",
        0x00c7 => "Easy-Switch 1",
        0x00c8 => "Easy-Switch 2",
        0x00d1 => "Brilho -",
        0x00d2 => "Brilho +",
        0x00d3 => "Mission Control / Task View",
        0x00d7 => "Botao de gesto virtual",
        0x00e0 => "Easy-Switch 3",
        0x00e1 => "Backlight -",
        0x00e2 => "Backlight +",
        0x00e3 => "Previous track",
        _ => "unknown",
    }
}

#[derive(Debug, Clone)]
pub struct Control {
    pub cid: u16,
    pub tid: u16,
    pub flags: u8,
    /// Can take another function (flag 0x10).
    pub reprogrammable: bool,
    /// Can be diverted to software (flag 0x20).
    pub divertable: bool,
    /// Which CID it is mapped to now, if the device reports it.
    pub remapped_to: Option<u16>,
}

impl Receiver {
    /// Lists the device's reprogrammable controls.
    pub fn controls(&self, slot: u8) -> Result<Vec<Control>, String> {
        let fi = self.feature_index(slot, FEATURE_REPROG_CONTROLS)?;
        let (r, _) = self.request(slot, fi, 0x00, &[0, 0, 0])?;
        let count = *r.first().ok_or("getCount came back empty")?;
        let mut out = Vec::new();
        for i in 0..count {
            let (c, _) = self.request(slot, fi, 0x01, &[i, 0, 0])?;
            if c.len() < 5 {
                continue;
            }
            let cid = u16::from_be_bytes([c[0], c[1]]);
            let tid = u16::from_be_bytes([c[2], c[3]]);
            let flags = c[4];
            // func 2 = getControlReporting. Not all answer; absence is
            // not an error, it just means "no remap declared".
            let remapped_to = self
                .request(slot, fi, 0x02, &[c[0], c[1], 0])
                .ok()
                .and_then(|(rr, _)| {
                    (rr.len() >= 5).then(|| u16::from_be_bytes([rr[3], rr[4]]))
                })
                .filter(|&to| to != 0 && to != cid);
            out.push(Control {
                cid,
                tid,
                flags,
                reprogrammable: flags & 0x10 != 0,
                divertable: flags & 0x20 != 0,
                remapped_to,
            });
        }
        Ok(out)
    }

    /// Remaps a control to act as another.
    ///
    /// `to` = 0 restores the default behavior. Refuses a control the
    /// device itself says is not reprogrammable -- the left button, for
    /// example, reports flags=0x01 without bit 0x10.
    pub fn set_control_remap(&self, slot: u8, cid: u16, to: u16) -> Result<(), String> {
        let ctl = self
            .controls(slot)?
            .into_iter()
            .find(|c| c.cid == cid)
            .ok_or_else(|| format!("control 0x{cid:04x} does not exist on this device"))?;
        if !ctl.reprogrammable {
            return Err(format!(
                "0x{cid:04x} ({}) is not reprogrammable: the device reports \
                 flags=0x{:02x}, without bit 0x10",
                control_name(cid),
                ctl.flags
            ));
        }
        let fi = self.feature_index(slot, FEATURE_REPROG_CONTROLS)?;
        // func 3 = setControlReporting(cid, flags, remap)
        // flags bit 1 (0x02) = apply the remap that follows.
        let p = [
            (cid >> 8) as u8,
            (cid & 0xFF) as u8,
            0x02,
            (to >> 8) as u8,
            (to & 0xFF) as u8,
        ];
        self.request(slot, fi, 0x03, &p).map(|_| ())
    }
}

/// Visible HID++ endpoints, for diagnostics.
pub fn endpoints(api: &HidApi) -> Vec<(String, u16, u16, u16)> {
    api.device_list()
        .filter(|d| d.vendor_id() == LOGITECH_VID && d.usage_page() == HIDPP_USAGE_PAGE)
        .map(|d| {
            (
                d.path().to_string_lossy().to_string(),
                d.product_id(),
                d.usage_page(),
                d.usage(),
            )
        })
        .collect()
}

#[cfg(test)]
mod host_info_tests {
    /// The range check, isolated from the hardware read.
    ///
    /// Same arithmetic `host_info` does after reading the two bytes.
    /// Exists as a test function because the real path needs a receiver
    /// on the desk, and the rule -- "channel within the total the device
    /// declared" -- does not.
    fn coherent(num_hosts: u8, current_zero_based: u8) -> bool {
        let current = current_zero_based + 1;
        !(num_hosts == 0 || num_hosts > 16 || current == 0 || current > num_hosts)
    }

    #[test]
    fn channel_in_range_passes() {
        assert!(coherent(3, 0), "host 1 of 3");
        assert!(coherent(3, 2), "host 3 of 3");
        assert!(coherent(1, 0), "single host");
    }

    #[test]
    fn channel_above_total_is_rejected() {
        // The real case: an MX Keys with three hosts answered channel 5
        // during a reconnection. Returning that as fact made the
        // reconciler decide on a channel that does not exist.
        assert!(!coherent(3, 4), "channel 5 of 3 hosts");
        assert!(!coherent(3, 3), "channel 4 of 3 hosts");
    }

    #[test]
    fn absurd_total_is_rejected() {
        // `num_hosts` also comes from the same corrupted reply: a device
        // declaring 19 hosts read garbage in both bytes.
        assert!(!coherent(0, 0), "zero hosts");
        assert!(!coherent(19, 3), "19 hosts");
    }
}
