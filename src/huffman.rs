use byteorder::ReadBytesExt;
use error::{Error, Result};
use marker::Marker;
use std::io::Read;
use std::iter::repeat;

const LUT_BITS: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HuffmanTableClass {
    DC,
    AC,
}

pub struct HuffmanTable {
    values: Vec<u8>,
    value_offset: [isize; 16],
    maxcode: [isize; 16],
    lut: [(u8, u8); 1 << LUT_BITS],
    fast_ac: Option<[i16; 1 << LUT_BITS]>,
}

impl HuffmanTable {
    pub fn new(bits: &[u8; 16], values: &[u8], class: HuffmanTableClass) -> Result<HuffmanTable> {
        let (huffcode, huffsize) = try!(derive_huffman_codes(bits));

        // Section F.2.2.3
        // Figure F.15

        // value_offset[i] is set to VALPTR(I) - MINCODE(I).
        let mut value_offset = [0isize; 16];
        let mut maxcode = [-1isize; 16];
        let mut j = 0;

        for i in 0 .. 16 {
            if bits[i] != 0 {
                value_offset[i] = j as isize - huffcode[j] as isize;
                j += bits[i] as usize;
                maxcode[i] = huffcode[j - 1] as isize;
            }
        }

        let mut lut = [(0u8, 0u8); 1 << LUT_BITS];

        for (i, &value) in values.iter().enumerate().filter(|&(i, _)| huffsize[i] <= LUT_BITS) {
            let bits_remaining = LUT_BITS - huffsize[i];
            let start = (huffcode[i] << bits_remaining) as usize;

            for j in 0 .. 1 << bits_remaining {
                lut[start + j] = (value, huffsize[i]);
            }
        }

        let mut fast_ac = None;

        if class == HuffmanTableClass::AC {
            let mut table = [0i16; 1 << LUT_BITS];

            for (i, &(value, size)) in lut.iter().enumerate() {
                if value < 255 {
                    let run = (value >> 4) & 0x0f;
                    let magnitude_bits = value & 0x0f;

                    if magnitude_bits > 0 && size + magnitude_bits <= LUT_BITS {
                        let unextended_ac_value = ((i << size) & ((1 << LUT_BITS) - 1)) >> (LUT_BITS - magnitude_bits);
                        let ac_value = extend(unextended_ac_value as i32, magnitude_bits);

                        if ac_value >= -128 && ac_value <= 127 {
                            table[i] = ((ac_value as i16) << 8) + ((run as i16) << 4) + (size + magnitude_bits) as i16;
                        }
                    }
                }
            }

            fast_ac = Some(table);
        }

        Ok(HuffmanTable {
            values: values.to_vec(),
            value_offset: value_offset,
            maxcode: maxcode,
            lut: lut,
            fast_ac: fast_ac,
        })
    }
}

fn derive_huffman_codes(bits: &[u8; 16]) -> Result<(Vec<u16>, Vec<u8>)> {
    // Figure C.1
    let huffsize = bits.iter()
                       .enumerate()
                       .fold(Vec::new(), |mut acc, (i, &value)| {
                           let mut repeated_size: Vec<u8> = repeat((i + 1) as u8).take(value as usize).collect();
                           acc.append(&mut repeated_size);
                           acc
                       });

    // Figure C.2
    let mut huffcode = vec![0u16; huffsize.len()];
    let mut size = *huffsize.first().unwrap_or(&0);
    let mut code = 0u16;

    for (i, &v) in huffsize.iter().enumerate() {
        while size != v {
            code <<= 1;
            size += 1;
        }

        if code as u32 >= (1u32 << size) {
            return Err(Error::Format("bad huffman code length".to_owned()));
        }

        huffcode[i] = code;
        code += 1;
    }

    Ok((huffcode, huffsize))
}

// Section F.2.2.1
// Figure F.12
fn extend(value: i32, count: u8) -> i32 {
    let vt = 1 << (count as u32 - 1);

    if value < vt {
        value + (-1 << count as i32) + 1
    } else {
        value
    }
}

#[derive(Debug)]
pub struct HuffmanDecoder {
    bits: u32,
    num_bits: u8,
    marker: Option<Marker>,
}

