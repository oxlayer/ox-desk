//! Diagnostico do crate contra o hardware real.
use hidapi::HidApi;
use std::time::Instant;

fn main() {
    let api = HidApi::new().expect("hidapi");
    println!("endpoints HID++:");
    for (path, pid, up, u) in hidpp::endpoints(&api) {
        println!("  {path:16} pid={pid:04x} usage_page={up:#06x} usage={u:#04x}");
    }
    let r = match hidpp::Receiver::open(&api) {
        Ok(r) => r,
        Err(e) => { eprintln!("abrir receptor: {e}"); std::process::exit(1); }
    };
    println!("\nreceptor: {} (pid {:04x})", r.path, r.product_id);

    let t0 = Instant::now();
    let devs = r.devices(6, true);
    let total = t0.elapsed().as_secs_f64() * 1000.0;

    for d in &devs {
        if d.online {
            let h = d.host.map(|h| format!("canal {}/{}", h.current, h.num_hosts))
                          .unwrap_or_else(|| "canal ?".into());
            println!("  slot {}: ONLINE  {:?}  {h}  ping {:?}ms",
                     d.slot, d.name.as_deref().unwrap_or("?"), d.reply_ms);
        }
    }
    let n = devs.iter().filter(|d| d.online).count();
    println!("\n{n} online; varredura completa de 6 slots com nomes: {total:.0} ms");
    println!("(solaar show, para comparar: 5900 a 15300 ms)");
}
