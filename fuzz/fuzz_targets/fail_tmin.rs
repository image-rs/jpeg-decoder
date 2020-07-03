#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::{Error, Write};
use std::process::{Command, Stdio};

use jpeg_decoder::Decoder;

// Try to check the image, never panic.
fn soft_check(data: &[u8]) -> Result<(), Error> {
    let mut check = Command::new("convert")
        .arg("-verbose")
        .arg("-")
        .arg("/tmp/fail.png")
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

    let mut iter = output.stdout.iter();
    // Should still contain JPEG somewhere.
    loop {
        if iter.as_slice().starts_with(b"JPEG") {
            break;
        }
        match iter.next() {
            None => return Err(Error::from(std::io::ErrorKind::Other)),
            Some(_) => {},
        }
    }

    let mut iter = output.stderr.iter();
    // But should not be marked corrupt.
    loop {
        if iter.as_slice().starts_with(b"Corrupt") {
            return Err(Error::from(std::io::ErrorKind::Other));
        }
        match iter.next() {
            None => break,
            Some(_) => {},
        }
    }

    Ok(())
}

fuzz_target!(|data: &[u8]| {
    match soft_check(data) {
        Ok(_) => {}, // Now crash.
        Err(_) => return, // Don't crash if it's not a jpeg.
    }

    let mut decoder = Decoder::new(data);
    let _ = decoder.decode().expect("");
});
