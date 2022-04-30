mod immediate;
mod multithreaded;
#[cfg(feature = "rayon")]
mod rayon;

use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::error::Result;
use crate::parser::Component;

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
    fn append_rows(&mut self, row: &mut dyn Iterator<Item=(usize, Vec<i16>)>)
        -> Result<()>
    {
        for item in row {
            self.append_row(item)?;
        }
        Ok(())
    }
}

pub enum PreferWorkerKind {
    Immediate,
    Multithreaded,
}


/// Execute something with a worker system.
pub fn with_worker<T>(prefer: PreferWorkerKind, f: impl FnOnce(&mut dyn Worker) -> T) -> T {
    #[cfg(any(target_arch = "asmjs", target_arch = "wasm32"))]
    {
        return self::immediate::with_immediate(f);
    }
    match prefer {
        #[cfg(feature = "rayon")]
        PreferWorkerKind::Multithreaded => self::rayon::with_rayon(f),
        PreferWorkerKind::Multithreaded => self::multithreaded::with_multithreading(f),
        _ => self::immediate::with_immediate(f),
    }
}

