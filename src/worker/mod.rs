mod immediate;

#[cfg(feature = "std")]
mod multithreaded;

#[cfg(all(feature = "std", not(any(target_arch = "asmjs", target_arch = "wasm32"))))]
pub use self::multithreaded::MultiThreadedWorker as PlatformWorker;
#[cfg(any(not(feature = "std"), target_arch = "asmjs", target_arch = "wasm32"))]
pub use self::immediate::ImmediateWorker as PlatformWorker;

use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::error::Result;
use crate::parser::Component;

pub struct RowData {
    pub index: usize,
    pub component: Component,
    pub quantization_table: Arc<[u16; 64]>,
}

pub trait Worker: Sized {
    fn new() -> Result<Self>;
    fn start(&mut self, row_data: RowData) -> Result<()>;
    fn append_row(&mut self, row: (usize, Vec<i16>)) -> Result<()>;
    fn get_result(&mut self, index: usize) -> Result<Vec<u8>>;
}
