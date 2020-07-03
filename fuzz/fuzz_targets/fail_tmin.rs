#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::{Error, Write};
use std::process::{Command, Stdio};

use jpeg_decoder::Decoder;

// Try to check the image, never panic.
fn soft_check(data: &[u8]) -> Result<(), Error> {
    let mut check = Command::new("djpeg")
        .arg("-fast")
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
