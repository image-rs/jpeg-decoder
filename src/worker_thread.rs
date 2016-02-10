use error::Result;
use euclid::Point2D;
use idct::idct;
use parser::Component;
use rayon::par_iter::*;
use std::mem;
use std::sync::mpsc::{channel, Sender};
use std::thread;

pub struct RowData {
    pub index: usize,
    pub component: Component,
    pub blocks: Vec<[i32; 64]>,
    pub quantization_table: [u8; 64],
}

pub enum WorkerMsg {
    AppendRow(RowData),
    GetResult((usize, Sender<Vec<u8>>)),
}

pub fn spawn_worker_thread(component_count: usize) -> Result<Sender<WorkerMsg>> {
    let thread_builder = thread::Builder::new().name("worker thread".to_owned());
    let (tx, rx) = channel();

    try!(thread_builder.spawn(move || {
        let mut results = vec![Vec::new(); component_count];

        while let Ok(message) = rx.recv() {
            match message {
                WorkerMsg::AppendRow(data) => {
                    let mut samples = samples_from_mcu_row(&data.component, &data.blocks, &data.quantization_table);
                    results[data.index].append(&mut samples);
                },
                WorkerMsg::GetResult((index, chan)) => {
                    let result = mem::replace(&mut results[index], Vec::new());
                    let _ = chan.send(result);
                },
            }
        }
    }));

    Ok(tx)
}

fn samples_from_mcu_row(component: &Component, blocks: &[[i32; 64]], quantization_table: &[u8; 64]) -> Vec<u8> {
    let mcus_per_row = component.block_size.width / component.horizontal_sampling_factor as u16;
    let blocks_per_mcu = component.horizontal_sampling_factor * component.vertical_sampling_factor;
    let block_count = mcus_per_row as usize * blocks_per_mcu as usize;
    let blocks_per_row = block_count / component.vertical_sampling_factor as usize;
    let line_stride = mcus_per_row as usize * component.horizontal_sampling_factor as usize * 8;

    assert_eq!(blocks.len(), block_count);

    let mut buffer = vec![0u8; block_count * 64];
    let mut coefficients = [0i32; 64];
    let mut samples = [0u8; 64];

    for i in 0 .. block_count {
        for j in 0 .. 64 {
            coefficients[j] = blocks[i][j] * quantization_table[j] as i32;
        }

        idct(&coefficients, &mut samples);

        let coords = Point2D::new(i % blocks_per_row, i / blocks_per_row) * 8;

        for y in 0 .. 8 {
            for x in 0 .. 8 {
                buffer[(coords.y + y) * line_stride + coords.x + x] = samples[y * 8 + x];
            }
        }
    }

    buffer
}

pub fn samples_from_coefficients(component: &Component, coefficients: &[[i32; 64]], quantization_table: &[u8; 64]) -> Vec<u8> {
    let mcu_row_count = component.block_size.height as usize / component.vertical_sampling_factor as usize;
    let mcus_per_row = component.block_size.width as usize / component.horizontal_sampling_factor as usize;
    let blocks_per_mcu = component.horizontal_sampling_factor as usize * component.vertical_sampling_factor as usize;
    let row_stride = mcus_per_row * blocks_per_mcu;

    let mut row_buffers = Vec::new();

    (0 .. mcu_row_count)
            .into_par_iter()
            .weight_max()
            .map(|i| samples_from_mcu_row(component, &coefficients[i * row_stride .. (i + 1) * row_stride], quantization_table))
            .collect_into(&mut row_buffers);

    row_buffers.into_iter()
               .fold(Vec::with_capacity(coefficients.len() * 64), |mut acc, mut row_buffer| { acc.append(&mut row_buffer); acc })
}
