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

        let components: Vec<Component> = scan.component_indices.iter()
                                                            .map(|&i| frame.components[i].clone())
                                                            .collect();

        // Verify that all required huffman tables has been set.
        if scan.dc_table_indices.iter().any(|&i| self.dc_huffman_tables[i].is_none()) {
            return Err(Error::Format("scan makes use of unset dc huffman table".to_owned()));
        }

        let mut huffman = HuffmanDecoder::new();

        let mut marker = huffman.take_marker(&mut self.reader)?;
        while let Some(Marker::RST(_)) = marker {
            marker = self.read_marker().ok();
        }

        Ok((marker, None))
    }
}