impl HuffmanDecoder {
    pub fn new() -> HuffmanDecoder {
        HuffmanDecoder {
            bits: 0,
            num_bits: 0,
            marker: None,
        }
    }

    pub fn take_marker(&mut self) -> Option<Marker> {
        self.marker.take()
    }

    pub fn reset(&mut self) {
        self.bits = 0;
        self.num_bits = 0;
    }

    // Section F.2.2.3
    // Figure F.16
    pub fn decode<R: Read>(&mut self, reader: &mut R, table: &HuffmanTable) -> Result<u8> {
        if self.num_bits < 16 {
            try!(self.read_bits(reader));
        }

        let index = ((self.bits >> (32 - LUT_BITS)) & ((1 << LUT_BITS) - 1)) as usize;
        let (value, size) = table.lut[index];

        if size > 0 {
            self.consume_bits(size);
            return Ok(value);
        }

        let mut code = 0;

        for i in 0 .. 16 {
            code |= self.next_bit() as u16;

            if code as isize <= table.maxcode[i] {
                let index = code as isize + table.value_offset[i];
                return Ok(table.values[index as usize]);
            }

            code <<= 1;
        }

        Err(Error::Format("failed to decode huffman code".to_owned()))
    }

    pub fn decode_fast_ac<R: Read>(&mut self, reader: &mut R, table: &HuffmanTable) -> Result<Option<(i32, u8)>> {
        if let Some(ref fast_ac) = table.fast_ac {
            if self.num_bits < LUT_BITS {
                try!(self.read_bits(reader));
            }

            let index = ((self.bits >> (32 - LUT_BITS)) & ((1 << LUT_BITS) - 1)) as usize;
            let value = fast_ac[index];

            if value != 0 {
                let run = ((value >> 4) & 0x0f) as u8;
                let size = (value & 0x0f) as u8;

                self.consume_bits(size);
                return Ok(Some(((value >> 8) as i32, run)));
            }
        }

        Ok(None)
    }

    pub fn receive<R: Read>(&mut self, reader: &mut R, count: u8) -> Result<u32> {
        if self.num_bits < count {
            try!(self.read_bits(reader));

            if self.num_bits < count {
                return Err(Error::Format("not enough bits in huffman receive".to_owned()));
            }
        }

        // Section F.2.2.4
        // Figure F.17
        let mask = 0xffffffff << (32 - count as usize);
        let value = (self.bits & mask) >> (32 - count as usize);

        self.consume_bits(count);

        Ok(value)
    }

    pub fn receive_extend<R: Read>(&mut self, reader: &mut R, count: u8) -> Result<i32> {
        let value = try!(self.receive(reader, count)) as i32;
        Ok(extend(value, count))
    }

    // Section F.2.2.5
    // Figure F.18
    #[inline]
    fn next_bit(&mut self) -> u8 {
        let bit = ((self.bits & (1 << 31)) >> 31) as u8;
        self.consume_bits(1);

        bit
    }

    fn read_bits<R: Read>(&mut self, reader: &mut R) -> Result<()> {
        while self.num_bits < 25 {
            // Fill with zero bits if we have reached the end.
            let byte = match self.marker {
                Some(_) => 0,
                None => try!(reader.read_u8()),
            };

            if byte == 0xFF {
                let mut next_byte = try!(reader.read_u8());

                // Check for byte stuffing.
                if next_byte != 0x00 {
                    // We seem to have reached the end of entropy-coded data and encountered a
                    // marker. Since we can't put data back into the reader, we have to continue
                    // reading to identify the marker so we can pass it on.

                    // Section B.1.1.2
                    // "Any marker may optionally be preceded by any number of fill bytes, which are bytes assigned code X’FF’."
                    while next_byte == 0xFF {
                        next_byte = try!(reader.read_u8());
                    }

                    match next_byte {
                        0x00 => return Err(Error::Format("FF 00 found where marker was expected".to_owned())),
                        _    => self.marker = Some(Marker::from_u8(next_byte).unwrap()),
                    }

                    continue;
                }
            }

            self.bits |= (byte as u32) << (24 - self.num_bits);
            self.num_bits += 8;
        }

        Ok(())
    }

    #[inline]
    fn consume_bits(&mut self, count: u8) {
        debug_assert!(self.num_bits >= count);

        self.bits <<= count as usize;
        self.num_bits -= count;
    }
}
