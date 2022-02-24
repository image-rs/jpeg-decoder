//! This module implements per-component parallelism.
//! It should be possible to implement per-row parallelism as well,
//! which should also boost performance of grayscale images
//! and allow scaling to more cores.
//! However, that would be more complex, so we use this as a starting point.

use std::{mem, sync::mpsc::{self, Receiver, Sender}};
use crate::decoder::MAX_COMPONENTS;
use crate::error::Result;
use super::{RowData, Worker};
use super::immediate::ImmediateWorker;

pub fn with_multithreading<T>(f: impl FnOnce(&mut dyn Worker) -> T) -> T {
    #[cfg(not(feature = "rayon"))]
    return self::enter_threads(f);

    #[cfg(feature = "rayon")]
    return jpeg_rayon::enter(|mut worker| {
        f(&mut worker)
    });
}

enum WorkerMsg {
    Start(RowData),
    AppendRow(Vec<i16>),
    GetResult(Sender<Vec<u8>>),
}

#[derive(Default)]
pub struct MpscWorker {
    senders: [Option<Sender<WorkerMsg>>; MAX_COMPONENTS]
}

pub struct StdThreadWorker(MpscWorker);

impl MpscWorker {
    fn start_with(
        &mut self,
        row_data: RowData,
        spawn_worker: impl FnOnce(usize) -> Result<Sender<WorkerMsg>>,
    ) -> Result<()> {
        // if there is no worker thread for this component yet, start one
        let component = row_data.index;
        if let None = self.senders[component] {
            let sender = spawn_worker(component)?;
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

    fn get_result_with(
        &mut self,
        index: usize,
        collect: impl FnOnce(Receiver<Vec<u8>>) -> Vec<u8>,
    ) -> Result<Vec<u8>> {
        let (tx, rx) = mpsc::channel();
        let sender = mem::replace(&mut self.senders[index], None).unwrap();
        sender.send(WorkerMsg::GetResult(tx)).expect("jpeg-decoder worker thread error");
        Ok(collect(rx))
    }
}

impl Worker for StdThreadWorker {
    fn start(&mut self, row_data: RowData) -> Result<()> {
        self.0.start_with(row_data, spawn_worker_thread)
    }
    fn append_row(&mut self, row: (usize, Vec<i16>)) -> Result<()> {
        self.0.append_row(row)
    }
    fn get_result(&mut self, index: usize) -> Result<Vec<u8>> {
        self.0.get_result_with(index, collect_worker_thread)
    }
}

fn create_worker() -> (Sender<WorkerMsg>, impl FnOnce() + 'static) {
    let (tx, rx) = mpsc::channel();
    let closure = move || {
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
    };

    (tx, closure)
}

fn spawn_worker_thread(component: usize) -> Result<Sender<WorkerMsg>> {
    let (tx, worker) = create_worker();
    let thread_builder =
        std::thread::Builder::new().name(format!("worker thread for component {}", component));
    thread_builder.spawn(worker)?;
    Ok(tx)
}


fn collect_worker_thread(rx: Receiver<Vec<u8>>) -> Vec<u8> {
    rx.recv().expect("jpeg-decoder worker thread error")
}

#[allow(dead_code)]
fn enter_threads<T>(f: impl FnOnce(&mut dyn Worker) -> T) -> T {
    let mut worker = StdThreadWorker(MpscWorker::default());
    f(&mut worker)
}


#[cfg(feature = "rayon")]
mod jpeg_rayon {
    use crate::error::Result;
    use super::{MpscWorker, RowData};

    pub struct Scoped<'r, 'scope> {
        fifo: &'r rayon::ScopeFifo<'scope>,
        inner: MpscWorker,
    }

    pub fn enter<T>(f: impl FnOnce(Scoped) -> T) -> T {
        // Note: Must be at least two threads. Otherwise, we may deadlock, due to ordering
        // constraints that we can not impose properly. Note that `append_row` creates a new task
        // while in `get_result` we wait for all tasks of a component. The only way for rayon to
        // impose this wait __and get a result__ is by ending an in_place_scope.
        //
        // However, the ordering of tasks is not as FIFO as the name would suggest. Indeed, even
        // though tasks are spawned in `start` _before_ the task spawned in `get_result`, the
        // `in_place_scope_fifo` will wait for ITS OWN results in fifo order. This implies, unless
        // there is some other thread capable of stealing the worker the work task will in fact not
        // get executed and the result will wait forever. It is impossible to otherwise schedule
        // the worker tasks specifically (e.g. join handle would be cool *cough* if you read this
        // and work on rayon) before while yielding from the current thread.
        //
        // So: we need at least one more worker thread that is _not_ occupied.
        let threads = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();

        threads.in_place_scope_fifo(|fifo| {
            f(Scoped { fifo, inner: MpscWorker::default() })
        })
    }

    impl super::Worker for Scoped<'_, '_> {
        fn start(&mut self, row_data: RowData) -> Result<()> {
            let fifo = &mut self.fifo;
            self.inner.start_with(row_data, |_| {
                let (tx, worker) = super::create_worker();
                fifo.spawn_fifo(move |_| {
                    worker()
                });
                Ok(tx)
            })
        }

        fn append_row(&mut self, row: (usize, Vec<i16>)) -> Result<()> {
            self.inner.append_row(row)
        }

        fn get_result(&mut self, index: usize) -> Result<Vec<u8>> {
            self.inner.get_result_with(index, |rx| {
                let mut result = vec![];
                let deliver_result = &mut result;

                rayon::in_place_scope_fifo(|scope| {
                    scope.spawn_fifo(move |_| {
                        *deliver_result = rx.recv().expect("jpeg-decoder worker thread error");
                    });
                });

                result
            })
        }
    }
}

