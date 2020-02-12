use afl::fuzz;

use jpeg_decoder::{Decoder, Error};

#[inline(always)]
fn decode(data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut decoder = Decoder::new(data);
    decoder.decode()
}

fn main() {
    fuzz!(|data: &[u8]| {
        let _ = decode(data);
    });
}
