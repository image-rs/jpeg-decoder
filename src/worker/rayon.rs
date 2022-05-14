use core::convert::TryInto;

use rayon::iter::{IndexedParallelIterator, ParallelDrainRange, ParallelIterator};
use rayon::slice::ParallelSliceMut;

use crate::decoder::choose_color_convert_func;
use crate::error::Result;
use crate::idct::dequantize_and_idct_block;
use crate::parser::{AdobeColorTransform, Component};
use crate::upsampler::Upsampler;
use crate::{decoder::MAX_COMPONENTS, parser::Dimensions};

use std::sync::Arc;

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

#[derive(Clone, Copy)]
struct ComponentMetadata {
    block_width: usize,
    block_count: usize,
    line_stride: usize,
    dct_scale: usize,
}

struct WorkItem {
    quantization_table: Arc<[u16; 64]>,
    metadata: ComponentMetadata,
    data: Vec<i16>,
}

#[derive(Default)]
pub struct Scoped {
    inner: ImmediateWorker,
    // Unfinished, synchronous work for each component. Must simulate finishing the synchronously
    // as they write to `results` which is tracked externally.
    task_queues: [Vec<WorkItem>; MAX_COMPONENTS],
    sync: usize,
    parallel: usize,
}

pub fn with_rayon<T>(f: impl FnOnce(&mut dyn Worker) -> T) -> T {
    let mut scope = Scoped::default();
    let result = f(&mut scope);
    // dbg!(scope.sync, scope.parallel);
    result
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

    pub fn component_metadata(&self, index: usize) -> Option<ComponentMetadata> {
        let component = self.components[index].as_ref()?;
        let block_size = component.block_size;
        let block_width = block_size.width as usize;
        let block_count = block_size.width as usize * component.vertical_sampling_factor as usize;
        let line_stride = block_size.width as usize * component.dct_scale;
        let dct_scale = component.dct_scale;

        Some(ComponentMetadata {
            block_width,
            block_count,
            line_stride,
            dct_scale,
        })
    }

    pub fn append_row_locked(
        quantization_table: Arc<[u16; 64]>,
        metadata: ComponentMetadata,
        data: Vec<i16>,
        result_block: &mut [u8],
    ) {
        // Convert coefficients from a MCU row to samples.
        let ComponentMetadata {
            block_count,
            line_stride,
            block_width,
            dct_scale,
        } = metadata;

        assert_eq!(data.len(), block_count * 64);

        let mut output_buffer = [0; 64];
        for i in 0..block_count {
            let x = (i % block_width) * dct_scale;
            let y = (i / block_width) * dct_scale;

            let coefficients: &[i16; 64] = &data[i * 64..(i + 1) * 64].try_into().unwrap();

            // Write to a temporary intermediate buffer, a 8x8 'image'.
            dequantize_and_idct_block(
                dct_scale,
                coefficients,
                &*quantization_table,
                8,
                &mut output_buffer,
            );

            let write_back = &mut result_block[y * line_stride + x..];

            let buffered_lines = output_buffer.chunks_mut(8);
            let back_lines = write_back.chunks_mut(line_stride);

            for (buf, back) in buffered_lines.zip(back_lines).take(dct_scale) {
                back[..dct_scale].copy_from_slice(&buf[..dct_scale]);
            }
        }
    }
}

impl Scoped {
    fn task_queue_len(&self) -> usize {
        self.task_queues
            .iter()
            .map(|q| q.len())
            .sum()
    }

