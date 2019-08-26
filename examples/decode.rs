extern crate jpeg_decoder as jpeg;
extern crate png;

use png::HasParameters;
use std::env;
use std::fs::File;
use std::io::BufReader;
use std::process;

fn usage() -> ! {
    eprint!("usage: decode image.jpg image.png");
    process::exit(1)
}

fn main() {
    let mut args = env::args().skip(1);
    let input_path = args.next().unwrap_or_else(|| usage());
    let output_path = args.next().unwrap_or_else(|| usage());

    let input_file = File::open(input_path).expect("The specified input file could not be opened");
    let mut decoder = jpeg::Decoder::new(BufReader::new(input_file));
    let mut data = decoder.decode().expect("Decoding failed. If other software can successfully decode the specified JPEG image, then it's likely that there is a bug in jpeg-decoder");
    let info = decoder.info().unwrap();

    let output_file = File::create(output_path).unwrap();
    let mut encoder = png::Encoder::new(output_file, u32::from(info.width), u32::from(info.height));
    encoder.set(png::BitDepth::Eight);

    match info.pixel_format {
        jpeg::PixelFormat::L8     => encoder.set(png::ColorType::Grayscale),
        jpeg::PixelFormat::RGB24  => encoder.set(png::ColorType::RGB),
        jpeg::PixelFormat::CMYK32 => {
            data = cmyk_to_rgb(&data);
            encoder.set(png::ColorType::RGB)
        },
    };

    encoder.write_header()
           .expect("writing png header failed")
           .write_image_data(&data)
           .expect("png encoding failed");
}

#[allow(clippy::many_single_char_names)]
fn cmyk_to_rgb(input: &[u8]) -> Vec<u8> {
    let size = input.len() - input.len() / 4;
    let mut output = Vec::with_capacity(size);

    for pixel in input.chunks(4) {
        let c = f32::from(pixel[0]) / 255.0;
        let m = f32::from(pixel[1]) / 255.0;
        let y = f32::from(pixel[2]) / 255.0;
        let k = f32::from(pixel[3]) / 255.0;

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
