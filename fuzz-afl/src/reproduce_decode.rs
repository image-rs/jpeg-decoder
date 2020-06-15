use jpeg_decoder::{Decoder, Error};

mod utils;

#[inline(always)]
fn decode(data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut decoder = Decoder::new(data);
    decoder.decode()
}

fn main() {
    let data = utils::read_file_from_args();
    match decode(&data) {
        Ok(bytes) => println!("Decoded {} bytes", bytes.len()),
        Err(e) => println!("Decoder returned an error: {:?}\nNote: Not a panic, this is fine.", e),
    };
}
