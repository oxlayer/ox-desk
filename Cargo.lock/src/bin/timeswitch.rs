//! Cronometra exatamente o que o usuario sente: ping de posse + escrita
//! do ChangeHost, sem verificacao e sem API.
use std::time::Instant;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (Some(slot), Some(host)) = (
        a.first().and_then(|s| s.parse::<u8>().ok()),
        a.get(1).and_then(|s| s.parse::<u8>().ok()),
    ) else { eprintln!("uso: timeswitch <slot> <canal>"); std::process::exit(2) };

    let api = hidapi::HidApi::new().expect("hidapi");
    let t_open = Instant::now();
    let rx = hidpp::Receiver::open(&api).expect("receptor");
    let ms_open = t_open.elapsed().as_secs_f64() * 1000.0;

    let t_ping = Instant::now();
    let presente = rx.ping(slot).is_ok();
    let ms_ping = t_ping.elapsed().as_secs_f64() * 1000.0;
    if !presente { eprintln!("slot {slot} nao responde"); std::process::exit(1) }

    // Primeira chamada paga o feature_index; a segunda usa o memo.
    let t_fi = Instant::now();
    let _ = rx.feature_index(slot, hidpp::FEATURE_CHANGE_HOST);
    let ms_fi = t_fi.elapsed().as_secs_f64() * 1000.0;

    let t_set = Instant::now();
    let r = rx.set_host_nowait(slot, host);
    let ms_set = t_set.elapsed().as_secs_f64() * 1000.0;

    println!("abrir receptor : {ms_open:7.2} ms");
    println!("ping de posse  : {ms_ping:7.2} ms");
    println!("feature index  : {ms_fi:7.2} ms  (memoizado depois da 1a vez)");
    println!("escrita ChangeHost: {ms_set:6.2} ms");
    println!("--------------------------------");
    println!("TOTAL sentido  : {:7.2} ms   {r:?}", ms_ping + ms_fi + ms_set);
}