    fn drain_task_queues(&mut self) {
        let inner = &mut self.inner;
        let [res0, res1, res2, res3] = &mut inner.results;

        // Lazily get the blocks. Note: if we've already collected results from a component
        // then the result vector has already been deallocated/taken. But no more tasks should
        // be created for it.
        let mut result_blocks = [
            res0.get_mut(inner.offsets[0]..).unwrap_or(&mut []),
            res1.get_mut(inner.offsets[1]..).unwrap_or(&mut []),
            res2.get_mut(inner.offsets[2]..).unwrap_or(&mut []),
            res3.get_mut(inner.offsets[3]..).unwrap_or(&mut []),
        ];

        let result_blocks = &mut result_blocks;
        let offsets = &mut inner.offsets;

        let mut work_items = vec![];

        for index in 0..MAX_COMPONENTS {
            let tasks = self.task_queues[index]
                .drain(..)
                .map(|item| {
                    let WorkItem { quantization_table, metadata, data } = item;
                    offsets[index] += metadata.bytes_used();
                    let (result_block, tail) =
                        core::mem::take(&mut result_blocks[index]).split_at_mut(metadata.bytes_used());
                    result_blocks[index] = tail;
                    (quantization_table, metadata, data, result_block)
                });
            work_items.extend(tasks);
        }

        work_items.par_drain(..).for_each(
            |(quantization_table, metadata, data, result_block)| {
                ImmediateWorker::append_row_locked(
                    quantization_table,
                    metadata,
                    data,
                    result_block,
                )
            },
        );
    }
}

impl super::Worker for Scoped {
    fn start(&mut self, row_data: RowData) -> Result<()> {
        self.inner.start_immediate(row_data);
        Ok(())
    }

    fn append_row(&mut self, row: (usize, Vec<i16>)) -> Result<()> {
        self.sync += 1;
        let inner = &mut self.inner;
        let (index, data) = row;

        let quantization_table = inner.quantization_tables[index].as_ref().unwrap().clone();
        let metadata = inner.component_metadata(index).unwrap();

        self.task_queues[index].push(WorkItem { quantization_table, metadata, data });
        Ok(())
    }

    fn get_result(&mut self, index: usize) -> Result<Vec<u8>> {
        self.drain_task_queues();
        let result = self.inner.get_result_immediate(index);
        Ok(result)
    }

    // Magic sauce, these _may_ run in parallel.
    fn append_rows(&mut self, iter: &mut dyn Iterator<Item = (usize, Vec<i16>)>) -> Result<()> {
        let metadatas = [
            self.inner.component_metadata(0),
            self.inner.component_metadata(1),
            self.inner.component_metadata(2),
            self.inner.component_metadata(3),
        ];

        while {
            if self.task_queue_len() > 4096 {
                self.drain_task_queues();
            }

            let quantization_tables = &mut self.inner.quantization_tables;
            let task_queue = &mut self.task_queues;

            let work_items = iter.take(1024).fold(0, move |ctr, (index, data)| {
                let metadata = metadatas[index].unwrap();
                let quantization_table = quantization_tables[index].as_ref().unwrap().clone();

                task_queue[index].push(WorkItem {quantization_table, metadata, data });
                ctr + 1
            });

            self.parallel += work_items;

            work_items > 0
        } {}

        Ok(())
    }
}

impl ComponentMetadata {
    fn bytes_used(&self) -> usize {
        self.block_count * self.dct_scale * self.dct_scale
    }
}

pub fn compute_image_parallel(
    components: &[Component],
    data: Vec<Vec<u8>>,
    output_size: Dimensions,
    is_jfif: bool,
    color_transform: Option<AdobeColorTransform>,
) -> Result<Vec<u8>> {
    let color_convert_func = choose_color_convert_func(components.len(), is_jfif, color_transform)?;
    let upsampler = Upsampler::new(components, output_size.width, output_size.height)?;
    let line_size = output_size.width as usize * components.len();
    let mut image = vec![0u8; line_size * output_size.height as usize];

    image
        .par_chunks_mut(line_size)
        .with_max_len(1)
        .enumerate()
        .for_each(|(row, line)| {
            upsampler.upsample_and_interleave_row(
                &data,
                row,
                output_size.width as usize,
                line,
                color_convert_func,
            );
        });

    Ok(image)
}
