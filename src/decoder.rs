use byteorder::ReadBytesExt;
use error::{Error, Result, UnsupportedFeature};
use euclid::{Point2D, Rect, Size2D};
use huffman::{HuffmanDecoder, HuffmanTable};
use marker::Marker;
use parser::{AdobeColorTransform, AppData, CodingProcess, Component, EntropyCoding, FrameInfo,
             parse_app, parse_com, parse_dht, parse_dqt, parse_dri, parse_sof, parse_sos};
use rayon::par_iter::*;
use resampler::Resampler;
use std::cmp;
use std::io::Read;
use std::mem;
use std::sync::mpsc::channel;
use worker_thread::{RowData, samples_from_coefficients, spawn_worker_thread, WorkerMsg};

const MAX_COMPONENTS: usize = 4;

static UNZIGZAG: [u8; 64] = [
     0,  1,  8, 16,  9,  2,  3, 10,
    17, 24, 32, 25, 18, 11,  4,  5,
    12, 19, 26, 33, 40, 48, 41, 34,
    27, 20, 13,  6,  7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36,
    29, 22, 15, 23, 30, 37, 44, 51,
    58, 59, 52, 45, 38, 31, 39, 46,
    53, 60, 61, 54, 47, 55, 62, 63,
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PixelFormat {
    L8,     // Luminance, 8 bits per channel
    RGB24,  // RGB, 8 bits per channel
    CMYK32, // CMYK, 8 bits per channel
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageInfo {
    pub width: u16,
    pub height: u16,
    pub pixel_format: PixelFormat,
}

pub struct Decoder<R> {
    reader: R,

    component_data: Vec<Vec<u8>>,

    // Used for progressive JPEGs.
    coefficients: Vec<Vec<[i32; 64]>>,

    frame: Option<FrameInfo>,
    dc_huffman_tables: Vec<Option<HuffmanTable>>,
    ac_huffman_tables: Vec<Option<HuffmanTable>>,
    quantization_tables: [Option<[u8; 64]>; 4],

    restart_interval: u16,
    color_transform: Option<AdobeColorTransform>,
    is_jfif: bool,
}

impl<R: Read> Decoder<R> {
    pub fn new(reader: R) -> Decoder<R> {
        Decoder {
            reader: reader,
            component_data: Vec::new(),
            coefficients: Vec::new(),
            frame: None,
            dc_huffman_tables: vec![None, None, None, None],
            ac_huffman_tables: vec![None, None, None, None],
            quantization_tables: [None; 4],
            restart_interval: 0,
            color_transform: None,
            is_jfif: false,
        }
    }

    pub fn info(&self) -> Option<ImageInfo> {
        match self.frame {
            Some(ref frame) => {
                let pixel_format = match frame.components.len() {
                    1 => PixelFormat::L8,
                    3 => PixelFormat::RGB24,
                    4 => PixelFormat::CMYK32,
                    _ => panic!(),
                };

                Some(ImageInfo {
                    width: frame.image_size.width,
                    height: frame.image_size.height,
                    pixel_format: pixel_format,
                })
            },
            None => None,
        }
    }

    pub fn read_info(&mut self) -> Result<()> {
        self.decode_internal(true).map(|_| ())
    }

    pub fn decode(&mut self) -> Result<Vec<u8>> {
        self.decode_internal(false)
    }

    fn decode_internal(&mut self, stop_after_metadata: bool) -> Result<Vec<u8>> {
        if stop_after_metadata && self.frame.is_some() {
            // The metadata has already been read.
            return Ok(Vec::new());
        }
        else if self.frame.is_none() && (try!(self.reader.read_u8()) != 0xFF || try!(self.reader.read_u8()) != Marker::SOI as u8) {
            return Err(Error::Format("first two bytes is not a SOI marker".to_owned()));
        }

        let mut previous_marker = Marker::SOI;
        let mut pending_marker = None;
        let mut scans_processed = 0;

        loop {
            let marker = match pending_marker.take() {
                Some(m) => m,
                None => try!(self.read_marker()),
            };

            match marker {
                // Frame header
                Marker::SOF0 | Marker::SOF1 | Marker::SOF2 | Marker::SOF3 | Marker::SOF5 |
                Marker::SOF6 | Marker::SOF7 | Marker::SOF9 | Marker::SOF10 | Marker::SOF11 |
                Marker::SOF13 | Marker::SOF14 | Marker::SOF15 => {
                    // Section 4.10
                    // "An image contains only one frame in the cases of sequential and
                    //  progressive coding processes; an image contains multiple frames for the
                    //  hierarchical mode."
                    if self.frame.is_some() {
                        return Err(Error::Unsupported(UnsupportedFeature::Hierarchical));
                    }

                    let frame = try!(parse_sof(&mut self.reader, marker));

                    if frame.is_differential {
                        return Err(Error::Unsupported(UnsupportedFeature::Hierarchical));
                    }
                    if frame.coding_process == CodingProcess::Lossless {
                        return Err(Error::Unsupported(UnsupportedFeature::Lossless));
                    }
                    if frame.entropy_coding == EntropyCoding::Arithmetic {
                        return Err(Error::Unsupported(UnsupportedFeature::ArithmeticEntropyCoding));
                    }
                    if frame.precision != 8 {
                        return Err(Error::Unsupported(UnsupportedFeature::SamplePrecision(frame.precision)));
                    }
                    if frame.image_size.height == 0 {
                        return Err(Error::Unsupported(UnsupportedFeature::DNL));
                    }
                    if frame.components.len() != 1 && frame.components.len() != 3 && frame.components.len() != 4 {
                        return Err(Error::Unsupported(UnsupportedFeature::ComponentCount(frame.components.len() as u8)));
                    }
                    if Resampler::new(&frame.components).is_none() {
                        return Err(Error::Unsupported(UnsupportedFeature::SubsamplingRatio));
                    }

                    if frame.coding_process == CodingProcess::DctProgressive {
                        for component in &frame.components {
                            let block_count = component.block_size.width as usize * component.block_size.height as usize;

                            // This is a workaround for
                            // "error: the trait `core::clone::Clone` is not implemented for the type `[i32; 64]`".
                            // let coefficients = vec![[0i32; 64]; block_count];
                            let mut coefficients = Vec::with_capacity(block_count);
                            for _ in 0 .. block_count {
                                coefficients.push([0i32; 64]);
                            }

                            self.coefficients.push(coefficients);
                        }
                    }
                    else {
                        self.component_data = vec![Vec::new(); frame.components.len()];
                    }

                    self.frame = Some(frame);

                    if stop_after_metadata {
                        return Ok(Vec::new());
                    }
                },

                // Scan header
                Marker::SOS => {
                    if self.frame.is_none() {
                        return Err(Error::Format("scan encountered before frame".to_owned()));
                    }

                    let frame = self.frame.clone().unwrap();
                    pending_marker = try!(self.decode_scan(&frame));
                    scans_processed += 1;
                },

                // Table-specification and miscellaneous markers
                // Quantization table-specification
                Marker::DQT => {
                    let tables = try!(parse_dqt(&mut self.reader));

                    for (i, &table) in tables.into_iter().enumerate() {
                        if let Some(table) = table {
                            let mut unzigzagged_table = [0u8; 64];

                            for j in 0 .. 64 {
                                unzigzagged_table[UNZIGZAG[j] as usize] = table[j];
                            }

                            self.quantization_tables[i] = Some(unzigzagged_table);
                        }
                    }
                },
                // Huffman table-specification
                Marker::DHT => {
                    let is_baseline = self.frame.as_ref().map(|frame| frame.is_baseline);
                    let (dc_tables, ac_tables) = try!(parse_dht(&mut self.reader, is_baseline));

                    let current_dc_tables = mem::replace(&mut self.dc_huffman_tables, vec![]);
                    self.dc_huffman_tables = dc_tables.into_iter()
                                                      .zip(current_dc_tables.into_iter())
                                                      .map(|(a, b)| a.or(b))
                                                      .collect();

                    let current_ac_tables = mem::replace(&mut self.ac_huffman_tables, vec![]);
                    self.ac_huffman_tables = ac_tables.into_iter()
                                                      .zip(current_ac_tables.into_iter())
                                                      .map(|(a, b)| a.or(b))
                                                      .collect();
                },
                // Arithmetic conditioning table-specification
                Marker::DAC => return Err(Error::Unsupported(UnsupportedFeature::ArithmeticEntropyCoding)),
                // Restart interval definition
                Marker::DRI => self.restart_interval = try!(parse_dri(&mut self.reader)),
                // Comment
                Marker::COM => {
                    let _comment = try!(parse_com(&mut self.reader));
                },
                // Application data
                Marker::APP0 | Marker::APP1 | Marker::APP2 | Marker::APP3 | Marker::APP4 |
                Marker::APP5 | Marker::APP6 | Marker::APP7 | Marker::APP8 | Marker::APP9 |
                Marker::APP10 | Marker::APP11 | Marker::APP12 | Marker::APP13 | Marker::APP14 |
                Marker::APP15 => {
                    if let Some(data) = try!(parse_app(&mut self.reader, marker)) {
                        match data {
                            AppData::Adobe(color_transform) => self.color_transform = Some(color_transform),
                            AppData::Jfif => {
                                // From the JFIF spec:
                                // "The APP0 marker is used to identify a JPEG FIF file.
                                //     The JPEG FIF APP0 marker is mandatory right after the SOI marker."
                                // Some JPEGs in the wild does not follow this though, so we allow
                                // JFIF headers anywhere APP0 markers are allowed.
                                /*
                                if previous_marker != Marker::SOI {
                                    return Err(Error::Format("the JFIF APP0 marker must come right after the SOI marker".to_owned()));
                                }
                                */

                                self.is_jfif = true;
                            },
                        }
                    }
                },

                // Define number of lines
                Marker::DNL => {
                    // Section B.2.1
                    // "If a DNL segment (see B.2.5) is present, it shall immediately follow the first scan."
                    if previous_marker != Marker::SOS || scans_processed != 1 {
                        return Err(Error::Format("DNL is only allowed immediately after the first scan".to_owned()));
                    }

                    return Err(Error::Unsupported(UnsupportedFeature::DNL));
                },

                // Hierarchical mode markers
                Marker::DHP | Marker::EXP => return Err(Error::Unsupported(UnsupportedFeature::Hierarchical)),

                // End of image
                Marker::EOI => break,

                _ => return Err(Error::Format(format!("{:?} marker found where not allowed", marker))),
            }

            previous_marker = marker;
        }

        if scans_processed > 0 {
            let frame = self.frame.as_ref().unwrap();

            if frame.coding_process == CodingProcess::DctProgressive {
                let coefficients = &self.coefficients;
                let quantization_tables = &self.quantization_tables;

                frame.components.par_iter()
                                .enumerate()
                                .weight_max()
                                .map(|(i, component)| samples_from_coefficients(component, &coefficients[i], quantization_tables[component.quantization_table_index].as_ref().unwrap()))
                                .collect_into(&mut self.component_data);
            }

            let image = compute_image(&frame.components, &self.component_data, frame.image_size, self.is_jfif, self.color_transform);

            self.component_data = Vec::new();
            self.coefficients = Vec::new();

            image
        }
        else {
            Err(Error::Format("no data found".to_owned()))
        }
    }

    fn read_marker(&mut self) -> Result<Marker> {
        if try!(self.reader.read_u8()) != 0xFF {
            return Err(Error::Format("did not find marker where expected".to_owned()));
        }

        let mut byte = try!(self.reader.read_u8());

        // Section B.1.1.2
        // "Any marker may optionally be preceded by any number of fill bytes, which are bytes assigned code X’FF’."
        while byte == 0xFF {
            byte = try!(self.reader.read_u8());
        }

        match byte {
            0x00 => Err(Error::Format("FF 00 found where marker was expected".to_owned())),
            _    => Ok(Marker::from_u8(byte).unwrap()),
        }
    }

    fn decode_scan(&mut self, frame: &FrameInfo) -> Result<Option<Marker>> {
        let scan = try!(parse_sos(&mut self.reader, frame));

        assert!(scan.component_indices.len() <= MAX_COMPONENTS);

        let components: Vec<Component> = scan.component_indices.iter()
                                                               .map(|&i| frame.components[i].clone())
                                                               .collect();

        // Verify that all required quantization tables has been set.
        if components.iter().any(|component| self.quantization_tables[component.quantization_table_index].is_none()) {
            return Err(Error::Format("use of unset quantization table".to_owned()));
        }

        // Verify that all required huffman tables has been set.
        if scan.spectral_selection_start == 0 &&
                scan.dc_table_indices.iter().any(|&i| self.dc_huffman_tables[i].is_none()) {
            return Err(Error::Format("scan makes use of unset dc huffman table".to_owned()));
        }
        if scan.spectral_selection_end > 0 &&
                scan.ac_table_indices.iter().any(|&i| self.ac_huffman_tables[i].is_none()) {
            return Err(Error::Format("scan makes use of unset ac huffman table".to_owned()));
        }

        let blocks_per_mcu: Vec<u16> = components.iter()
                                                 .map(|c| c.horizontal_sampling_factor as u16 * c.vertical_sampling_factor as u16)
                                                 .collect();
        let is_progressive = frame.coding_process == CodingProcess::DctProgressive;
        let is_interleaved = components.len() > 1;
        let mut huffman = HuffmanDecoder::new();
        let mut dc_predictors = [0i32; MAX_COMPONENTS];
        let mut restarts_left = self.restart_interval;
        let mut expected_rst_num = 0;
        let mut eob_run = 0;
        let mut worker_chan = None;

        if !is_progressive {
            worker_chan = Some(try!(spawn_worker_thread(components.len())));
        }

        for mcu_y in 0 .. frame.mcu_size.height {
            let mut component_blocks: Vec<Vec<[i32; 64]>>;

            if !is_progressive {
                component_blocks = components.iter()
                        .map(|component| {
                            let blocks_per_mcu_row = component.block_size.width as usize * component.vertical_sampling_factor as usize;

                            // This is a workaround for
                            // "error: the trait `core::clone::Clone` is not implemented for the type `[i32; 64]`".
                            // let blocks = vec![[0i32; 64]; blocks_per_mcu_row];
                            let mut blocks = Vec::with_capacity(blocks_per_mcu_row);
                            for _ in 0 .. blocks_per_mcu_row {
                                blocks.push([0i32; 64]);
                            }

                            blocks
                        }).collect();
            }
            else {
                component_blocks = Vec::new();
            }

            for mcu_x in 0 .. frame.mcu_size.width {
                for (i, component) in components.iter().enumerate() {
                    for j in 0 .. blocks_per_mcu[i] {
                        let block_coords;

                        if is_interleaved {
                            // Section A.2.3

                            block_coords = Point2D::new(
                                    mcu_x * component.horizontal_sampling_factor as u16 + j % component.horizontal_sampling_factor as u16,
                                    mcu_y * component.vertical_sampling_factor as u16 + j / component.horizontal_sampling_factor as u16);
                        }
                        else {
                            // Section A.2.2

                            let blocks_per_row = component.block_size.width as usize;
                            let block_num = (mcu_y as usize * frame.mcu_size.width as usize + mcu_x as usize) * blocks_per_mcu[i] as usize + j as usize;

                            block_coords = Point2D::new((block_num % blocks_per_row) as u16, (block_num / blocks_per_row) as u16);

                            let pixel_coords = block_coords * 8;
                            let component_rect = Rect::new(Point2D::zero(), component.size);

                            if !component_rect.contains(&pixel_coords) {
                                continue;
                            }
                        }

                        let block_index = block_coords.y as usize * component.block_size.width as usize + block_coords.x as usize;
                        let mut coefficients = if is_progressive {
                            self.coefficients[scan.component_indices[i]][block_index]
                        } else {
                            [0i32; 64]
                        };

                        if scan.successive_approximation_high == 0 {
                            let dc_diff = try!(self.decode_block(&mut coefficients,
                                                                 &mut huffman,
                                                                 scan.dc_table_indices[i],
                                                                 scan.ac_table_indices[i],
                                                                 scan.spectral_selection_start,
                                                                 scan.spectral_selection_end,
                                                                 scan.successive_approximation_low,
                                                                 &mut eob_run,
                                                                 dc_predictors[i]));
                            dc_predictors[i] += dc_diff;
                        }
                        else {
                            try!(self.decode_block_successive_approximation(&mut coefficients,
                                                                            &mut huffman,
                                                                            scan.ac_table_indices[i],
                                                                            scan.spectral_selection_start,
                                                                            scan.spectral_selection_end,
                                                                            scan.successive_approximation_low,
                                                                            &mut eob_run));
                        }

                        if is_progressive {
                            self.coefficients[scan.component_indices[i]][block_index] = coefficients;
                        }
                        else {
                            let row_block_index = block_index - mcu_y as usize * component.vertical_sampling_factor as usize * component.block_size.width as usize;
                            component_blocks[i][row_block_index] = coefficients;
                        }
                    }
                }

                if self.restart_interval > 0 {
                    let is_last_mcu = mcu_x == frame.mcu_size.width - 1 && mcu_y == frame.mcu_size.height - 1;
                    restarts_left -= 1;

                    if restarts_left == 0 && !is_last_mcu {
                        let expected_marker = Marker::from_u8(Marker::RST0 as u8 + expected_rst_num).unwrap();

                        match huffman.take_marker() {
                            Some(marker) => {
                                match marker {
                                    Marker::RST0 | Marker::RST1 | Marker::RST2 | Marker::RST3 |
                                    Marker::RST4 | Marker::RST5 | Marker::RST6 | Marker::RST7 => {
                                        if marker != expected_marker {
                                            return Err(Error::Format(format!("found {:?} marker where {:?} was expected", marker, expected_marker)));
                                        }

                                        expected_rst_num = (expected_rst_num + 1) % 8;
                                    },
                                    _ => return Err(Error::Format(format!("found marker {:?} inside scan where {:?} was expected", marker, expected_marker))),
                                }
                            },
                            None => return Err(Error::Format(format!("{:?} marker not found where expected", expected_marker))),
                        }

                        huffman.reset();
                        // Section 2.1.3.1
                        dc_predictors = [0i32; MAX_COMPONENTS];
                        // Section G.1.2.2
                        eob_run = 0;

                        restarts_left = self.restart_interval;
                    }
                }
            }

            if !is_progressive {
                // Send the blocks from this MCU row to the worker thread for idct.
                let chan = worker_chan.as_ref().unwrap();

                for (i, blocks) in component_blocks.into_iter().enumerate() {
                    let row = RowData {
                        index: i,
                        component: components[i].clone(),
                        blocks: blocks,
                        quantization_table: self.quantization_tables[components[i].quantization_table_index].unwrap(),
                    };

                    try!(chan.send(WorkerMsg::AppendRow(row)));
                }
            }
        }

        if !is_progressive {
            // Retrieve all the data from the worker thread.
            let chan = worker_chan.as_ref().unwrap();

            for (i, &component_index) in scan.component_indices.iter().enumerate() {
                let (tx, rx) = channel();
                try!(chan.send(WorkerMsg::GetResult((i, tx))));

                self.component_data[component_index] = try!(rx.recv());
            }
        }

        Ok(huffman.take_marker())
    }

    fn decode_block(&mut self,
                    coefficients: &mut [i32; 64],
                    huffman: &mut HuffmanDecoder,
                    dc_table_index: usize,
                    ac_table_index: usize,
                    spectral_selection_start: u8,
                    spectral_selection_end: u8,
                    successive_approximation_low: u8,
                    eob_run: &mut u16,
                    dc_predictor: i32) -> Result<i32> {
        let mut dc_diff = 0;

        if spectral_selection_start == 0 {
            // Section F.2.2.1
            let dc_table = self.dc_huffman_tables[dc_table_index].as_ref().unwrap();
            let value = try!(huffman.decode(&mut self.reader, dc_table));
            let diff = match value {
                0 => 0,
                _ => try!(huffman.receive_extend(&mut self.reader, value)),
            };

            coefficients[0] = (dc_predictor + diff) << successive_approximation_low;
            dc_diff = diff;
        }

        let mut index = cmp::max(spectral_selection_start, 1);

        if index <= spectral_selection_end && *eob_run > 0 {
            *eob_run -= 1;
            return Ok(dc_diff);
        }

        let ac_table = self.ac_huffman_tables[ac_table_index].as_ref();

        // Section F.1.2.2.1
        while index <= spectral_selection_end {
            let byte = try!(huffman.decode(&mut self.reader, ac_table.unwrap()));
            let r = byte >> 4;
            let s = byte & 0x0f;

            if s == 0 {
                match r {
                    15 => index += 16, // Run length of 16 zero coefficients.
                    _  => {
                        *eob_run = (1 << r) - 1;

                        if r > 0 {
                            *eob_run += try!(huffman.receive(&mut self.reader, r)) as u16;
                        }

                        break;
                    },
                }
            }
            else {
                index += r;

                if index > spectral_selection_end {
                    break;
                }

                coefficients[UNZIGZAG[index as usize] as usize] = try!(huffman.receive_extend(&mut self.reader, s)) << successive_approximation_low;
                index += 1;
            }
        }

        Ok(dc_diff)
    }

    fn decode_block_successive_approximation(&mut self,
                                             coefficients: &mut [i32; 64],
                                             huffman: &mut HuffmanDecoder,
                                             ac_table_index: usize,
                                             spectral_selection_start: u8,
                                             spectral_selection_end: u8,
                                             successive_approximation_low: u8,
                                             eob_run: &mut u16) -> Result<()> {
        let bit = 1 << successive_approximation_low;

        if spectral_selection_start == 0 {
            // Section G.1.2.1

            if try!(huffman.receive(&mut self.reader, 1)) == 1 {
                coefficients[0] |= bit;
            }
        }
        else {
            // Section G.1.2.3

            if *eob_run > 0 {
                *eob_run -= 1;
                try!(self.refine_non_zeroes(coefficients, huffman, spectral_selection_start, spectral_selection_end, 64, bit));
                return Ok(());
            }

            let mut index = spectral_selection_start;

            while index <= spectral_selection_end {
                let byte = try!(huffman.decode(&mut self.reader, self.ac_huffman_tables[ac_table_index].as_ref().unwrap()));
                let r = byte >> 4;
                let s = byte & 0x0f;

                let mut zero_run_length = r;
                let mut value = 0;

                match s {
                    0 => {
                        match r {
                            15 => {
                                // Run length of 16 zero coefficients.
                                // We don't need to do anything special here, zero_run_length is 15
                                // and then value (which is zero) gets written, resulting in 16
                                // zero coefficients.
                            },
                            _ => {
                                *eob_run = (1 << r) - 1;

                                if r > 0 {
                                    *eob_run += try!(huffman.receive(&mut self.reader, r)) as u16;
                                }

                                // Force end of block.
                                zero_run_length = 64;
                            },
                        }
                    },
                    1 => {
                        if try!(huffman.receive(&mut self.reader, 1)) == 1 {
                            value = bit;
                        }
                        else {
                            value = -bit;
                        }
                    },
                    _ => return Err(Error::Format("unexpected huffman code".to_owned())),
                }

                index = try!(self.refine_non_zeroes(coefficients, huffman, index, spectral_selection_end, zero_run_length, bit));
                coefficients[UNZIGZAG[index as usize] as usize] = value;
                index += 1;
            }
        }

        Ok(())
    }

    fn refine_non_zeroes(&mut self, coefficients: &mut [i32; 64], huffman: &mut HuffmanDecoder, start: u8, end: u8, zrl: u8, bit: i32) -> Result<u8> {
        let mut zero_run_length = zrl;

        for i in start .. end + 1 {
            let index = UNZIGZAG[i as usize] as usize;

            if coefficients[index] == 0 {
                if zero_run_length == 0 {
                    return Ok(i);
                }

                zero_run_length -= 1;
            }
            else {
                if try!(huffman.receive(&mut self.reader, 1)) == 1 && coefficients[index] & bit == 0 {
                    if coefficients[index] > 0 {
                        coefficients[index] += bit;
                    }
                    else {
                        coefficients[index] -= bit;
                    }
                }
            }
        }

        Ok(end)
    }
}

fn compute_image(components: &[Component],
                 data: &[Vec<u8>],
                 output_size: Size2D<u16>,
                 is_jfif: bool,
                 color_transform: Option<AdobeColorTransform>) -> Result<Vec<u8>> {
    if data.iter().any(|data| data.is_empty()) {
        return Err(Error::Format("not all components has data".to_owned()));
    }

    if components.len() == 1 {
        let component = &components[0];

        if component.size.width % 8 == 0 && component.size.height % 8 == 0 {
            return Ok(data[0].clone())
        }

        let mut buffer = vec![0u8; component.size.width as usize * component.size.height as usize];
        let line_stride = component.block_size.width as usize * 8;

        for y in 0 .. component.size.height as usize {
            for x in 0 .. component.size.width as usize {
                buffer[y * component.size.width as usize + x] = data[0][y * line_stride + x];
            }
        }

        Ok(buffer)
    }
    else {
        let color_convert_func = try!(choose_color_convert_func(components.len(), is_jfif, color_transform));
        let resampler = Resampler::new(components).unwrap();
        let line_size = output_size.width as usize * components.len();
        let mut result = Vec::with_capacity(output_size.height as usize);

        (0 .. output_size.height as usize)
                .into_par_iter()
                .weight_max()
                .map(|row| {
                    let mut buffer = vec![0u8; line_size];
                    resampler.resample_and_interleave_row(data, row, output_size.width as usize, &mut buffer);
                    color_convert_func(&mut buffer, output_size.width as usize);
                    buffer
                })
                .collect_into(&mut result);

        let size = line_size * output_size.height as usize;
        Ok(result.into_iter().fold(Vec::with_capacity(size), |mut acc, mut line| { acc.append(&mut line); acc }))
    }
}

fn choose_color_convert_func(component_count: usize,
                             _is_jfif: bool,
                             color_transform: Option<AdobeColorTransform>)
                             -> Result<fn(&mut [u8], usize)> {
    match component_count {
        3 => {
            // http://www.sno.phy.queensu.ca/~phil/exiftool/TagNames/JPEG.html#Adobe
            // Unknown means the data is RGB, so we don't need to perform any color conversion on it.
            if color_transform == Some(AdobeColorTransform::Unknown) {
                Ok(color_convert_line_null)
            }
            else {
                Ok(color_convert_line_ycbcr)
            }
        },
        4 => {
            // http://www.sno.phy.queensu.ca/~phil/exiftool/TagNames/JPEG.html#Adobe
            match color_transform {
                Some(AdobeColorTransform::Unknown) => Ok(color_convert_line_cmyk),
                Some(_) => Ok(color_convert_line_ycck),
                None => Err(Error::Format("4 components without Adobe APP14 metadata to tell color space".to_owned())),
            }
        },
        _ => panic!(),
    }
}

fn color_convert_line_null(_data: &mut [u8], _width: usize) {
}

fn color_convert_line_ycbcr(data: &mut [u8], width: usize) {
    for i in 0 .. width {
        let (r, g, b) = ycbcr_to_rgb(data[i * 3], data[i * 3 + 1], data[i * 3 + 2]);

        data[i * 3]     = r;
        data[i * 3 + 1] = g;
        data[i * 3 + 2] = b;
    }
}

fn color_convert_line_ycck(data: &mut [u8], width: usize) {
    for i in 0 .. width {
        let (r, g, b) = ycbcr_to_rgb(data[i * 4], data[i * 4 + 1], data[i * 4 + 2]);
        let k = data[i * 4 + 3];

        data[i * 4]     = r;
        data[i * 4 + 1] = g;
        data[i * 4 + 2] = b;
        data[i * 4 + 3] = 255 - k;
    }
}

fn color_convert_line_cmyk(data: &mut [u8], width: usize) {
    for i in 0 .. width {
        data[i * 4]     = 255 - data[i * 4];
        data[i * 4 + 1] = 255 - data[i * 4 + 1];
        data[i * 4 + 2] = 255 - data[i * 4 + 2];
        data[i * 4 + 3] = 255 - data[i * 4 + 3];
    }
}

// ITU-R BT.601
fn ycbcr_to_rgb(y: u8, cb: u8, cr: u8) -> (u8, u8, u8) {
    let y = y as f32;
    let cb = cb as f32 - 128.0;
    let cr = cr as f32 - 128.0;

    let r = y                + 1.40200 * cr;
    let g = y - 0.34414 * cb - 0.71414 * cr;
    let b = y + 1.77200 * cb;

    (clamp((r + 0.5) as i32, 0, 255) as u8,
     clamp((g + 0.5) as i32, 0, 255) as u8,
     clamp((b + 0.5) as i32, 0, 255) as u8)
}

fn clamp<T: PartialOrd>(value: T, min: T, max: T) -> T {
    if value < min { return min; }
    if value > max { return max; }
    value
}
