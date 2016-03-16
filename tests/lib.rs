extern crate jpeg_decoder as jpeg;
extern crate png;
extern crate walkdir;

use std::path::Path;
use std::fs::File;

mod common;
mod crashtest;
mod reftest;

#[test]
fn read_metadata() {
    let path = Path::new("tests").join("reftest").join("images").join("mozilla").join("jpg-progressive.jpg");

    let mut decoder = jpeg::Decoder::new(File::open(&path).unwrap());
    let ref_data = decoder.decode_pixels().unwrap();
    let ref_metadata = decoder.metadata().unwrap();

    decoder = jpeg::Decoder::new(File::open(&path).unwrap());
    decoder.read_metadata().unwrap();
    let metadata = decoder.metadata().unwrap();
    let data = decoder.decode_pixels().unwrap();

    assert_eq!(metadata, decoder.metadata().unwrap());
    assert_eq!(metadata, ref_metadata);
    assert_eq!(data, ref_data);
}
