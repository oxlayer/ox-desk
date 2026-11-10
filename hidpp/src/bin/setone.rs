// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: MIT

//! Moves ONE slot to another channel. Used to test the reconciler: we send
//! only the leader and watch whether the follower goes along.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (Some(slot), Some(host)) = (
        a.first().and_then(|s| s.parse::<u8>().ok()),
        a.get(1).and_then(|s| s.parse::<u8>().ok()),
    ) else {
        eprintln!("usage: setone <slot> <channel 1-based>");
        std::process::exit(2);
    };
    let api = hidapi::HidApi::new().expect("hidapi");
    let rx = hidpp::Receiver::open(&api).expect("receiver");
    match rx.set_host(slot, host) {
        Ok(()) => println!("slot {slot} -> channel {host} sent"),
        Err(e) => { eprintln!("failed: {e}"); std::process::exit(1); }
    }
}
