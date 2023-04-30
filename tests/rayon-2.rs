//! Must be a separate test because it modifies the _global_ rayon pool.
use jpeg_decoder::Decoder;
use std::{fs::File, path::Path};

#[test]
fn decoding_in_global_pool() {
    let path = Path::new("tests/reftest/images/progressive3.jpg");

    rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build_global()
        .unwrap();

    let _: Vec<_> = (0..1024)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut decoder = Decoder::new(File::open(&path).unwrap());
                let _ = decoder.decode().unwrap();
            });
        })
        .collect();
}
