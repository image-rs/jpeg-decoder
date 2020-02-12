use jpeg_decoder::{Decoder, ImageInfo};

mod utils;

#[inline(always)]
fn get_info(data: &[u8]) -> Option<ImageInfo> {
    let mut decoder = Decoder::new(data);
    decoder.read_info().ok().and_then(|_| decoder.info())
}

fn main() {
    let data = utils::read_file_from_args();
    match get_info(&data) {
        Some(info) => println!("Info: {:?}", info),
        None => println!("Found no info in file"),
    };
}
