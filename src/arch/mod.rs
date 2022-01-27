#![allow(unsafe_code)]

mod ssse3;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use std::is_x86_feature_detected;

/// Arch-specific implementation of YCbCr conversion. Returns the number of pixels that were
/// converted.
pub fn get_color_convert_line_ycbcr() -> Option<unsafe fn(&[u8], &[u8], &[u8], &mut [u8]) -> usize>
{
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[allow(unsafe_code)]
    {
        if is_x86_feature_detected!("ssse3") {
            return Some(ssse3::color_convert_line_ycbcr);
        }
    }
    None
}

/// Arch-specific implementation of 8x8 IDCT.
pub fn get_dequantize_and_idct_block_8x8() -> Option<unsafe fn(&[i16], &[u16; 64], usize, &mut [u8])>
{
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[allow(unsafe_code)]
    {
        if is_x86_feature_detected!("ssse3") {
            return Some(ssse3::dequantize_and_idct_block_8x8);
        }
    }
    None
}
