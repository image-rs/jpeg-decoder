extern crate byteorder;
extern crate euclid;
extern crate num;
extern crate rayon;

pub use color::ColorSpace;
pub use decoder::{Decoder, Metadata};
pub use error::{Error, UnsupportedFeature};

mod color;
mod decoder;
mod error;
mod huffman;
mod idct;
mod marker;
mod parser;
mod resampler;
mod worker_thread;
