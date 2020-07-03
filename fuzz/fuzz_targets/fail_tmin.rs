#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::{Error, Read, Write};
use std::process::{Command, Stdio};

use jpeg_decoder::Decoder;
use image::ImageDecoder;

// Try to check the image, never panic.
fn soft_check(data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut check = Command::new("djpeg")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let mut stdin = match check.stdin.take() {
            None => return Err(Error::from(std::io::ErrorKind::Other)),
            Some(stdin) => stdin,
        };

        stdin.write_all(data)?;
        stdin.flush()?;
    }

    let output = check.wait_with_output()?;
    if !output.status.success() {
        return Err(Error::from(std::io::ErrorKind::Other));
    }

    let decoder = match image::pnm::PnmDecoder::new(output.stdout.as_slice()) {
        Err(_) => return Err(Error::from(std::io::ErrorKind::Other)),
        Ok(decoder) => decoder,
    };

    let mut image = vec![];
    match decoder.into_reader() {
        Err(_) => return Err(Error::from(std::io::ErrorKind::Other)),
        Ok(mut reader) => match reader.read_to_end(&mut image) {
            Err(_) => return Err(Error::from(std::io::ErrorKind::Other)),
            Ok(_) => {},
        }
    }

    Ok(image)
}

fn roughly(data: &[u8], reference: &[u8]) -> bool {
    data.len() == reference.len() && data
        .iter()
        .zip(reference)
        .all(|(&o, &r)| {
            // Same criterion as in ref test
            (o as i16 - r as i16).abs() <= 2
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
