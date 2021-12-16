#![allow(unsafe_code)]

mod ssse3;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use std::is_x86_feature_detected;

/// Arch-specific implementation of YCbCr conversion. Returns the number of pixels that were
/// converted.
pub fn color_convert_line_ycbcr(y: &[u8], cb: &[u8], cr: &[u8], output: &mut [u8]) -> usize {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[allow(unsafe_code)]
    {
        if is_x86_feature_detected!("ssse3") {
            unsafe {
                return ssse3::color_convert_line_ycbcr(y, cb, cr, output);
            }
        }
    }
    return 0;
}

/// Arch-specific implementation of 8x8 IDCT.
pub fn dequantize_and_idct_block_8x8(
    coefficients: &[i16],
    quantization_table: &[u16; 64],
    output_linestride: usize,
    output: &mut [u8],
) {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[allow(unsafe_code)]
    {
        if is_x86_feature_detected!("ssse3") {
            unsafe {
                ssse3::dequantize_and_idct_block_8x8(
                    coefficients,
                    quantization_table,
                    output_linestride,
                    output,
                );
                return;
            }
        }
    }
    unreachable!("No arch-specific IDCT available");
}

/// Returns true if an arch-specific IDCT is avaliable, false otherwise.
pub fn has_arch_specific_idct() -> bool {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[allow(unsafe_code)]
    {
        if is_x86_feature_detected!("ssse3") {
            return true;
        }
    }
    return false;
}
