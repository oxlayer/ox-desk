//! Raw dump of the battery features, to build the parser on the real
//! reply instead of guessing the layout.
fn main() {
    let api = hidapi::HidApi::new().expect("hidapi");
    let rx = hidpp::Receiver::open(&api).expect("receiver");
    for slot in [1u8, 2] {
        println!("--- slot {slot} ---");
        for (feat, func, name) in [
            (0x1004u16, 0x00u8, "0x1004 getCapabilities"),
            (0x1004, 0x01, "0x1004 getStatus"),
            (0x1000, 0x00, "0x1000 getBatteryLevelStatus"),
            (0x1000, 0x01, "0x1000 getBatteryCapability"),
        ] {
            match rx.raw(slot, feat, func, &[0, 0, 0]) {
                Ok(r) => {
                    let hex: Vec<String> = r.iter().take(8).map(|b| format!("{b:02x}")).collect();
                    let dec: Vec<String> = r.iter().take(8).map(|b| b.to_string()).collect();
                    println!("  {name:32} hex[{}]  dec[{}]", hex.join(" "), dec.join(" "));
                }
                Err(e) => println!("  {name:32} -- {e}"),
            }
        }
    }
}
