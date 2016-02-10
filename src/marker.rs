// Table B.1
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Marker {
    // Start Of Frame markers, non-differential, Huffman coding
    SOF0  = 0xC0, // Baseline DCT
    SOF1  = 0xC1, // Extended sequential DCT
    SOF2  = 0xC2, // Progressive DCT
    SOF3  = 0xC3, // Lossless (sequential)

    // Start Of Frame markers, differential, Huffman coding
    SOF5  = 0xC5, // Differential sequential DCT
    SOF6  = 0xC6, // Differential progressive DCT
    SOF7  = 0xC7, // Differential lossless (sequential)

    // Start Of Frame markers, non-differential, arithmetic coding
    JPG   = 0xC8, // Reserved for JPEG extensions
    SOF9  = 0xC9, // Extended sequential DCT
    SOF10 = 0xCA, // Progressive DCT
    SOF11 = 0xCB, // Lossless (sequential)

    // Start Of Frame markers, differential, arithmetic coding
    SOF13 = 0xCD, // Differential sequential DCT
    SOF14 = 0xCE, // Differential progressive DCT
    SOF15 = 0xCF, // Differential lossless (sequential)

    // Huffman table specification
    DHT   = 0xC4, // Define Huffman table(s)

    // Arithmetic coding conditioning specification
    DAC   = 0xCC, // Define arithmetic coding conditioning(s)

    // Restart interval termination
    RST0  = 0xD0, // Restart with modulo 8 count “m”
    RST1  = 0xD1,
    RST2  = 0xD2,
    RST3  = 0xD3,
    RST4  = 0xD4,
    RST5  = 0xD5,
    RST6  = 0xD6,
    RST7  = 0xD7,

    // Other markers
    SOI   = 0xD8, // Start of image
    EOI   = 0xD9, // End of image
    SOS   = 0xDA, // Start of scan
    DQT   = 0xDB, // Define quantization table(s)
    DNL   = 0xDC, // Define number of lines
    DRI   = 0xDD, // Define restart interval
    DHP   = 0xDE, // Define hierarchical progression
    EXP   = 0xDF, // Expand reference component(s)
    APP0  = 0xE0, // Reserved for application segments
    APP1  = 0xE1,
    APP2  = 0xE2,
    APP3  = 0xE3,
    APP4  = 0xE4,
    APP5  = 0xE5,
    APP6  = 0xE6,
    APP7  = 0xE7,
    APP8  = 0xE8,
    APP9  = 0xE9,
    APP10 = 0xEA,
    APP11 = 0xEB,
    APP12 = 0xEC,
    APP13 = 0xED,
    APP14 = 0xEE,
    APP15 = 0xEF,
    JPG0  = 0xF0, // Reserved for JPEG extensions
    JPG1  = 0xF1,
    JPG2  = 0xF2,
    JPG3  = 0xF3,
    JPG4  = 0xF4,
    JPG5  = 0xF5,
    JPG6  = 0xF6,
    JPG7  = 0xF7,
    JPG8  = 0xF8,
    JPG9  = 0xF9,
    JPG10 = 0xFA,
    JPG11 = 0xFB,
    JPG12 = 0xFC,
    JPG13 = 0xFD,
    COM   = 0xFE, // Comment

    // Reserved markers
    TEM   = 0x01, // For temporary private use in arithmetic coding
    // RES is really 0x02 through 0xBF
    RES   = 0x02, // Reserved
}

impl Marker {
    pub fn has_length(self) -> bool {
        match self {
            Marker::RST0 | Marker::RST1 | Marker::RST2 | Marker::RST3 | Marker::RST4 |
            Marker::RST5 | Marker::RST6 | Marker::RST7 | Marker::SOI | Marker::EOI |
            Marker::TEM => false,
            _ => true,
        }
    }

    pub fn from_u8(n: u8) -> Option<Marker> {
        match n {
            0x00 => None, // Byte stuffing
            0x01 => Some(Marker::TEM),
            0x02 ... 0xBF => Some(Marker::RES),
            0xC0 => Some(Marker::SOF0),
            0xC1 => Some(Marker::SOF1),
            0xC2 => Some(Marker::SOF2),
            0xC3 => Some(Marker::SOF3),
            0xC4 => Some(Marker::DHT),
            0xC5 => Some(Marker::SOF5),
            0xC6 => Some(Marker::SOF6),
            0xC7 => Some(Marker::SOF7),
            0xC8 => Some(Marker::JPG),
            0xC9 => Some(Marker::SOF9),
            0xCA => Some(Marker::SOF10),
            0xCB => Some(Marker::SOF11),
            0xCC => Some(Marker::DAC),
            0xCD => Some(Marker::SOF13),
            0xCE => Some(Marker::SOF14),
            0xCF => Some(Marker::SOF15),
            0xD0 => Some(Marker::RST0),
            0xD1 => Some(Marker::RST1),
            0xD2 => Some(Marker::RST2),
            0xD3 => Some(Marker::RST3),
            0xD4 => Some(Marker::RST4),
            0xD5 => Some(Marker::RST5),
            0xD6 => Some(Marker::RST6),
            0xD7 => Some(Marker::RST7),
            0xD8 => Some(Marker::SOI),
            0xD9 => Some(Marker::EOI),
            0xDA => Some(Marker::SOS),
            0xDB => Some(Marker::DQT),
            0xDC => Some(Marker::DNL),
            0xDD => Some(Marker::DRI),
            0xDE => Some(Marker::DHP),
            0xDF => Some(Marker::EXP),
            0xE0 => Some(Marker::APP0),
            0xE1 => Some(Marker::APP1),
            0xE2 => Some(Marker::APP2),
            0xE3 => Some(Marker::APP3),
            0xE4 => Some(Marker::APP4),
            0xE5 => Some(Marker::APP5),
            0xE6 => Some(Marker::APP6),
            0xE7 => Some(Marker::APP7),
            0xE8 => Some(Marker::APP8),
            0xE9 => Some(Marker::APP9),
            0xEA => Some(Marker::APP10),
            0xEB => Some(Marker::APP11),
            0xEC => Some(Marker::APP12),
            0xED => Some(Marker::APP13),
            0xEE => Some(Marker::APP14),
            0xEF => Some(Marker::APP15),
            0xF0 => Some(Marker::JPG0),
            0xF1 => Some(Marker::JPG1),
            0xF2 => Some(Marker::JPG2),
            0xF3 => Some(Marker::JPG3),
            0xF4 => Some(Marker::JPG4),
            0xF5 => Some(Marker::JPG5),
            0xF6 => Some(Marker::JPG6),
            0xF7 => Some(Marker::JPG7),
            0xF8 => Some(Marker::JPG8),
            0xF9 => Some(Marker::JPG9),
            0xFA => Some(Marker::JPG10),
            0xFB => Some(Marker::JPG11),
            0xFC => Some(Marker::JPG12),
            0xFD => Some(Marker::JPG13),
            0xFE => Some(Marker::COM),
            0xFF => None, // Fill byte
            _ => unreachable!(),
        }
    }
}
