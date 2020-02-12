use jpeg_decoder::{Decoder, Error};

#[inline(always)]
fn decode(data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut decoder = Decoder::new(data);
    decoder.decode()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        println!("Usage: {} <path-to-crash>", args[0]);
        std::process::exit(1);
    }

    let data = std::fs::read(&args[1]).expect(&format!("Could not open file {}", args[1]));
    match decode(&data) {
        Ok(bytes) => println!("Decoded {} bytes", bytes.len()),
        Err(e) => println!("Decoder returned an error: {:?}\nNote: Not a panic, this is fine.", e),
    };
}
