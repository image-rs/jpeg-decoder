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
        assert!(scan.component_indices.len() <= MAX_COMPONENTS);
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
        let mut ra = 0;
        let mut rb = 0;
        let mut rc = 0;
        for mcu_y in 0..frame.image_size.height as usize {
            for mcu_x in 0..frame.image_size.width as usize {
                for (i, component) in components.iter().enumerate() {
                    let dc_table = self.dc_huffman_tables[scan.dc_table_indices[i]]
                        .as_ref()
                        .unwrap();
                    let value = huffman.decode(reader, dc_table)?;
                    let diff = match value {
                        0 => 0,
                        1..=15 => huffman.receive_extend(reader, value)?,
                        // 16 => 32768,
                        _ => {
                            // Section F.1.2.1.1
                            // Table F.1
                            return Err(Error::Format(
                                "invalid DC difference magnitude category".to_owned(),
                            ));
                        }
                    };
                    if mcu_y > 0 {
                        rb = results[i][(mcu_y - 1) * frame.image_size.width as usize + mcu_x];
                        if mcu_x > 0 {
                            rc = results[i]
                                [(mcu_y - 1) * frame.image_size.width as usize + (mcu_x - 1)];
                        }
                    }
                    let prediction = reverse_predict(
                        diff,
                        ra,
                        rb,
                        rc,
                        scan.predictor_selection,
                        scan.point_transform,
                        frame.precision,
                        mcu_x,
                        mcu_y,
                        false,
                    );
                    results[i].push(prediction << scan.point_transform);
                    ra = prediction;
                }
            }
        }

        let mut marker = huffman.take_marker(&mut self.reader)?;
        while let Some(Marker::RST(_)) = marker {
            marker = self.read_marker().ok();
        }

        println!("image size : {:?}", frame.image_size);
        println!("results size : {:?}", results[0].len());
        println!("ouput size : {:?}", frame.output_size);
        println!("point transform : {:?}", scan.point_transform);
        println!("predictor : {:?}", scan.predictor_selection);
        Ok((marker, Some(results)))
    }
}

/// H.1.2.1
fn reverse_predict(
    diff: i16,
    ra: u16,
    rb: u16,
    rc: u16,
    predictor: Predictor,
    point_transform: u8,
    input_precision: u8,
    ix: usize,
    iy: usize,
    restart: bool,
) -> u16 {
    let prediction = if (ix == 0 && iy == 0) || restart {
        // start of first line or restart
        0 //1 << (input_precision - point_transform - 1)
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
            Predictor::RaRbRc2 => ra + (rb - rc) >> 1,
            Predictor::RaRbRc3 => ra + (rc - rb) >> 1,
            Predictor::RaRb => (ra + rb) / 2,
        }
    };
    diff.wrapping_add(prediction as i16) as u16
}
