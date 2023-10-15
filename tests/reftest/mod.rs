use jpeg;
use png;
use std::cmp;
use std::fs::File;
use std::path::Path;

use super::common;

#[test]
fn reftest() {
    let files = common::test_files(&Path::new("tests").join("reftest").join("images"));

    for path in &files {
        reftest_file(path);
    }
}

#[test]
fn reftest_scaled() {
    let base = &Path::new("tests").join("reftest").join("images");
    reftest_scaled_file(&base.join("rgb.jpg"), 500, 333, &base.join("rgb.png"));
    reftest_scaled_file(&base.join("rgb.jpg"), 250, 167, &base.join("rgb_250x167.png"));
    reftest_scaled_file(&base.join("rgb.jpg"), 125, 84,  &base.join("rgb_125x84.png"));
    reftest_scaled_file(&base.join("rgb.jpg"), 63,  42,  &base.join("rgb_63x42.png"));
}

fn reftest_file(path: &Path) {
    let file = File::open(path).unwrap();
    let decoder = jpeg::Decoder::new(file);
    reftest_decoder(decoder, path, &path.with_extension("png"));
}

fn reftest_scaled_file(path: &Path, width: u16, height: u16, ref_path: &Path) {
    let file = File::open(path).unwrap();
    let mut decoder = jpeg::Decoder::new(file);
    decoder.read_info().unwrap();
    decoder.scale(width, height).unwrap();
    reftest_decoder(decoder, path, &ref_path);
}

fn reftest_decoder<T: std::io::Read>(mut decoder: jpeg::Decoder<T>, path: &Path, ref_path: &Path) {
    let mut data = decoder.decode().expect(&format!("failed to decode file: {:?}", path));
    let info = decoder.info().unwrap();
    let mut pixel_format = info.pixel_format;

    if pixel_format == jpeg::PixelFormat::CMYK32 {
        data = cmyk_to_rgb(&data);
        pixel_format = jpeg::PixelFormat::RGB24;
    }

    let ref_file = File::open(ref_path).unwrap();
    let mut decoder = png::Decoder::new(ref_file);

    if pixel_format == jpeg::PixelFormat::L16 {
        // disable the default 8bit output of png v0.16.8 (fixed in master branch of png)
        decoder.set_transformations(png::Transformations::EXPAND);
    }
    
    let (ref_info, mut ref_reader) = decoder.read_info().expect("png failed to read info");

    assert_eq!(ref_info.width, info.width as u32);
    assert_eq!(ref_info.height, info.height as u32);

    let mut ref_data = vec![0; ref_info.buffer_size()];
    ref_reader.next_frame(&mut ref_data).expect("png decode failed");
    let mut ref_pixel_format = ref_info.color_type;

    if ref_pixel_format == png::ColorType::RGBA { 
        ref_data = rgba_to_rgb(&ref_data);
        ref_pixel_format = png::ColorType::RGB;
    }

    let (refdata_16, data_u16) : (Vec<u16>, Vec<u16>) = match pixel_format {
        jpeg::PixelFormat::L8 => {
            assert_eq!(ref_pixel_format, png::ColorType::Grayscale);
            assert_eq!(ref_info.bit_depth, png::BitDepth::Eight);
            (ref_data.iter().map(|x| *x as u16).collect(), data.iter().map(|x| *x as u16).collect())
        },
        jpeg::PixelFormat::L16 => {
            assert_eq!(ref_pixel_format, png::ColorType::Grayscale);
            assert_eq!(ref_info.bit_depth, png::BitDepth::Sixteen);
            (ref_data.chunks_exact(2).map(|a| u16::from_be_bytes([a[0],a[1]])).collect(),
            data.chunks_exact(2).map(|a| u16::from_ne_bytes([a[0],a[1]])).collect())
        },
        jpeg::PixelFormat::RGB24 => {
            assert_eq!(ref_pixel_format, png::ColorType::RGB);
            assert_eq!(ref_info.bit_depth, png::BitDepth::Eight);
            (ref_data.iter().map(|x| *x as u16).collect(), data.iter().map(|x| *x as u16).collect())
        },
        _ => panic!(),
    };

    assert_eq!(data_u16.len(), refdata_16.len());
    let mut max_diff = 0;
    let pixels: Vec<u8> = data_u16.iter().zip(refdata_16.iter()).map(|(&a, &b)| {
        let diff = (a as isize - b as isize).abs();
        max_diff = cmp::max(diff, max_diff);

        if (info.coding_process != jpeg::CodingProcess::Lossless && diff <= 3) || diff == 0  {
            // White for correct
            0xFF
        } else {
            // "1100" in the RGBA channel with an error for an incorrect value
            // This results in some number of C0 and FFs, which is much more
            // readable (and distinguishable) than the previous difference-wise
            // scaling but does not require reconstructing the actual RGBA pixel.
            0xC0
        }
    }).collect();

    if pixels.iter().any(|&a| a < 255) {
        let output_path = path.with_file_name(format!("{}-diff.png", path.file_stem().unwrap().to_str().unwrap()));
        let output = File::create(&output_path).unwrap();
        let mut encoder = png::Encoder::new(output, info.width as u32, info.height as u32);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_color(ref_pixel_format);
        encoder.write_header().expect("png failed to write header").write_image_data(&pixels).expect("png failed to write data");

        panic!("decoding difference: {output_path:?}, maximum difference was {max_diff}");
    }
}

fn rgba_to_rgb(input: &[u8]) -> Vec<u8> {
    let size = input.len() - input.len() / 4;
    let mut output = Vec::with_capacity(size);

    for pixel in input.chunks(4) {
        assert_eq!(pixel[3], 255);

        output.push(pixel[0]);
        output.push(pixel[1]);
        output.push(pixel[2]);
    }

    output
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
