use decoder::MAX_COMPONENTS;
use error::Result;
use euclid::Point2D;
use idct::dequantize_and_idct;
use parser::Component;
use rayon::par_iter::*;
use std::mem;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;

pub struct RowData {
    pub index: usize,
    pub component: Component,
    pub quantization_table: Arc<[u8; 64]>,
}

pub enum WorkerMsg {
    Start(RowData),
    AppendRow((usize, Vec<i16>)),
    GetResult((usize, Sender<Vec<u8>>)),
}

pub fn spawn_worker_thread() -> Result<Sender<WorkerMsg>> {
    let thread_builder = thread::Builder::new().name("worker thread".to_owned());
    let (tx, rx) = mpsc::channel();

    try!(thread_builder.spawn(move || {
        let mut offsets = [0; MAX_COMPONENTS];
        let mut results = vec![Vec::new(); MAX_COMPONENTS];
        let mut components = vec![None; MAX_COMPONENTS];
        let mut quantization_tables = vec![None; MAX_COMPONENTS];
        let mut samples = [0u8; 64];

        while let Ok(message) = rx.recv() {
            match message {
                WorkerMsg::Start(data) => {
                    assert!(results[data.index].is_empty());

                    offsets[data.index] = 0;
                    results[data.index].resize(data.component.block_size.width as usize * data.component.block_size.height as usize * 64, 0u8);
                    components[data.index] = Some(data.component);
                    quantization_tables[data.index] = Some(data.quantization_table);
                },
                WorkerMsg::AppendRow((index, data)) => {
                    // Convert coefficients from a MCU row to samples.

                    let component = components[index].as_ref().unwrap();
                    let quantization_table = quantization_tables[index].as_ref().unwrap();
                    let block_count = component.block_size.width as usize * component.vertical_sampling_factor as usize;
                    let line_stride = component.block_size.width as usize * 8;
                    let output = &mut results[index][offsets[index] ..];

                    assert_eq!(data.len(), block_count * 64);

                    for i in 0 .. block_count {
                        let coords = Point2D::new(i % component.block_size.width as usize, i / component.block_size.width as usize) * 8;
                        dequantize_and_idct(&data[i * 64 .. (i + 1) * 64], quantization_table, &mut samples);

                        for y in 0 .. 8 {
                            for x in 0 .. 8 {
                                output[(coords.y + y) * line_stride + coords.x + x] = samples[y * 8 + x];
                            }
                        }
                    }

                    offsets[index] += data.len();
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
