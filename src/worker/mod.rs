mod immediate;
mod multithreaded;
#[cfg(all(
    not(any(target_arch = "asmjs", target_arch = "wasm32")),
    feature = "rayon"
))]
mod rayon;

use crate::decoder::choose_color_convert_func;
use crate::error::Result;
use crate::parser::{AdobeColorTransform, Component, Dimensions};
use crate::upsampler::Upsampler;
use alloc::sync::Arc;
use alloc::vec::Vec;

pub struct RowData {
    pub index: usize,
    pub component: Component,
    pub quantization_table: Arc<[u16; 64]>,
}

pub trait Worker {
    fn start(&mut self, row_data: RowData) -> Result<()>;
    fn append_row(&mut self, row: (usize, Vec<i16>)) -> Result<()>;
    fn get_result(&mut self, index: usize) -> Result<Vec<u8>>;
    /// Default implementation for spawning multiple tasks.
    fn append_rows(&mut self, row: &mut dyn Iterator<Item = (usize, Vec<i16>)>) -> Result<()> {
        for item in row {
            self.append_row(item)?;
        }
        Ok(())
    }
}

#[allow(dead_code)]
pub enum PreferWorkerKind {
    Immediate,
    Multithreaded,
}

/// Execute something with a worker system.
pub fn with_worker<T>(prefer: PreferWorkerKind, f: impl FnOnce(&mut dyn Worker) -> T) -> T {
    match prefer {
        #[cfg(all(
            not(any(target_arch = "asmjs", target_arch = "wasm32")),
            feature = "rayon"
        ))]
        PreferWorkerKind::Multithreaded => self::rayon::with_rayon(f),
        #[allow(unreachable_patterns)]
        #[cfg(not(any(target_arch = "asmjs", target_arch = "wasm32")))]
        PreferWorkerKind::Multithreaded => self::multithreaded::with_multithreading(f),
        _ => self::immediate::with_immediate(f),
    }
}

pub fn compute_image_parallel(
    components: &[Component],
    data: Vec<Vec<u8>>,
    output_size: Dimensions,
    is_jfif: bool,
    color_transform: Option<AdobeColorTransform>,
) -> Result<Vec<u8>> {
    #[cfg(all(
        not(any(target_arch = "asmjs", target_arch = "wasm32")),
        feature = "rayon"
    ))]
    return rayon::compute_image_parallel(components, data, output_size, is_jfif, color_transform);

    #[allow(unreachable_code)]
    {
        let color_convert_func =
            choose_color_convert_func(components.len(), is_jfif, color_transform)?;
        let upsampler = Upsampler::new(components, output_size.width, output_size.height)?;
        let line_size = output_size.width as usize * components.len();
        let mut image = vec![0u8; line_size * output_size.height as usize];

        for (row, line) in image.chunks_mut(line_size).enumerate() {
            upsampler.upsample_and_interleave_row(
                &data,
                row,
                output_size.width as usize,
                line,
                color_convert_func,
            );
        }

        Ok(image)
    }
}
