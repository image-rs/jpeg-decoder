use std::cmp;
use std::io::Read;
use std::ops::RangeInclusive;
use std::sync::Arc;

use byteorder::ReadBytesExt;

use error::{Error, Result, UnsupportedFeature};
use huffman::{
    ComponentVec, DhtTables, empty_component_vec, fill_default_mjpeg_tables,
    HuffmanDecoder, HuffmanTable,
};
use huffman::HuffmanTableClass::{AC, DC};
use marker::Marker;
use parser::{
    AdobeColorTransform, AppData, CodingProcess, Component,
    Dimensions, EntropyCoding, FrameInfo,
    parse_app, parse_com, parse_dht, parse_dqt, parse_dri, parse_sof, parse_sos, ScanInfo,
};
use upsampler::Upsampler;
use worker::{PlatformWorker, RowData, Worker};

pub const MAX_COMPONENTS: usize = 4;

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

/// An enumeration over combinations of color spaces and bit depths a pixel can have.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PixelFormat {
    /// Luminance (grayscale), 8 bits
    L8,
    /// RGB, 8 bits per channel
    RGB24,
    /// CMYK, 8 bits per channel
    CMYK32,
}

/// Represents metadata of an image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageInfo {
    /// The width of the image, in pixels.
    pub width: u16,
    /// The height of the image, in pixels.
    pub height: u16,
    /// The pixel format of the image.
    pub pixel_format: PixelFormat,
}

/// JPEG decoder
pub struct Decoder<R> {
    reader: R,

    frame: Option<FrameInfo>,
    huffman_tables: DhtTables,
    quantization_tables: ComponentVec<Arc<[u16; 64]>>,
    restart_interval: u16,
    color_transform: Option<AdobeColorTransform>,
    is_jfif: bool,
    is_mjpeg: bool,

    // Used for progressive JPEGs.
    coefficients: Vec<Vec<i16>>,
    // Bitmask of which coefficients has been completely decoded.
    coefficients_finished: [u64; MAX_COMPONENTS],
}

impl<R: Read> Decoder<R> {
    /// Creates a new `Decoder` using the reader `reader`.
    pub fn new(reader: R) -> Decoder<R> {
        Decoder {
            reader,
            frame: None,
            huffman_tables: DhtTables::new(),
            quantization_tables: empty_component_vec(),
            restart_interval: 0,
            color_transform: None,
            is_jfif: false,
            is_mjpeg: false,
            coefficients: Vec::new(),
            coefficients_finished: [0; MAX_COMPONENTS],
        }
    }

