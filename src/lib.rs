//! This crate contains a JPEG decoder.
//!
//! # Examples
//!
//! ```
//! use jpeg_decoder::Decoder;
//! use std::fs::File;
//! use std::io::BufReader;
//!
//! let file = File::open("tests/reftest/images/extraneous-data.jpg").expect("failed to open file");
//! let mut decoder = Decoder::new(BufReader::new(file));
//! let pixels = decoder.decode().expect("failed to decode image");
//! let metadata = decoder.info().unwrap();
//! ```
//!
//! Get metadata from a file without decoding it:
//!
//! ```
//! use jpeg_decoder::Decoder;
//! use std::fs::File;
//! use std::io::BufReader;
//!
//! let file = File::open("tests/reftest/images/extraneous-data.jpg").expect("failed to open file");
//! let mut decoder = Decoder::new(BufReader::new(file));
//! decoder.read_info().expect("failed to read metadata");
//! let metadata = decoder.info().unwrap();
//! ```

#![deny(missing_docs)]

#[cfg(feature="rayon")]
extern crate rayon;

pub use decoder::{Decoder, ImageInfo, PixelFormat};
pub use error::{Error, UnsupportedFeature};

mod decoder;
mod error;
mod huffman;
mod idct;
mod marker;
mod parser;
mod upsampler;
mod worker;

fn read_u8(mut r: impl std::io::Read) -> std::io::Result<u8> {
    let mut buf = [0; 1];
    r.read_exact(&mut buf).map(|()| buf[0])
}

fn read_be_u16(mut r: impl std::io::Read) -> std::io::Result<u16> {
    let mut buf = [0; 2];
    r.read_exact(&mut buf).map(|()| u16::from_be_bytes(buf))
}
