use std::io::Read;

use crate::Error;

/// A `no_std` compliant replacement for [std::io::Read].
pub trait JpegRead {
    /// Read the exact number of bytes required to fill buf.
    ///
    /// See [std::io::Read::read_exact]
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), Error>;

    /// Skip `length` amount of bytes
    fn skip_bytes(&mut self, length: usize) -> Result<(), Error>;

    /// Read a single `u8` value
    fn read_u8(&mut self) -> Result<u8, Error> {
        let mut buf = [0];
        self.read_exact(&mut buf)?;
        Ok(buf[0])
    }

    /// Read a single big endian encoded `u16` value
    fn read_u16_from_be(&mut self) -> Result<u16, Error> {
        let mut buf = [0, 0];
        self.read_exact(&mut buf)?;
        Ok(u16::from_be_bytes(buf))
    }
}

impl<T: Read> JpegRead for T {
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        Ok(std::io::Read::read_exact(self, buf)?)
    }

    fn skip_bytes(&mut self, length: usize) -> Result<(), Error> {
        let length = length as u64;
        let to_skip = &mut Read::by_ref(self).take(length);
        let copied = std::io::copy(to_skip, &mut std::io::sink())?;
        if copied < length {
            Err(Error::Io(std::io::ErrorKind::UnexpectedEof.into()))
        } else {
            Ok(())
        }
    }
}