    /// Returns metadata about the image.
    ///
    /// The returned value will be `None` until a call to either `read_info` or `decode` has
    /// returned `Ok`.
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
                    pixel_format,
                })
            },
            None => None,
        }
    }

    /// Tries to read metadata from the image without decoding it.
    ///
    /// If successful, the metadata can be obtained using the `info` method.
    pub fn read_info(&mut self) -> Result<()> {
        self.decode_internal(true).map(|_| ())
    }

    /// Decodes the image and returns the decoded pixels if successful.
    pub fn decode(&mut self) -> Result<Vec<u8>> {
        self.decode_internal(false)
    }

    fn decode_internal(&mut self, stop_after_metadata: bool) -> Result<Vec<u8>> {
        if stop_after_metadata && self.frame.is_some() {
            // The metadata has already been read.
            return Ok(Vec::new());
        }
        else if self.frame.is_none() && (self.reader.read_u8()? != 0xFF || Marker::from_u8(self.reader.read_u8()?) != Some(Marker::SOI)) {
            return Err(Error::Format("first two bytes is not a SOI marker".to_owned()));
        }

        let mut previous_marker = Marker::SOI;
        let mut pending_marker = None;
        let mut worker = None;
        let mut scans_processed = 0;
        let mut planes = vec![Vec::new(); self.frame.as_ref().map_or(0, |frame| frame.components.len())];

        loop {
            let marker = match pending_marker.take() {
                Some(m) => m,
                None => self.read_marker()?,
            };

            match marker {
                // Frame header
                Marker::SOF(..) => {
                    // Section 4.10
                    // "An image contains only one frame in the cases of sequential and
                    //  progressive coding processes; an image contains multiple frames for the
                    //  hierarchical mode."
                    if self.frame.is_some() {
                        return Err(Error::Unsupported(UnsupportedFeature::Hierarchical));
                    }

                    let frame = parse_sof(&mut self.reader, marker)?;
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

                    // Make sure we support the subsampling ratios used.
                    let _ = Upsampler::new(&frame.components, frame.image_size.width, frame.image_size.height)?;

                    self.frame = Some(frame);

                    if stop_after_metadata {
                        return Ok(Vec::new());
                    }

                    planes = vec![Vec::new(); component_count];
                },

                // Scan header
                Marker::SOS => {
                    if self.frame.is_none() {
                        return Err(Error::Format("scan encountered before frame".to_owned()));
                    }
                    if worker.is_none() {
                        worker = Some(PlatformWorker::new()?);
                    }

                    let frame = self.frame.clone().unwrap();
                    let scan = parse_sos(&mut self.reader, &frame)?;

                    if self.coefficients.is_empty() {
                        self.coefficients = frame.components.iter().map(|c| {
                            let block_count = c.block_size.width as usize * c.block_size.height as usize;
                            vec![0; block_count * 64]
                        }).collect();
                    }

                    if scan.successive_approximation_low == 0 {
                        for &i in scan.component_indices.iter() {
                            for j in scan.spectral_selection.clone() {
                                self.coefficients_finished[i] |= 1 << j;
                            }
                        }
                    }

                    let is_final_scan = scan.component_indices.iter().all(|&i| self.coefficients_finished[i] == !0);
                    let ScanData { marker, data } = self.decode_scan(&frame, &scan, worker.as_mut().unwrap(), is_final_scan)?;

                    if let Some(data) = data {
                        for (i, plane) in data.into_iter().enumerate().filter(|&(_, ref plane)| !plane.is_empty()) {
                            planes[i] = plane;
                        }
                    }

                    pending_marker = marker;
                    scans_processed += 1;
                },

                // Table-specification and miscellaneous markers
                // Quantization table-specification
                Marker::DQT => {
                    let tables = parse_dqt(&mut self.reader)?;

                    for (i, &table) in tables.iter().enumerate() {
                        if let Some(table) = table {
                            let mut unzigzagged_table = [0u16; 64];

                            for j in 0 .. 64 {
                                unzigzagged_table[UNZIGZAG[j] as usize] = table[j];
                            }

                            self.quantization_tables[i] = Some(Arc::new(unzigzagged_table));
                        }
                    }
                },
                // Huffman table-specification
                Marker::DHT => {
                    let is_baseline = self.frame.as_ref().map(|frame| frame.is_baseline);
                    self.huffman_tables.update(parse_dht(&mut self.reader, is_baseline)?);
                },
                // Arithmetic conditioning table-specification
                Marker::DAC => return Err(Error::Unsupported(UnsupportedFeature::ArithmeticEntropyCoding)),
                // Restart interval definition
                Marker::DRI => self.restart_interval = parse_dri(&mut self.reader)?,
                // Comment
                Marker::COM => {
                    let _comment = parse_com(&mut self.reader)?;
                },
                // Application data
                Marker::APP(..) => {
                    if let Some(data) = parse_app(&mut self.reader, marker)? {
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
                            AppData::Avi1 => self.is_mjpeg = true,
                        }
                    }
                },
                // Restart
                Marker::RST(..) => {
                    // Some encoders emit a final RST marker after entropy-coded data, which
                    // decode_scan does not take care of. So if we encounter one, we ignore it.
                    if previous_marker != Marker::SOS {
                        return Err(Error::Format("RST found outside of entropy-coded data".to_owned()));
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

        if planes.is_empty() || planes.iter().any(|plane| plane.is_empty()) {
            return Err(Error::Format("no data found".to_owned()));
        }

        let frame = self.frame.as_ref().unwrap();
        compute_image(&frame.components, &planes, frame.image_size, self.is_jfif, self.color_transform)
    }

    fn read_marker(&mut self) -> Result<Marker> {
        // This should be an error as the JPEG spec doesn't allow extraneous data between marker segments.
        // libjpeg allows this though and there are images in the wild utilising it, so we are
        // forced to support this behavior.
        // Sony Ericsson P990i is an example of a device which produce this sort of JPEGs.
        while self.reader.read_u8()? != 0xFF {}

        let mut byte = self.reader.read_u8()?;

        // Section B.1.1.2
        // "Any marker may optionally be preceded by any number of fill bytes, which are bytes assigned code X’FF’."
        while byte == 0xFF {
            byte = self.reader.read_u8()?;
        }

        match byte {
            0x00 => Err(Error::Format("FF 00 found where marker was expected".to_owned())),
            _    => Ok(Marker::from_u8(byte).unwrap()),
        }
    }

    fn decode_scan(&mut self,
                   frame: &FrameInfo,
                   scan: &ScanInfo,
                   worker: &mut PlatformWorker,
                   produce_data: bool)
                   -> Result<ScanData> {
        assert!(scan.component_indices.len() <= MAX_COMPONENTS);

        let components: Vec<Component> = scan.component_indices.iter()
                                                               .map(|&i| frame.components[i].clone())
                                                               .collect();

        // Verify that all required quantization tables has been set.
        if components.iter().any(|component| self.quantization_tables[component.quantization_table_index].is_none()) {
            return Err(Error::Format("use of unset quantization table".to_owned()));
        }

        if self.is_mjpeg {
            fill_default_mjpeg_tables(scan, &mut self.huffman_tables);
        }

        self.check_huffman_tables_for_scan(scan)?;

        let is_interleaved = components.len() > 1;

        let mut decode_state = DecodeScanState::new(produce_data, self.restart_interval);

        for mcu_y in 0 .. frame.mcu_size.height {
            for mcu_x in 0 .. frame.mcu_size.width {
                for (i, component) in components.iter().enumerate() {
                    let coefficient_line = &mut self.coefficients[scan.component_indices[i]];
                    decode_state.new_component(i, component, worker, &self.quantization_tables)?;
                    for j in 0 .. component.blocks_per_mcu() {
                        let (block_x, block_y) = block_position(frame, is_interleaved, (mcu_x, mcu_y), component, j);
                        if !is_interleaved && (block_x * 8 >= component.size.width || block_y * 8 >= component.size.height) {
                            continue;
                        }

                        let block_offset = (block_y as usize * component.block_size.width as usize + block_x as usize) * 64;
                        let mut coefficients = &mut coefficient_line[block_offset..block_offset + 64];

                        if scan.successive_approximation_high == 0 {
                            decode_block(&mut self.reader,
                                         &mut coefficients,
                                         &mut decode_state,
                                         &self.huffman_tables,
                                         scan)?;
                        }
                        else {
                            decode_block_successive_approximation(&mut self.reader,
                                                                  &mut coefficients,
                                                                  &mut decode_state,
                                                                  self.huffman_tables[AC][scan.ac_table_indices[i]].as_ref(),
                                                                  scan)?;
                        }
                    }
                }

                let is_last_mcu = mcu_x == frame.mcu_size.width - 1 && mcu_y == frame.mcu_size.height - 1;
                decode_state.advance_mcu(&mut self.reader, is_last_mcu)?;
            }

            if produce_data {
                for (i, component) in components.iter().enumerate() {
                    // Send the coefficients from this MCU row to the worker thread for dequantization and idct.
                    let coefficients_per_mcu_row = component.block_size.width as usize * component.vertical_sampling_factor as usize * 64;
                    let offset = mcu_y as usize * coefficients_per_mcu_row;
                    let coefficient_line = &mut self.coefficients[scan.component_indices[i]];
                    let row_coefficients = coefficient_line[offset..offset + coefficients_per_mcu_row].to_vec();
                    worker.append_row((i, row_coefficients))?;
                }
            }
        }

        let marker = decode_state.huffman.take_marker(&mut self.reader)?;

        let data = if produce_data {
            // Retrieve all the data from the worker thread.
            let mut data = vec![Vec::new(); frame.components.len()];

            for (i, &component_index) in scan.component_indices.iter().enumerate() {
                data[component_index] = worker.get_result(i)?;
            }

            Some(data)
        } else { None };
        Ok(ScanData { marker, data })
    }

    /// Verify that all required huffman tables have been set.
    #[inline]
    fn check_huffman_tables_for_scan(&mut self, scan: &ScanInfo) -> Result<()> {
        if *scan.spectral_selection.start() == 0 &&
            scan.dc_table_indices.iter().any(|&i| self.huffman_tables[DC][i].is_none()) {
            Err(Error::Format("scan makes use of unset dc huffman table".to_owned()))
        } else if *scan.spectral_selection.end() > 0 &&
            scan.ac_table_indices.iter().any(|&i| self.huffman_tables[AC][i].is_none()) {
            Err(Error::Format("scan makes use of unset ac huffman table".to_owned()))
        } else {
            Ok(())
        }
    }
}

#[inline]
fn block_position(
    frame: &FrameInfo,
    is_interleaved: bool,
    (mcu_x, mcu_y): (u16, u16),
    component: &Component,
    block_index: u16) -> (u16, u16) {
    let sampling_h = u16::from(component.horizontal_sampling_factor);
    let sampling_v = u16::from(component.vertical_sampling_factor);

    if is_interleaved {
        // Section A.2.3
        (mcu_x * sampling_h + block_index % sampling_h,
         mcu_y * sampling_v + block_index / sampling_h)
    } else {
        // Section A.2.2
        let blocks_per_row = component.block_size.width as usize;
        let block_num = block_index as usize +
            (mcu_y as usize * frame.mcu_size.width as usize + mcu_x as usize) *
                (sampling_h * sampling_v) as usize;

        let x = (block_num % blocks_per_row) as u16;
        let y = (block_num / blocks_per_row) as u16;

        (x, y)
    }
}

fn decode_block<R: Read>(reader: &mut R,
                         coefficients: &mut [i16],
                         decode_state: &mut DecodeScanState,
                         huffman_tables: &DhtTables,
                         scan: &ScanInfo) -> Result<()> {
    debug_assert_eq!(coefficients.len(), 64);

    let idx = decode_state.component_idx;

    if *scan.spectral_selection.start() == 0 {
        // Section F.2.2.1
        // Figure F.12
        let dc_table = huffman_tables[DC][scan.dc_table_indices[idx]].as_ref().unwrap();
        let value = decode_state.huffman.decode(reader, dc_table)?;
        let diff = match value {
            0 => 0,
            value if value > 11 => {
                // Section F.1.2.1.1
                // Table F.1
                return Err(Error::Format("invalid DC difference magnitude category".to_owned()));
            }
            count => decode_state.huffman.receive_extend(reader, count)?,
        };

        let dc_predictor = &mut decode_state.dc_predictors[idx];
        // Malicious JPEG files can cause this add to overflow, therefore we use wrapping_add.
        // One example of such a file is tests/crashtest/images/dc-predictor-overflow.jpg
        *dc_predictor = dc_predictor.wrapping_add(diff);
        coefficients[0] = *dc_predictor << scan.successive_approximation_low;
    }

    let mut index = cmp::max(*scan.spectral_selection.start(), 1);

    if index <= *scan.spectral_selection.end() && decode_state.eob_run > 0 {
        decode_state.eob_run -= 1;
        return Ok(());
    }

    let ac_table = huffman_tables[AC][scan.ac_table_indices[idx]].as_ref();

    // Section F.1.2.2.1
    while index <= *scan.spectral_selection.end() {
        if let Some((value, run)) = decode_state.huffman.decode_fast_ac(reader, ac_table.unwrap())? {
            index += run;

            if index > *scan.spectral_selection.end() {
                break;
            }

            coefficients[UNZIGZAG[index as usize] as usize] = value << scan.successive_approximation_low;
            index += 1;
        }
        else {
            let byte = decode_state.huffman.decode(reader, ac_table.unwrap())?;
            match (byte >> 4, byte & 0x0f) { // Match on higher and lower 4 bits
                (15, 0) => index += 16, // Run length of 16 zero coefficients.
                (r, 0) => {
                    decode_state.advance_eob(reader, r)?;
                    break;
                },
                (r, s) => {
                    index += r;

                    if index > *scan.spectral_selection.end() {
                        break;
                    }

                    coefficients[UNZIGZAG[index as usize] as usize] =
                        decode_state.huffman.receive_extend(reader, s)? << scan.successive_approximation_low;
                    index += 1;
                }
            }
        }
    }

    Ok(())
}

struct ScanData {
    marker: Option<Marker>,
    data: Option<Vec<Vec<u8>>>,
}

fn decode_block_successive_approximation<R: Read>(reader: &mut R,
                                                  coefficients: &mut [i16],
                                                  decode_state: &mut DecodeScanState,
                                                  ac_table: Option<&HuffmanTable>,
                                                  scan: &ScanInfo) -> Result<()> {
    debug_assert_eq!(coefficients.len(), 64);
    let bit = 1 << scan.successive_approximation_low;

    if *scan.spectral_selection.start() == 0 {
        // Section G.1.2.1

        if decode_state.huffman.get_bits(reader, 1)? == 1 {
            coefficients[0] |= bit;
        }
    }
    else {
        // Section G.1.2.3

        if decode_state.eob_run > 0 {
            decode_state.eob_run -= 1;
            refine_non_zeroes(reader, coefficients, &mut decode_state.huffman, scan.spectral_selection.clone(), 64, bit)?;
            return Ok(());
        }

        let mut index = *scan.spectral_selection.start();

        while index <= *scan.spectral_selection.end() {
            let byte = decode_state.huffman.decode(reader, ac_table.unwrap())?;
            let (zero_run_length, value) = match (byte >> 4, byte & 0x0f) {
                (15, 0) => {
                    // Run length of 16 zero coefficients.
                    // We don't need to do anything special here, zero_run_length is 15
                    // and then value (which is zero) gets written, resulting in 16
                    // zero coefficients.
                    (15, 0)
                },
                (r, 0) => {
                    decode_state.advance_eob(reader, r)?;
                    (64, 0) // Force end of block.
                },
                (r, 1) => {
                    let opposite = decode_state.huffman.get_bits(reader, 1)? != 1;
                    (r, if opposite { -bit } else { bit })
                },
                _ => return Err(Error::Format("unexpected huffman code".to_owned())),
            };

            let range = index ..= *scan.spectral_selection.end();

            index = refine_non_zeroes(reader, coefficients, &mut decode_state.huffman, range, zero_run_length, bit)?;

            if value != 0 {
                coefficients[UNZIGZAG[index as usize] as usize] = value;
            }

            index += 1;
        }
    }

    Ok(())
}

struct DecodeScanState {
    huffman: HuffmanDecoder,
    dc_predictors: [i16; MAX_COMPONENTS],
    mcus_left_until_restart: u16,
    restart_interval: u16,
    expected_rst_num: u8,
    eob_run: u16,
    last_worker_idx: usize,
    produce_data: bool,
    component_idx: usize,
}

impl DecodeScanState {
    fn new(produce_data: bool, restart_interval: u16) -> DecodeScanState {
        DecodeScanState {
            huffman: HuffmanDecoder::new(),
            dc_predictors: [0i16; MAX_COMPONENTS],
            mcus_left_until_restart: restart_interval,
            restart_interval,
            expected_rst_num: 0,
            eob_run: 0,
            last_worker_idx: 0,
            produce_data,
            component_idx: 0
        }
    }

    fn new_component(&mut self,
                     i: usize,
                     component: &Component,
                     worker: &mut PlatformWorker,
                     quantization_tables: &ComponentVec<Arc<[u16; 64]>>,
    ) -> Result<()> {
        self.component_idx = i;
        // seeing this component for the 1st time:
        if i >= self.last_worker_idx && self.produce_data {
            self.last_worker_idx += 1;
            worker.start(RowData {
                index: i,
                component: component.clone(),
                quantization_table: quantization_tables[component.quantization_table_index].clone().unwrap(),
            })?;
        }
        Ok(())
    }

    fn advance_mcu<R: Read>(&mut self, reader: &mut R, is_last_mcu: bool) -> Result<()> {
        if self.restart_interval == 0 { return Ok(()) }

        self.mcus_left_until_restart -= 1;

        if self.mcus_left_until_restart == 0 && !is_last_mcu {
            match self.huffman.take_marker(reader)? {
                Some(Marker::RST(n)) => {
                    if n != self.expected_rst_num {
                        return Err(Error::Format(format!("found RST{} where RST{} was expected", n, self.expected_rst_num)));
                    }

                    self.reset();
                },
                Some(marker) => return Err(Error::Format(format!("found marker {:?} inside scan where RST{} was expected", marker, self.expected_rst_num))),
                None => return Err(Error::Format(format!("no marker found where RST{} was expected", self.expected_rst_num))),
            }
        }
        Ok(())
    }

    fn advance_eob<R: Read>(&mut self, reader: &mut R, r: u8) -> Result<()> {
        self.eob_run = (1 << r) - 1;
        if r > 0 {
            self.eob_run += self.huffman.get_bits(reader, r)?;
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.huffman.reset();
        // Section F.2.1.3.1
        self.dc_predictors = [0i16; MAX_COMPONENTS];
        // Section G.1.2.2
        self.eob_run = 0;
        self.expected_rst_num = (self.expected_rst_num + 1) % 8;
        self.mcus_left_until_restart = self.restart_interval;
    }
}

fn refine_non_zeroes<R: Read>(reader: &mut R,
                              coefficients: &mut [i16],
                              huffman: &mut HuffmanDecoder,
                              range: RangeInclusive<u8>,
                              zrl: u8,
                              bit: i16) -> Result<u8> {
    debug_assert_eq!(coefficients.len(), 64);

    let last = *range.end();
    let mut zero_run_length = zrl;

    for i in range {
        let index = UNZIGZAG[i as usize] as usize;

        if coefficients[index] == 0 {
            if zero_run_length == 0 {
                return Ok(i);
            }

            zero_run_length -= 1;
        }
        else if huffman.get_bits(reader, 1)? == 1 && coefficients[index] & bit == 0 {
            if coefficients[index] > 0 {
                coefficients[index] += bit;
            }
            else {
                coefficients[index] -= bit;
            }
        }
    }

    Ok(last)
}

fn compute_image(components: &[Component],
                 data: &[Vec<u8>],
                 output_size: Dimensions,
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
        compute_image_parallel(components, data, output_size, is_jfif, color_transform)
    }
}

#[cfg(feature="rayon")]
fn compute_image_parallel(components: &[Component],
                 data: &[Vec<u8>],
                 output_size: Dimensions,
                 is_jfif: bool,
                 color_transform: Option<AdobeColorTransform>) -> Result<Vec<u8>> {
    use rayon::prelude::*;

    let color_convert_func = choose_color_convert_func(components.len(), is_jfif, color_transform)?;
    let upsampler = Upsampler::new(components, output_size.width, output_size.height)?;
    let line_size = output_size.width as usize * components.len();
    let mut image = vec![0u8; line_size * output_size.height as usize];

    image.par_chunks_mut(line_size)
         .with_max_len(1)
         .enumerate()
         .for_each(|(row, line)| {
             upsampler.upsample_and_interleave_row(data, row, output_size.width as usize, line);
             color_convert_func(line, output_size.width as usize);
         });

    Ok(image)
 }

#[cfg(not(feature="rayon"))]
fn compute_image_parallel(components: &[Component],
                 data: &[Vec<u8>],
                 output_size: Dimensions,
                 is_jfif: bool,
                 color_transform: Option<AdobeColorTransform>) -> Result<Vec<u8>> {
    let color_convert_func = choose_color_convert_func(components.len(), is_jfif, color_transform)?;
    let upsampler = Upsampler::new(components, output_size.width, output_size.height)?;
    let line_size = output_size.width as usize * components.len();
    let mut image = vec![0u8; line_size * output_size.height as usize];

    for (row, line) in image.chunks_mut(line_size)
         .enumerate() {
             upsampler.upsample_and_interleave_row(data, row, output_size.width as usize, line);
             color_convert_func(line, output_size.width as usize);
         }

    Ok(image)
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
    let y  = f32::from(y);
    let cb = f32::from(cb) - 128.0;
    let cr = f32::from(cr) - 128.0;

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
