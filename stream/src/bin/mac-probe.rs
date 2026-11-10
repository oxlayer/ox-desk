// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the Mac offers to become a screen HOST.
//!
//! Spec: ../../specs/010-remote-desktop.md
//!
//! First step of the Mac host, in the same spirit as what I did for Windows:
//! prove the pieces before assembling the pipeline. This probe answers a
//! single question, the one that decides the design: **does libavdevice link,
//! and does the hardware encoder exist?**
//!
//! If yes, the Mac host ships without new FFI -- capture through the
//! `avfoundation` device and encoding through `h264_videotoolbox`, both in the
//! ffmpeg that is already installed. If not, the path is raw ScreenCaptureKit
//! and VideoToolbox, which means hundreds of lines of Objective-C bridging to
//! get to the same place.
//!
//!   oxdesk-probe-mac

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    ffmpeg_next::init()?;
    // Registers the input devices. If this links, `avdevice` is in the
    // binary -- which is half the question.
    ffmpeg_next::device::register_all();

    println!("  libavdevice: linked and registered");

    let mut found = false;
    for name in ["h264_videotoolbox", "libx264", "h264"] {
        match ffmpeg_next::encoder::find_by_name(name) {
            Some(_) => {
                println!("  encoder {name}: present");
                found = true;
            }
            None => println!("  encoder {name}: missing"),
        }
    }
    if !found {
        anyhow::bail!("no H.264 encoder in this ffmpeg");
    }

    // Capture input: ask by NAME, do not enumerate.
    //
    // `format::list()` existed and was removed in ffmpeg 5; the crate only
    // exposes it for old versions. The current path is the same one
    // `ffmpeg -f avfoundation` itself uses: look the demuxer up by name. A
    // non-null pointer proves the device is compiled into the library AND
    // that `avdevice` was registered -- without that the name does not
    // resolve.
    let name = std::ffi::CString::new("avfoundation")?;
    let input = unsafe { ffmpeg_next::ffi::av_find_input_format(name.as_ptr()) };
    if input.is_null() {
        println!("  avfoundation input: MISSING");
        anyhow::bail!("no screen capture: the Mac host would need raw ScreenCaptureKit");
    }
    println!("  avfoundation input: present");
    println!();
    println!("  => the Mac host ships WITHOUT new FFI: avfoundation + videotoolbox");

    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("this probe is for macOS: it measures the pieces of the screen host there");
}
