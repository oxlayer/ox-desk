//! Dump cru do 0x1B04 REPROG CONTROLS V4, para montar o parser sobre a
//! resposta real. Mesmo metodo que decifrou 0x1815 e a bateria.
fn main() {
    let api = hidapi::HidApi::new().expect("hidapi");
    let rx = hidpp::Receiver::open(&api).expect("receptor");
    for slot in [1u8, 2] {
        println!("--- slot {slot} ---");
        // func 0 = getCount
        match rx.raw(slot, 0x1B04, 0x00, &[0, 0, 0]) {
            Ok(r) => {
                let n = r.first().copied().unwrap_or(0);
                println!("  getCount -> {n} controles  (hex {:02x?})", &r[..4.min(r.len())]);
                // func 1 = getControlIdInfo(index)
                for i in 0..n.min(10) {
                    match rx.raw(slot, 0x1B04, 0x01, &[i, 0, 0]) {
                        Ok(c) if c.len() >= 8 => {
                            let cid = u16::from_be_bytes([c[0], c[1]]);
                            let tid = u16::from_be_bytes([c[2], c[3]]);
                            println!(
                                "   [{i}] controlID=0x{cid:04x} taskID=0x{tid:04x} \
                                 flags=0x{:02x} pos={} group=0x{:02x} gmask=0x{:02x} \
                                 rawflags=0x{:02x}",
                                c[4], c[5], c[6], c[7],
                                c.get(8).copied().unwrap_or(0)
                            );
                        }
                        Ok(c) => println!("   [{i}] resposta curta: {:02x?}", c),
                        Err(e) => println!("   [{i}] {e}"),
                    }
                }
            }
            Err(e) => println!("  getCount -- {e}"),
        }
        // func 2 = getControlReporting(cid): o mapeamento ATUAL
        println!("  mapeamento atual:");
        for cid in [0x0052u16, 0x0053, 0x0056, 0x00c4, 0x00d7] {
            match rx.raw(slot, 0x1B04, 0x02, &[(cid >> 8) as u8, (cid & 0xff) as u8, 0]) {
                Ok(r) if r.len() >= 5 => println!(
                    "   cid=0x{cid:04x} -> remap=0x{:04x} flags=0x{:02x}  (hex {:02x?})",
                    u16::from_be_bytes([r[3], r[4]]), r[2], &r[..5]
                ),
                Ok(r) => println!("   cid=0x{cid:04x} curto: {:02x?}", r),
                Err(e) => println!("   cid=0x{cid:04x} {e}"),
            }
        }
    }
}
