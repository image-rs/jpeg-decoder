extern crate docopt;
extern crate jpeg_decoder as jpeg;
extern crate png;

use docopt::Docopt;
use png::HasParameters;
use std::env;
use std::fs::File;
use std::io::BufReader;
use std::process;

const USAGE: &'static str = "
Usage: decode <input> [--output=<file>]
       decode -h | --help

Options:
    -h --help                   Show this screen.
    -o <file>, --output=<file>  Output PNG file.
";

fn main() {
    let args = &Docopt::new(USAGE)
        .and_then(|d| d.argv(env::args()).parse())
        .unwrap_or_else(|e| e.exit());
    let input = args.get_str("<input>");
    let output = args.get_str("-o");
    let file = match File::open(input) {
        Ok(file) => file,
        Err(error) => {
            println!("The specified input could not be opened: {}", error);
            process::exit(1);
        },
    };
    let mut decoder = jpeg::Decoder::new(BufReader::new(file));
    let mut data = match decoder.decode() {
        Ok(data) => data,
        Err(error) => {
            println!("The image could not be decoded: {}", error);
            println!("If other software can decode this image successfully then it's likely that this is a bug.");
            process::exit(1);
        }
    };

    if !output.is_empty() {
        let output_file = File::create(output).unwrap();
        let info = decoder.info().unwrap();
        let mut encoder = png::Encoder::new(output_file, info.width as u32, info.height as u32);
        encoder.set(png::BitDepth::Eight);

        match info.pixel_format {
            jpeg::PixelFormat::L8     => encoder.set(png::ColorType::Grayscale),
            jpeg::PixelFormat::RGB24  => encoder.set(png::ColorType::RGB),
            jpeg::PixelFormat::CMYK32 => {
                data = cmyk_to_rgb(&mut data);
                encoder.set(png::ColorType::RGB)
            },
        };

        encoder.write_header().expect("writing png header failed").write_image_data(&data).expect("png encoding failed");
    }
}

fn cmyk_to_rgb(input: &[u8]) -> Vec<u8> {
    let size = input.len() - input.len() / 4;
    let mut output = Vec::with_capacity(size);

    for pixel in input.chunks(4) {
        let c = pixel[0] as f32 / 255.0;
        let m = pixel[1] as f32 / 255.0;
        let y = pixel[2] as f32 / 255.0;
        let k = pixel[3] as f32 / 255.0;

        // CMYK -> CMY
        let c = c * (1.0 - k) + k;
        let m = m * (1.0 - k) + k;
        let y = y * (1.0 - k) + k;

        // CMY -> RGB
        let r = (1.0 - c) * 255.0;
        let g = (1.0 - m) * 255.0;
        let b = (1.0 - y) * 255.0;

        output.push(r as u8);
        output.push(g as u8);
        output.push(b as u8);
    }

    output
}
