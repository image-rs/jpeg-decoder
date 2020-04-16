# jpeg-decoder

[![Rust CI](https://github.com/image-rs/jpeg-decoder/workflows/Rust%20CI/badge.svg)](https://github.com/image-rs/jpeg-decoder/actions)
[![AppVeyor Build Status](https://ci.appveyor.com/api/projects/status/k65rrkd0f8yb4o9w/branch/master?svg=true)](https://ci.appveyor.com/project/kaksmet/jpeg-decoder/branch/master)
[![Crates.io](https://img.shields.io/crates/v/jpeg-decoder.svg)](https://crates.io/crates/jpeg-decoder)

A Rust library for decoding JPEGs.

[Documentation](https://docs.rs/jpeg-decoder)

## Example

Cargo.toml:
```toml
[dependencies]
jpeg-decoder = "0.1"
```

main.rs:
```rust
extern crate jpeg_decoder as jpeg;

use std::fs::File;
use std::io::BufReader;

fn main() {
    let file = File::open("hello_world.jpg").expect("failed to open file");
    let mut decoder = jpeg::Decoder::new(BufReader::new(file));
    let pixels = decoder.decode().expect("failed to decode image");
    let metadata = decoder.info().unwrap();
}
```

## Performance
 This crate uses [rayon](https://github.com/rayon-rs/rayon) to decode images on all available cores by default.
 This can be disabled by requiring the dependency with `default-features = false`.
 
 This crate can optionally use [SIMD](https://en.wikipedia.org/wiki/SIMD) instructions
 to decode images even faster.
 This is not enabled by default because it requires a nightly compiler,
 but can be activated with the `packed_simd` feature.
 
## Requirements
This crate compiles only with rust >= 1.34.
