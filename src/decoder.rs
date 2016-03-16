use byteorder::ReadBytesExt;
use color::{ColorSpace, ConvertColorSpace};
use error::{Error, Result, UnsupportedFeature};
use euclid::{Point2D, Rect, Size2D};
use huffman::{HuffmanDecoder, HuffmanTable};
use marker::Marker;
use parser::{AdobeColorTransform, AppData, CodingProcess, Component, EntropyCoding, FrameInfo,
             parse_app, parse_com, parse_dht, parse_dqt, parse_dri, parse_sof, parse_sos, ScanInfo};
use rayon::par_iter::*;
use resampler::Resampler;
use std::cmp;
use std::io::Read;
use std::mem;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use worker_thread::{RowData, spawn_worker_thread, WorkerMsg};

pub const MAX_COMPONENTS: usize = 4;

// Figure A.6
pub static ZIGZAG: [u8; 64] = [
     0,  1,  5,  6, 14, 15, 27, 28,
     2,  4,  7, 13, 16, 26, 29, 42,
     3,  8, 12, 17, 25, 30, 41, 43,
     9, 11, 18, 24, 31, 40, 44, 53,
    10, 19, 23, 32, 39, 45, 52, 54,
    20, 22, 33, 38, 46, 51, 55, 60,
    21, 34, 37, 47, 50, 56, 59, 61,
    35, 36, 48, 49, 57, 58, 62, 63,
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum DataType {
    Metadata,
    Coefficients,
    Pixels,
}

enum Image {
    /// Coefficients and quantization tables for each plane, stored in zigzag order.
    Coefficients(Vec<Vec<i16>>, Vec<[u8; 64]>),
    Pixels(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Metadata {
    /// Width of the image.
    pub width: u16,
    /// Height of the image.
    pub height: u16,

    /// Color space the decoded pixels will be in.
    /// It can be either `Grayscale`, `RGB` or `CMYK`.
    pub dst_color_space: ColorSpace,
    /// Color space the compressed image is stored in.
    pub src_color_space: ColorSpace,

    /// Width and height of each plane in number of 8x8 blocks.
    pub plane_size_blocks: Vec<(u16, u16)>,
    /// Horizontal and vertical sampling factor of each plane.
    pub plane_sampling_factor: Vec<(u8, u8)>,

    /// Maximum horizontal and vertical sampling factor.
    pub max_sampling_factor: (u8, u8),
}

pub struct Decoder<R> {
    reader: R,

    frame: Option<FrameInfo>,
    dc_huffman_tables: Vec<Option<HuffmanTable>>,
    ac_huffman_tables: Vec<Option<HuffmanTable>>,
    quantization_tables: [Option<Arc<[u8; 64]>>; 4],

    metadata: Option<Metadata>,
    restart_interval: u16,
    color_transform: Option<AdobeColorTransform>,
    is_jfif: bool,

    // Used for progressive JPEGs.
    coefficients: Vec<Vec<i16>>,
    // Used for detecting the last scan of components.
    coefficient_complete: Vec<[bool; 64]>,
}

impl<R: Read> Decoder<R> {
    pub fn new(reader: R) -> Decoder<R> {
        Decoder {
            reader: reader,
            frame: None,
            dc_huffman_tables: vec![None, None, None, None],
            ac_huffman_tables: vec![None, None, None, None],
            quantization_tables: [None, None, None, None],
            metadata: None,
            restart_interval: 0,
            color_transform: None,
            is_jfif: false,
            coefficients: Vec::new(),
            coefficient_complete: Vec::new(),
        }
    }

    pub fn metadata(&self) -> Option<Metadata> {
        self.metadata.clone()
    }

    pub fn read_metadata(&mut self) -> Result<()> {
        self.decode(DataType::Metadata).map(|_| ())
    }

    pub fn decode_pixels(&mut self) -> Result<Vec<u8>> {
        self.decode(DataType::Pixels).map(|data| {
            match data {
                Some(Image::Pixels(pixels)) => pixels,
                _ => panic!(),
            }
        })
    }

    /// Returns coefficients and quantization tables for each plane, stored in zigzag order.
    pub fn decode_coefficients(&mut self) -> Result<(Vec<Vec<i16>>, Vec<[u8; 64]>)> {
        self.decode(DataType::Coefficients).map(|data| {
            match data {
                Some(Image::Coefficients(coefficients, quantization_tables)) => (coefficients, quantization_tables),
                _ => panic!(),
            }
        })
    }

    fn decode(&mut self, data_type: DataType) -> Result<Option<Image>> {
        if data_type == DataType::Metadata && self.frame.is_some() {
            // The metadata has already been read.
            return Ok(None);
        }
        else if self.frame.is_none() && (try!(self.reader.read_u8()) != 0xFF || try!(self.reader.read_u8()) != Marker::SOI as u8) {
            return Err(Error::Format("first two bytes is not a SOI marker".to_owned()));
        }

        let mut previous_marker = Marker::SOI;
        let mut pending_marker = None;
        let mut worker_chan = None;
        let mut scans_processed = 0;
        let mut planes = vec![Vec::new(); self.frame.as_ref().map_or(0, |frame| frame.components.len())];

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
                    let component_count = frame.components.len();

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
                    if component_count != 1 && component_count != 3 && component_count != 4 {
                        return Err(Error::Unsupported(UnsupportedFeature::ComponentCount(component_count as u8)));
                    }
                    if Resampler::new(&frame.components).is_none() {
                        return Err(Error::Unsupported(UnsupportedFeature::SubsamplingRatio));
                    }

                    let width = frame.image_size.width;
                    let height = frame.image_size.height;
                    let h_max = frame.components.iter().map(|c| c.horizontal_sampling_factor).max().unwrap();
                    let v_max = frame.components.iter().map(|c| c.vertical_sampling_factor).max().unwrap();
                    let mut plane_size_blocks = Vec::with_capacity(frame.components.len());
                    let mut plane_sampling_factor = Vec::with_capacity(frame.components.len());

                    for component in &frame.components {
                        if frame.coding_process == CodingProcess::DctProgressive {
                            let block_count = component.block_size.width as usize * component.block_size.height as usize;
                            self.coefficients.push(vec![0i16; block_count * 64]);
                        }

                        self.coefficient_complete.push([false; 64]);

                        plane_size_blocks.push((component.block_size.width,
                                                component.block_size.height));
                        plane_sampling_factor.push((component.horizontal_sampling_factor,
                                                     component.vertical_sampling_factor));
                    }

                    self.frame = Some(frame);
                    self.metadata = Some(Metadata {
                        width: width,
                        height: height,
                        dst_color_space: try!(self.dst_color_space()),
                        src_color_space: try!(self.src_color_space()),
                        plane_size_blocks: plane_size_blocks,
                        plane_sampling_factor: plane_sampling_factor,
                        max_sampling_factor: (h_max, v_max),
                    });

                    if data_type == DataType::Metadata {
                        return Ok(None);
                    }

                    planes = vec![Vec::new(); component_count];
                },

                // Scan header
                Marker::SOS => {
                    if self.frame.is_none() {
                        return Err(Error::Format("scan encountered before frame".to_owned()));
                    }
                    if worker_chan.is_none() && data_type == DataType::Pixels {
                        worker_chan = Some(try!(spawn_worker_thread()));
                    }

                    let frame = self.frame.clone().unwrap();
                    let scan = try!(parse_sos(&mut self.reader, &frame));

                    for &i in scan.component_indices.iter() {
                        for j in scan.spectral_selection_start .. scan.spectral_selection_end + 1 {
                            self.coefficient_complete[i][j as usize] = scan.successive_approximation_low == 0;
                        }
                    }

                    let is_final_scan = scan.component_indices.iter()
                                                              .map(|&i| self.coefficient_complete[i].iter().all(|&v| v))
                                                              .all(|v| v);
                    let produce_data = if is_final_scan {
                        Some(data_type)
                    } else {
                        None
                    };

                    let (marker, samples) = try!(self.decode_scan(&frame, &scan, worker_chan.as_ref(), produce_data));

                    if let Some(samples) = samples {
                        for (i, plane_samples) in samples.into_iter()
                                                         .enumerate()
                                                         .filter(|&(_, ref plane_samples)| !plane_samples.is_empty()) {
                            planes[i] = plane_samples;
                        }
                    }

                    pending_marker = marker;
                    scans_processed += 1;
                },

                // Table-specification and miscellaneous markers
                // Quantization table-specification
                Marker::DQT => {
                    let tables = try!(parse_dqt(&mut self.reader));

                    for (i, &table) in tables.into_iter().enumerate() {
                        if let Some(table) = table {
                            self.quantization_tables[i] = Some(Arc::new(table));
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

        match self.frame {
            Some(ref frame) => {
                match data_type {
                    DataType::Coefficients => {
                        let coefficients = mem::replace(&mut self.coefficients, Vec::new());
                        let mut quantization_tables = Vec::with_capacity(frame.components.len());

                        for component in &frame.components {
                            let quantization_table = &self.quantization_tables[component.quantization_table_index];
                            quantization_tables.push(*quantization_table.clone().unwrap());
                        }

                        Ok(Some(Image::Coefficients(coefficients, quantization_tables)))
                    },
                    DataType::Pixels => {
                        if planes.iter().all(|plane| !plane.is_empty()) {
                            let image = try!(compute_image(try!(self.src_color_space()),
                                                           try!(self.dst_color_space()),
                                                           frame.image_size,
                                                           &frame.components,
                                                           &planes));
                            Ok(Some(Image::Pixels(image)))
                        }
                        else {
                            Err(Error::Format("not all components has data".to_owned()))
                        }
                    },
                    DataType::Metadata => panic!(),
                }
            },
            None => Err(Error::Format("no frame found".to_owned())),
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

    fn decode_scan(&mut self,
                   frame: &FrameInfo,
                   scan: &ScanInfo,
                   worker_chan: Option<&Sender<WorkerMsg>>,
                   produce_data: Option<DataType>)
                   -> Result<(Option<Marker>, Option<Vec<Vec<u8>>>)> {
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

        let is_progressive = frame.coding_process == CodingProcess::DctProgressive;
        let mut mcu_row_coefficients = Vec::with_capacity(components.len());

        if produce_data == Some(DataType::Pixels) {
            // Prepare the worker thread for the work to come.
            for (i, component) in components.iter().enumerate() {
                let row_data = RowData {
                    index: i,
                    component: component.clone(),
                    quantization_table: self.quantization_tables[component.quantization_table_index].clone().unwrap(),
                };

                try!(worker_chan.unwrap().send(WorkerMsg::Start(row_data)));
            }

            if !is_progressive {
                for component in &components {
                    let coefficients_per_mcu_row = component.block_size.width as usize * component.vertical_sampling_factor as usize * 64;
                    mcu_row_coefficients.push(vec![0i16; coefficients_per_mcu_row]);
                }
            }
        }

        if !is_progressive && produce_data == Some(DataType::Coefficients) && self.coefficients.is_empty() {
            for component in &frame.components {
                let block_count = component.block_size.width as usize * component.block_size.height as usize;
                self.coefficients.push(vec![0i16; block_count * 64]);
            }
        }

        let blocks_per_mcu: Vec<u16> = components.iter()
                                                 .map(|c| c.horizontal_sampling_factor as u16 * c.vertical_sampling_factor as u16)
                                                 .collect();
        let is_interleaved = components.len() > 1;
        let mut dummy_block = [0i16; 64];
        let mut huffman = HuffmanDecoder::new();
        let mut dc_predictors = [0i16; MAX_COMPONENTS];
        let mut restarts_left = self.restart_interval;
        let mut expected_rst_num = 0;
        let mut eob_run = 0;

        for mcu_y in 0 .. frame.mcu_size.height {
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

                        let block_offset = (block_coords.y as usize * component.block_size.width as usize + block_coords.x as usize) * 64;
                        let mcu_row_offset = mcu_y as usize * component.block_size.width as usize * component.vertical_sampling_factor as usize * 64;
                        let coefficients = if is_progressive || produce_data == Some(DataType::Coefficients) {
                            &mut self.coefficients[scan.component_indices[i]][block_offset .. block_offset + 64]
                        } else if produce_data == Some(DataType::Pixels) {
                            &mut mcu_row_coefficients[i][block_offset - mcu_row_offset .. block_offset - mcu_row_offset + 64]
                        } else {
                            &mut dummy_block[..]
                        };

                        if scan.successive_approximation_high == 0 {
                            let dc_diff = try!(decode_block(&mut self.reader,
                                                            coefficients,
                                                            &mut huffman,
                                                            self.dc_huffman_tables[scan.dc_table_indices[i]].as_ref(),
                                                            self.ac_huffman_tables[scan.ac_table_indices[i]].as_ref(),
                                                            scan.spectral_selection_start,
                                                            scan.spectral_selection_end,
                                                            scan.successive_approximation_low,
                                                            &mut eob_run,
                                                            dc_predictors[i]));
                            dc_predictors[i] += dc_diff;
                        }
                        else {
                            try!(decode_block_successive_approximation(&mut self.reader,
                                                                       coefficients,
                                                                       &mut huffman,
                                                                       self.ac_huffman_tables[scan.ac_table_indices[i]].as_ref(),
                                                                       scan.spectral_selection_start,
                                                                       scan.spectral_selection_end,
                                                                       scan.successive_approximation_low,
                                                                       &mut eob_run));
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
                        // Section F.2.1.3.1
                        dc_predictors = [0i16; MAX_COMPONENTS];
                        // Section G.1.2.2
                        eob_run = 0;

                        restarts_left = self.restart_interval;
                    }
                }
            }

            if produce_data == Some(DataType::Pixels) {
                // Send the coefficients from this MCU row to the worker thread for dequantization and idct.
                for (i, component) in components.iter().enumerate() {
                    let coefficients_per_mcu_row = component.block_size.width as usize * component.vertical_sampling_factor as usize * 64;

                    let row_coefficients = if is_progressive {
                        let offset = mcu_y as usize * coefficients_per_mcu_row;
                        self.coefficients[scan.component_indices[i]][offset .. offset + coefficients_per_mcu_row].to_vec()
                    } else {
                        mem::replace(&mut mcu_row_coefficients[i], vec![0i16; coefficients_per_mcu_row])
                    };

                    try!(worker_chan.unwrap().send(WorkerMsg::AppendRow((i, row_coefficients))));
                }
            }
        }

        if produce_data == Some(DataType::Pixels) {
            // Retrieve all the data from the worker thread.
            let mut data = vec![Vec::new(); frame.components.len()];

            for (i, &component_index) in scan.component_indices.iter().enumerate() {
                let (tx, rx) = mpsc::channel();
                try!(worker_chan.unwrap().send(WorkerMsg::GetResult((i, tx))));

                data[component_index] = try!(rx.recv());
            }

            Ok((huffman.take_marker(), Some(data)))
        }
        else {
            Ok((huffman.take_marker(), None))
        }
    }

    fn src_color_space(&self) -> Result<ColorSpace> {
        let frame = self.frame.as_ref().unwrap();

        match frame.components.len() {
            1 => Ok(ColorSpace::Grayscale),
            3 => {
                // http://www.sno.phy.queensu.ca/~phil/exiftool/TagNames/JPEG.html#Adobe
                match self.color_transform {
                    Some(AdobeColorTransform::Unknown) => Ok(ColorSpace::RGB),
                    _ => Ok(ColorSpace::YCbCr),
                }
            },
            4 => {
                // http://www.sno.phy.queensu.ca/~phil/exiftool/TagNames/JPEG.html#Adobe
                match self.color_transform {
                    Some(AdobeColorTransform::Unknown) => Ok(ColorSpace::CMYK),
                    Some(_) => Ok(ColorSpace::YCCK),
                    None => Err(Error::Format("4 components without Adobe APP14 metadata to tell color space".to_owned())),
                }
            },
            _ => panic!(),
        }
    }

    fn dst_color_space(&self) -> Result<ColorSpace> {
        match try!(self.src_color_space()) {
            ColorSpace::Grayscale => Ok(ColorSpace::Grayscale),
            ColorSpace::RGB | ColorSpace::YCbCr => Ok(ColorSpace::RGB),
            ColorSpace::CMYK | ColorSpace::YCCK => Ok(ColorSpace::CMYK),
        }
    }
}

fn decode_block<R: Read>(reader: &mut R,
                         coefficients: &mut [i16],
                         huffman: &mut HuffmanDecoder,
                         dc_table: Option<&HuffmanTable>,
                         ac_table: Option<&HuffmanTable>,
                         spectral_selection_start: u8,
                         spectral_selection_end: u8,
                         successive_approximation_low: u8,
                         eob_run: &mut u16,
                         dc_predictor: i16) -> Result<i16> {
    debug_assert_eq!(coefficients.len(), 64);

    let mut dc_diff = 0;

    if spectral_selection_start == 0 {
        // Section F.2.2.1
        // Figure F.12
        let value = try!(huffman.decode(reader, dc_table.unwrap()));
        let diff = match value {
            0 => 0,
            _ => {
                // Section F.1.2.1.1
                // Table F.1
                if value > 11 {
                    return Err(Error::Format("invalid DC difference magnitude category".to_owned()));
                }

                try!(huffman.receive_extend(reader, value))
            },
        };

        coefficients[0] = (dc_predictor + diff) << successive_approximation_low;
        dc_diff = diff;
    }

    let mut index = cmp::max(spectral_selection_start, 1);

    if index <= spectral_selection_end && *eob_run > 0 {
        *eob_run -= 1;
        return Ok(dc_diff);
    }

    // Section F.1.2.2.1
    while index <= spectral_selection_end {
        if let Some((value, run)) = try!(huffman.decode_fast_ac(reader, ac_table.unwrap())) {
            index += run;

            if index > spectral_selection_end {
                break;
            }

            coefficients[index as usize] = value << successive_approximation_low;
            index += 1;
        }
        else {
            let byte = try!(huffman.decode(reader, ac_table.unwrap()));
            let r = byte >> 4;
            let s = byte & 0x0f;

            if s == 0 {
                match r {
                    15 => index += 16, // Run length of 16 zero coefficients.
                    _  => {
                        *eob_run = (1 << r) - 1;

                        if r > 0 {
                            *eob_run += try!(huffman.get_bits(reader, r));
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

                coefficients[index as usize] = try!(huffman.receive_extend(reader, s)) << successive_approximation_low;
                index += 1;
            }
        }
    }

    Ok(dc_diff)
}

fn decode_block_successive_approximation<R: Read>(reader: &mut R,
                                                  coefficients: &mut [i16],
                                                  huffman: &mut HuffmanDecoder,
                                                  ac_table: Option<&HuffmanTable>,
                                                  spectral_selection_start: u8,
                                                  spectral_selection_end: u8,
                                                  successive_approximation_low: u8,
                                                  eob_run: &mut u16) -> Result<()> {
    debug_assert_eq!(coefficients.len(), 64);

    let bit = 1 << successive_approximation_low;

    if spectral_selection_start == 0 {
        // Section G.1.2.1

        if try!(huffman.get_bits(reader, 1)) == 1 {
            coefficients[0] |= bit;
        }
    }
    else {
        // Section G.1.2.3

        if *eob_run > 0 {
            *eob_run -= 1;
            try!(refine_non_zeroes(reader, coefficients, huffman, spectral_selection_start, spectral_selection_end, 64, bit));
            return Ok(());
        }

        let mut index = spectral_selection_start;

        while index <= spectral_selection_end {
            let byte = try!(huffman.decode(reader, ac_table.unwrap()));
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
                                *eob_run += try!(huffman.get_bits(reader, r));
                            }

                            // Force end of block.
                            zero_run_length = 64;
                        },
                    }
                },
                1 => {
                    if try!(huffman.get_bits(reader, 1)) == 1 {
                        value = bit;
                    }
                    else {
                        value = -bit;
                    }
                },
                _ => return Err(Error::Format("unexpected huffman code".to_owned())),
            }

            index = try!(refine_non_zeroes(reader, coefficients, huffman, index, spectral_selection_end, zero_run_length, bit));

            if value != 0 {
                coefficients[index as usize] = value;
            }

            index += 1;
        }
    }

    Ok(())
}

fn refine_non_zeroes<R: Read>(reader: &mut R,
                              coefficients: &mut [i16],
                              huffman: &mut HuffmanDecoder,
                              start: u8,
                              end: u8,
                              zrl: u8,
                              bit: i16) -> Result<u8> {
    debug_assert_eq!(coefficients.len(), 64);

    let mut zero_run_length = zrl;

    for i in start as usize .. (end + 1) as usize {
        if coefficients[i] == 0 {
            if zero_run_length == 0 {
                return Ok(i as u8);
            }

            zero_run_length -= 1;
        }
        else {
            if try!(huffman.get_bits(reader, 1)) == 1 && coefficients[i] & bit == 0 {
                if coefficients[i] > 0 {
                    coefficients[i] += bit;
                }
                else {
                    coefficients[i] -= bit;
                }
            }
        }
    }

    Ok(end)
}

fn compute_image(src_color_space: ColorSpace,
                 dst_color_space: ColorSpace,
                 output_size: Size2D<u16>,
                 components: &[Component],
                 data: &[Vec<u8>]) -> Result<Vec<u8>> {
    assert_eq!(data.len(), src_color_space.num_components());
    assert!(data.iter().all(|data| !data.is_empty()));

    if components.len() == 1 {
        let component = &components[0];

        if component.size.width % 8 == 0 && component.size.height % 8 == 0 {
            Ok(data[0].clone())
        }
        else {
            let mut buffer = vec![0u8; component.size.width as usize * component.size.height as usize];
            let line_stride = component.block_size.width as usize * 8;

            for y in 0 .. component.size.height as usize {
                for x in 0 .. component.size.width as usize {
                    buffer[y * component.size.width as usize + x] = data[0][y * line_stride + x];
                }
            }

            Ok(buffer)
        }
    }
    else {
        let resampler = Resampler::new(components).unwrap();
        let line_size = output_size.width as usize * components.len();
        let mut image = vec![0u8; line_size * output_size.height as usize];

        image.chunks_mut(line_size)
             .collect::<Vec<&mut [u8]>>()
             .par_iter_mut()
             .weight_max()
             .enumerate()
             .for_each(|(row, line)| {
                 resampler.resample_and_interleave_row(data, row, output_size.width as usize, *line);
                 src_color_space.convert(&dst_color_space, *line, output_size.width as usize);
             });

        Ok(image)
    }
}
