//! This module implements per-component parallelism.
//! It should be possible to implement per-row parallelism as well,
//! which should also boost performance of grayscale images
//! and allow scaling to more cores.
//! However, that would be more complex, so we use this as a starting point.

use std::{mem, io, sync::mpsc::{self, Sender}};
use crate::decoder::MAX_COMPONENTS;
use crate::error::Result;
use super::{RowData, Worker};
use super::immediate::ImmediateWorker;

enum WorkerMsg {
    Start(RowData),
    AppendRow(Vec<i16>),
    GetResult(Sender<Vec<u8>>),
}
pub struct MultiThreadedWorker {
    senders: [Option<Sender<WorkerMsg>>; MAX_COMPONENTS]
}

impl Worker for MultiThreadedWorker {
    fn new() -> Result<Self> {
        Ok(MultiThreadedWorker {
            senders: [None, None, None, None]
        })
    }
    fn start(&mut self, row_data: RowData) -> Result<()> {
        // if there is no worker thread for this component yet, start one
        let component = row_data.index;
        if let None = self.senders[component] {
            let sender = spawn_worker_thread(component)?;
            self.senders[component] = Some(sender);
        }
        // we do the "take out value and put it back in once we're done" dance here
        // and in all other message-passing methods because there's not that many rows
        // and this should be cheaper than spawning MAX_COMPONENTS many threads up front
        let sender = mem::replace(&mut self.senders[component], None).unwrap();
        sender.send(WorkerMsg::Start(row_data)).expect("jpeg-decoder worker thread error");
        self.senders[component] = Some(sender);
        Ok(())
    }
    fn append_row(&mut self, row: (usize, Vec<i16>)) -> Result<()> {
        let component = row.0;
        let sender = mem::replace(&mut self.senders[component], None).unwrap();
        sender.send(WorkerMsg::AppendRow(row.1)).expect("jpeg-decoder worker thread error");
        self.senders[component] = Some(sender);
        Ok(())
    }
    fn get_result(&mut self, index: usize) -> Result<Vec<u8>> {
        let (tx, rx) = mpsc::channel();
        let sender = mem::replace(&mut self.senders[index], None).unwrap();
        sender.send(WorkerMsg::GetResult(tx)).expect("jpeg-decoder worker thread error");
        Ok(rx.recv().expect("jpeg-decoder worker thread error"))
    }
}

fn spawn_worker_thread(component: usize) -> Result<Sender<WorkerMsg>> {
    let (tx, rx) = mpsc::channel();

    spawn(component, move || {
        let mut worker = ImmediateWorker::new_immediate();

        while let Ok(message) = rx.recv() {
            match message {
                WorkerMsg::Start(mut data) => {
                    // we always set component index to 0 for worker threads
                    // because they only ever handle one per thread and we don't want them
                    // to attempt to access nonexistent components
                    data.index = 0;
                    worker.start_immediate(data);
                },
                WorkerMsg::AppendRow(row) => {
                    worker.append_row_immediate((0, row));
                },
                WorkerMsg::GetResult(chan) => {
                    let _ = chan.send(worker.get_result_immediate(0));
                    break;
                },
            }
        }
    })?;

    Ok(tx)
}

#[cfg(feature = "rayon")]
fn spawn<F>(_component: usize, func: F) -> io::Result<()>
where
    F: FnOnce() + Send + 'static,
{
    rayon::spawn(func);
    Ok(())
}

#[cfg(not(feature = "rayon"))]
fn spawn<F>(component: usize, func: F) -> io::Result<()>
where
    F: FnOnce() + Send + 'static,
{
    let thread_builder =
        std::thread::Builder::new().name(format!("worker thread for component {}", component));
    thread_builder.spawn(func)?;
    Ok(())
}
