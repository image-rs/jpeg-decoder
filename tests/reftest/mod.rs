use jpeg;
use png::{self, HasParameters};
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

fn reftest_file(path: &Path) {
    let file = File::open(path).unwrap();
    let mut decoder = jpeg::Decoder::new(file);
    let mut data = decoder.decode_pixels().expect(&format!("failed to decode file: {:?}", path));
    let metadata = decoder.metadata().unwrap();
    let mut color_space = metadata.dst_color_space;

    if color_space == jpeg::ColorSpace::CMYK {
        data = cmyk_to_rgb(&data);
        color_space = jpeg::ColorSpace::RGB;
    }

    let ref_file = File::open(path.with_extension("png")).unwrap();
    let (ref_metadata, mut ref_reader) = png::Decoder::new(ref_file).read_info().expect("png failed to read info");

    assert_eq!(ref_metadata.width, metadata.width as u32);
    assert_eq!(ref_metadata.height, metadata.height as u32);
    assert_eq!(ref_metadata.bit_depth, png::BitDepth::Eight);

    let mut ref_data = vec![0; ref_metadata.buffer_size()];
    ref_reader.next_frame(&mut ref_data).expect("png decode failed");
    let mut ref_color_type = ref_metadata.color_type;

    if ref_color_type == png::ColorType::RGBA {
        ref_data = rgba_to_rgb(&ref_data);
        ref_color_type = png::ColorType::RGB;
    }

    match color_space {
        jpeg::ColorSpace::Grayscale => assert_eq!(ref_color_type, png::ColorType::Grayscale),
        jpeg::ColorSpace::RGB       => assert_eq!(ref_color_type, png::ColorType::RGB),
        _ => panic!(),
    }

    assert_eq!(data.len(), ref_data.len());

    let mut max_diff = 0;
    let pixels: Vec<u8> = data.iter().zip(ref_data.iter()).map(|(&a, &b)| {
        let diff = (a as i16 - b as i16).abs();
        max_diff = cmp::max(diff, max_diff);

        // FIXME: Only a diff of 1 should be allowed?
        if diff <= 2 {
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
        let mut encoder = png::Encoder::new(output, metadata.width as u32, metadata.height as u32);
        encoder.set(png::BitDepth::Eight);
        encoder.set(ref_color_type);
        encoder.write_header().expect("png failed to write header").write_image_data(&pixels).expect("png failed to write data");

        panic!("decoding difference: {:?}, maximum difference was {}", output_path, max_diff);
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
