#[cfg(feature = "std")]
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

#[cfg(feature = "std")]
impl<T: Read> JpegRead for T {
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        Ok(Read::read_exact(self, buf)?)
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

#[cfg(not(feature = "std"))]
impl<W: JpegRead + ?Sized> JpegRead for &mut W {
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        (**self).read_exact(buf)
    }

    fn skip_bytes(&mut self, length: usize) -> Result<(), Error> {
        (**self).skip_bytes(length)
    }
}

#[cfg(not(feature = "std"))]
impl JpegRead for &[u8] {
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        if buf.len() <= self.len() {
            let (data, remaining) = self.split_at(buf.len());
            buf.copy_from_slice(data);
            *self = remaining;
            Ok(())
        } else {
            panic!();
        }
    }

    fn skip_bytes(&mut self, length: usize) -> Result<(), Error> {
        if length <= self.len() {
            let (_, remaining) = self.split_at(length);
            *self = remaining;
            Ok(())
        } else {
            panic!();
        }
    }
}

