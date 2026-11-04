//! Move UM slot de canal. Usado para testar o reconciliador: mandamos so
//! o lider e vemos se o seguidor vai atras.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (Some(slot), Some(host)) = (
        a.first().and_then(|s| s.parse::<u8>().ok()),
        a.get(1).and_then(|s| s.parse::<u8>().ok()),
    ) else {
        eprintln!("uso: setone <slot> <canal 1-based>");
        std::process::exit(2);
    };
    let api = hidapi::HidApi::new().expect("hidapi");
    let rx = hidpp::Receiver::open(&api).expect("receptor");
    match rx.set_host(slot, host) {
        Ok(()) => println!("slot {slot} -> canal {host} enviado"),
        Err(e) => { eprintln!("falhou: {e}"); std::process::exit(1); }
    }
}
