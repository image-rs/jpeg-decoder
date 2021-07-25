use parser::Predictor;
use error::{Error, Result, UnsupportedFeature};
use decoder::{Decoder, MAX_COMPONENTS};
use parser::{AdobeColorTransform, AppData, CodingProcess, Component, Dimensions, EntropyCoding, FrameInfo,
    parse_app, parse_com, parse_dht, parse_dqt, parse_dri, parse_sof, parse_sos, IccChunk,
    ScanInfo};
use huffman::{fill_default_mjpeg_tables, HuffmanDecoder, HuffmanTable};
use marker::Marker;
use std::io::Read;

impl<R: Read> Decoder<R> {
    /// decode_scan_lossless
    pub fn decode_scan_lossless(&mut self,
                frame: &FrameInfo,
                scan: &ScanInfo)
                -> Result<(Option<Marker>, Option<Vec<Vec<u8>>>)> {
        assert!(scan.component_indices.len() <= MAX_COMPONENTS);
        let mut results: Vec<Vec<u8>> = vec![Vec::new(); MAX_COMPONENTS];

        let components: Vec<Component> = scan.component_indices.iter()
                                                            .map(|&i| frame.components[i].clone())
                                                            .collect();

        // Verify that all required huffman tables has been set.
        if scan.dc_table_indices.iter().any(|&i| self.dc_huffman_tables[i].is_none()) {
            return Err(Error::Format("scan makes use of unset dc huffman table".to_owned()));
        }

        let mut huffman = HuffmanDecoder::new();
        let reader = &mut self.reader;
        
        for mcu_y in 0..frame.image_size.height {
            for mcu_x in 0..frame.image_size.width {
                
                
                for (i, component) in components.iter().enumerate() {
                    let dc_table = self.dc_huffman_tables[scan.dc_table_indices[i]].as_ref().unwrap();
                    let value = huffman.decode(reader, dc_table)?;
                    let diff = match value {
                        0 => 0,
                        1..=16 => huffman.receive_extend(reader, value)?,
                        _ => {
                            // Section F.1.2.1.1
                            // Table F.1
                            return Err(Error::Format("invalid DC difference magnitude category".to_owned()));
                        },
                    };
                    results[i].push(0);
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