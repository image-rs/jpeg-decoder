use afl::fuzz;

use jpeg_decoder::{Decoder, ImageInfo};

#[inline(always)]
fn get_info(data: &[u8]) -> Option<ImageInfo> {
    let mut decoder = Decoder::new(data);
    decoder.read_info().ok().and_then(|_| decoder.info())
}

fn main() {
    fuzz!(|data: &[u8]| {
        let _ = get_info(data);
    });
}
