extern crate jpeg_decoder as jpeg;
extern crate walkdir;

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

mod common;

#[test]
fn crashtest() {
    let files = common::test_files(&Path::new("tests").join("crashtest"));

    for path in &files {
        let file = File::open(path).unwrap();
        let mut decoder = jpeg::Decoder::new(BufReader::new(file));
        let _ = decoder.decode();
    }
}
