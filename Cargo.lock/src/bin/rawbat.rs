//! Dump cru dos features de bateria, para montar o parser sobre a
//! resposta real em vez de supor o layout.
fn main() {
    let api = hidapi::HidApi::new().expect("hidapi");
    let rx = hidpp::Receiver::open(&api).expect("receptor");
    for slot in [1u8, 2] {
        println!("--- slot {slot} ---");
        for (feat, func, nome) in [
            (0x1004u16, 0x00u8, "0x1004 getCapabilities"),
            (0x1004, 0x01, "0x1004 getStatus"),
            (0x1000, 0x00, "0x1000 getBatteryLevelStatus"),
            (0x1000, 0x01, "0x1000 getBatteryCapability"),
        ] {
            match rx.raw(slot, feat, func, &[0, 0, 0]) {
                Ok(r) => {
                    let hex: Vec<String> = r.iter().take(8).map(|b| format!("{b:02x}")).collect();
                    let dec: Vec<String> = r.iter().take(8).map(|b| b.to_string()).collect();
                    println!("  {nome:32} hex[{}]  dec[{}]", hex.join(" "), dec.join(" "));
                }
                Err(e) => println!("  {nome:32} -- {e}"),
            }
        }
    }
}
