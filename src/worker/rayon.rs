use core::convert::TryInto;

use crate::decoder::MAX_COMPONENTS;
use crate::error::Result;
use crate::idct::dequantize_and_idct_block;
use crate::parser::Component;

use std::sync::{Arc, Mutex};

use super::{RowData, Worker};

/// Technically similar to `immediate::ImmediateWorker` but we copy it since we may prefer
/// different style of managing the memory allocation, something that multiple actors can access in
/// parallel.
#[derive(Default)]
struct ImmediateWorker {
    offsets: [usize; MAX_COMPONENTS],
    results: [Vec<u8>; MAX_COMPONENTS],
    components: [Option<Component>; MAX_COMPONENTS],
    quantization_tables: [Option<Arc<[u16; 64]>>; MAX_COMPONENTS],
}

struct ComponentMetadata {
    block_count: usize,
    line_stride: usize,
    dct_scale: usize,
}

pub struct Scoped {
    inner: Mutex<ImmediateWorker>,
}

pub fn with_rayon<T>(f: impl FnOnce(&mut dyn Worker) -> T) -> T {
    rayon::in_place_scope(|_| {
        let inner = ImmediateWorker::default();
        f(&mut Scoped {
            inner: Mutex::new(inner),
        })
    })
}

impl ImmediateWorker {
    pub fn start_immediate(&mut self, data: RowData) {
        let elements = data.component.block_size.width as usize
            * data.component.block_size.height as usize
            * data.component.dct_scale
            * data.component.dct_scale;
        self.offsets[data.index] = 0;
        self.results[data.index].resize(elements, 0u8);
        self.components[data.index] = Some(data.component);
        self.quantization_tables[data.index] = Some(data.quantization_table);
    }

    pub fn get_result_immediate(&mut self, index: usize) -> Vec<u8> {
        core::mem::take(&mut self.results[index])
    }

    pub fn component_metadata(&self, index: usize) -> ComponentMetadata {
        let component = self.components[index].as_ref().unwrap();
        let block_size = component.block_size;
        let block_count = block_size.width as usize * component.vertical_sampling_factor as usize;
        let line_stride = block_size.width as usize * component.dct_scale;
        let dct_scale = component.dct_scale;

        ComponentMetadata {
            block_count,
            line_stride,
            dct_scale,
        }
    }

    pub fn append_row_locked(
        mutex: &Mutex<ImmediateWorker>,
        (index, data): (usize, Vec<i16>),
        result_offset: usize,
    ) {
        // Convert coefficients from a MCU row to samples.
        let quantization_table;
        let block_count;
        let line_stride;
        let block_size;
        let dct_scale;

        {
            let inner = mutex.lock().unwrap();
            quantization_table = inner.quantization_tables[index].as_ref().unwrap().clone();
            block_size = inner.components[index].as_ref().unwrap().block_size;
            let metadata = inner.component_metadata(index);

            block_count = metadata.block_count;
            line_stride = metadata.line_stride;
            dct_scale = metadata.dct_scale;
        }

        assert_eq!(data.len(), block_count * 64);

        let mut output_buffer = [0; 64];
        for i in 0..block_count {
            let x = (i % block_size.width as usize) * dct_scale;
            let y = (i / block_size.width as usize) * dct_scale;

            let coefficients: &[i16; 64] = &data[i * 64..(i + 1) * 64].try_into().unwrap();

            // Write to a temporary intermediate buffer, a 8x8 'image'.
            dequantize_and_idct_block(
                dct_scale,
                coefficients,
                &*quantization_table,
                8,
                &mut output_buffer,
            );

            // Lock the mutex only for this write back, not the main computation.
            // FIXME: we are only copying image data. Can we use some atomic backing buffer and a
            // `Relaxed` write instead?
            let mut write_back = mutex.lock().unwrap();
            let write_back = &mut write_back.results[index][result_offset + y * line_stride + x..];

            let buffered_lines = output_buffer.chunks_mut(8);
            let back_lines = write_back.chunks_mut(line_stride);

            for (buf, back) in buffered_lines.zip(back_lines).take(dct_scale) {
                back[..dct_scale].copy_from_slice(&buf[..dct_scale]);
            }
        }
    }
}

impl super::Worker for Scoped {
    fn start(&mut self, row_data: RowData) -> Result<()> {
        self.inner.get_mut().unwrap().start_immediate(row_data);
        Ok(())
    }

    fn append_row(&mut self, row: (usize, Vec<i16>)) -> Result<()> {
        let (index, data) = row;
        let result_offset;

        {
            let mut inner = self.inner.get_mut().unwrap();
            let metadata = inner.component_metadata(index);

            result_offset = inner.offsets[index];
            inner.offsets[index] += metadata.bytes_used();
        }

        ImmediateWorker::append_row_locked(&self.inner, (index, data), result_offset);
        Ok(())
    }

    fn get_result(&mut self, index: usize) -> Result<Vec<u8>> {
        let result = self.inner.get_mut().unwrap().get_result_immediate(index);
        Ok(result)
    }

    // Magic sauce, these _may_ run in parallel.
    fn append_rows(&mut self, iter: &mut dyn Iterator<Item = (usize, Vec<i16>)>) -> Result<()> {
        rayon::in_place_scope(|scope| {
            let mut inner = self.inner.lock().unwrap();
            // First we schedule everything, making sure their index is right etc.
            for (index, data) in iter {
                let metadata = inner.component_metadata(index);

                let result_offset = inner.offsets[index];
                inner.offsets[index] += metadata.bytes_used();

                let mutex = &self.inner;
                scope.spawn(move |_| {
                    ImmediateWorker::append_row_locked(mutex, (index, data), result_offset)
                });
            }

            // Then the mutex is released, allowing all tasks to run.
        });

        Ok(())
    }
}

impl ComponentMetadata {
    fn bytes_used(&self) -> usize {
        self.block_count * self.dct_scale * self.dct_scale
    }
}
