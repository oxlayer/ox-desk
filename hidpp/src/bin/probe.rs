//! Diagnostic of the crate against the real hardware.
use hidapi::HidApi;
use std::time::Instant;

fn main() {
    let api = HidApi::new().expect("hidapi");
    println!("HID++ endpoints:");
    for (path, pid, up, u) in hidpp::endpoints(&api) {
        println!("  {path:16} pid={pid:04x} usage_page={up:#06x} usage={u:#04x}");
    }
    let r = match hidpp::Receiver::open(&api) {
        Ok(r) => r,
        Err(e) => { eprintln!("open receiver: {e}"); std::process::exit(1); }
    };
    println!("\nreceiver: {} (pid {:04x})", r.path, r.product_id);

    let t0 = Instant::now();
    let devs = r.devices(6, true, true);
    let total = t0.elapsed().as_secs_f64() * 1000.0;

    for d in &devs {
        if d.online {
            let h = d.host.map(|h| format!("channel {}/{}", h.current, h.num_hosts))
                          .unwrap_or_else(|| "channel ?".into());
            let bat = d.battery.as_ref().map(|b| {
                let p = b.percent.map(|v| format!("{v}%")).unwrap_or_default();
                // `status` is what the DEVICE declares, not our inference
                // -- the MX Keys says "descarregando" (discharging) on the
                // cable with the charge rising. That is why it is labeled.
                let st = if b.charging {
                    "charging".to_string()
                } else {
                    b.status.map(|s| format!("says: {s}")).unwrap_or_default()
                };
                let nx = b.next_level.map(|v| format!(" next {v}%")).unwrap_or_default();
                format!("battery {p} {st}{nx} ({})", b.source)
            }).unwrap_or_else(|| "battery ?".into());
            println!("  slot {}: ONLINE  {:?}  {h}  ping {:?}ms  {bat}",
                     d.slot, d.name.as_deref().unwrap_or("?"), d.reply_ms);
        }
    }
    let n = devs.iter().filter(|d| d.online).count();
    println!("\n{n} online; full scan of 6 slots with names: {total:.0} ms");
    println!("(solaar show, for comparison: 5900 to 15300 ms)");
}
