#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::{Error, Read, Write};
use std::process::{Command, Stdio};

use jpeg_decoder::Decoder;
use image::ImageDecoder;
use mozjpeg::decompress::Decompress;

// Try to check the image, never panic.
fn soft_check(data: &[u8]) -> Result<Vec<u8>, Error> {
    let decompress = Decompress::new_mem(data)?;
    let mut rgb = decompress.rgb()?;
    // Yikes. That method is unsound. But we don't care, we just don't use it with UB.
    let lines = rgb.read_scanlines::<[u8; 3]>()
        .ok_or_else(|| Error::from(std::io::ErrorKind::Other))?;
    let lines = unsafe {
        core::slice::from_raw_parts(
            lines.as_ptr() as *const u8,
            lines.len()*3)
    }.to_owned();
    Ok(lines)
}

fn roughly(data: &[u8], reference: &[u8]) -> bool {
    data.len() == reference.len() && data
        .iter()
        .zip(reference)
        .all(|(&o, &r)| {
            // Not the same criterion as in ref test. For some reason, mozjpeg disagrees with both
            // our output _and_ the output of djpeg/libjpeg-turbo. Let's not question this too
            // much.
            (o as i16 - r as i16).abs() <= 3
        })
}

fuzz_target!(|data: &[u8]| {
    let mut decoder = previous::Decoder::new(data);
    let wrong = decoder.decode().ok();

    // The case should now be fixed.
    let ours = match Decoder::new(data).decode() {
        Err(_) => return,
        Ok(ours) => ours,
    };

    // It should decode correctly.
    let reference = match soft_check(data) {
        Err(_) => return, // Don't crash if it's not a jpeg.
        Ok(reference) => reference,
    };

    let _ = std::fs::write("/tmp/reference", &reference);
    let _ = std::fs::write("/tmp/ours", &ours);

    // It must now pass the reftest
    if !roughly(&ours, &reference) {
        return;
    }

    // The case must have previously failed to decode, or failed reftest
    match wrong {
        Some(data) if roughly(&data, &reference) => return,
        _ => {},
    }

    panic!("Success")
});
