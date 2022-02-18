#[test]
pub fn test_rgb() {
    let input = include_bytes!("../reftest/images/mozilla/jpg-size-8x8.jpg");
    let reference = include_bytes!("jpg-size-8x8.raw");

    let mut decoder = jpeg::Decoder::new(input.as_ref());
    let result = decoder.decode().unwrap();

    let err = result.iter()
        .zip(reference)
        .find(|(&v1, &v2)| (v1 as i16 - v2 as i16).abs() > 1)
        .is_some();

    assert!(!err);
}
