use decoder::{Decoder, MAX_COMPONENTS};
use error::{Error, Result, UnsupportedFeature};
use huffman::{fill_default_mjpeg_tables, HuffmanDecoder, HuffmanTable};
use marker::Marker;
use parser::Predictor;
use parser::{
    parse_app, parse_com, parse_dht, parse_dqt, parse_dri, parse_sof, parse_sos,
    AdobeColorTransform, AppData, CodingProcess, Component, Dimensions, EntropyCoding, FrameInfo,
    IccChunk, ScanInfo,
};
use std::io::Read;

impl<R: Read> Decoder<R> {
    /// decode_scan_lossless
    pub fn decode_scan_lossless(
        &mut self,
        frame: &FrameInfo,
        scan: &ScanInfo,
    ) -> Result<(Option<Marker>, Option<Vec<Vec<u16>>>)> {
        let ncomp = scan.component_indices.len();
        assert!(ncomp <= MAX_COMPONENTS);
        let mut results = vec![Vec::new(); MAX_COMPONENTS];

        let components: Vec<Component> = scan
            .component_indices
            .iter()
            .map(|&i| frame.components[i].clone())
            .collect();

        // Verify that all required huffman tables has been set.
        if scan
            .dc_table_indices
            .iter()
            .any(|&i| self.dc_huffman_tables[i].is_none())
        {
            return Err(Error::Format(
                "scan makes use of unset dc huffman table".to_owned(),
            ));
        }

        let mut huffman = HuffmanDecoder::new();
        let reader = &mut self.reader;
        let mut mcus_left_until_restart = self.restart_interval;
        let mut expected_rst_num = 0;
        let mut ra = [0u16; MAX_COMPONENTS];
        let mut rb = [0u16; MAX_COMPONENTS];
        let mut rc = [0u16; MAX_COMPONENTS];

        for mcu_y in 0..frame.image_size.height as usize {
            for mcu_x in 0..frame.image_size.width as usize {
                
                if self.restart_interval > 0 {
                    if mcus_left_until_restart == 0 {
                        match huffman.take_marker(reader)? {
                            Some(Marker::RST(n)) => {
                                if n != expected_rst_num {
                                    return Err(Error::Format(format!("found RST{} where RST{} was expected", n, expected_rst_num)));
                                }

                                huffman.reset();

                                expected_rst_num = (expected_rst_num + 1) % 8;
                                mcus_left_until_restart = self.restart_interval;
                            },
                            Some(marker) => return Err(Error::Format(format!("found marker {:?} inside scan where RST{} was expected", marker, expected_rst_num))),
                            None => return Err(Error::Format(format!("no marker found where RST{} was expected", expected_rst_num))),
                        }
                    }

                    mcus_left_until_restart -= 1;
                }

                for (i, component) in components.iter().enumerate() {
                    let dc_table = self.dc_huffman_tables[scan.dc_table_indices[i]]
                        .as_ref()
                        .unwrap();
                    let value = huffman.decode(reader, dc_table)?;
                    let diff = match value {
                        0 => 0,
                        1..=15 => huffman.receive_extend(reader, value)? as i32,
                        16 => 32768,
                        _ => {
                            // Section F.1.2.1.1
                            // Table F.1
                            return Err(Error::Format(
                                "invalid DC difference magnitude category".to_owned(),
                            ));
                        }
                    };
                    
                    if mcu_x > 0 {
                        ra[i] = results[i][mcu_y * frame.image_size.width as usize + mcu_x - 1];
                    }
                    if mcu_y > 0 {
                        rb[i] = results[i][(mcu_y - 1) * frame.image_size.width as usize + mcu_x];
                        if mcu_x > 0 {
                            rc[i] = results[i]
                                [(mcu_y - 1) * frame.image_size.width as usize + (mcu_x - 1)];
                        }
                    }
                    let prediction = predict(
                        ra[i] as i32,
                        rb[i] as i32,
                        rc[i] as i32,
                        scan.predictor_selection,
                        scan.point_transform,
                        frame.precision,
                        mcu_x,
                        mcu_y,
                        self.restart_interval > 0 && mcus_left_until_restart == self.restart_interval - 1,
                    );
                    let result = ((prediction + diff) & 0xFFFF) as u16; // modulo 2^16
                    results[i].push(result << scan.point_transform);
                }
            }
        }

        let mut marker = huffman.take_marker(&mut self.reader)?;
        while let Some(Marker::RST(_)) = marker {
            marker = self.read_marker().ok();
        }
        
        Ok((marker, Some(results)))
    }
}

/// H.1.2.1
fn predict(
    ra: i32,
    rb: i32,
    rc: i32,
    predictor: Predictor,
    point_transform: u8,
    input_precision: u8,
    ix: usize,
    iy: usize,
    restart: bool,
) -> i32 {
    let result = if (ix == 0 && iy == 0) || restart {
        // start of first line or restart
        1 << (input_precision - point_transform - 1)
    } else if iy == 0 {
        // rest of first line
        ra
    } else if ix == 0 {
        // start of other line
        rb
    } else {
        // use predictor Table H.1
        match predictor {
            Predictor::NoPrediction => 0,
            Predictor::Ra => ra,
            Predictor::Rb => rb,
            Predictor::Rc => rc,
            Predictor::RaRbRc1 => ra + rb - rc,
            Predictor::RaRbRc2 => ra + ((rb - rc) >> 1),
            Predictor::RaRbRc3 => rb + ((ra - rc) >> 1),
            Predictor::RaRb => (ra + rb) / 2,
        }
    };
    result
}
