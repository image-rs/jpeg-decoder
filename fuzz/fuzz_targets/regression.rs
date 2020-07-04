#![no_main]
use libfuzzer_sys::fuzz_target;

use jpeg_decoder;
use previous;

fuzz_target!(|data: &[u8]| {
    // The case should now be fixed.
    match jpeg_decoder::Decoder::new(data).decode() {
        Err(_) => return,
        Ok(_) => {},
    }

    // And error/fail on previous.
    match previous::Decoder::new(data).decode() {
        Ok(_) => return,
        Err(_) => panic!(),
    }
});
