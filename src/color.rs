#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColorSpace {
    /// Monochrome
    Grayscale,
    /// Red/Green/Blue
    RGB,
    /// Cyan/Magenta/Yellow/Key(black)
    CMYK,

    /// Y/Cb/Cr, also known as YUV.
    YCbCr,
    /// Y/Cb/Cr/Key(black)
    YCCK,
}

impl ColorSpace {
    pub fn num_components(&self) -> usize {
        match *self {
            ColorSpace::Grayscale => 1,
            ColorSpace::RGB | ColorSpace::YCbCr => 3,
            ColorSpace::CMYK | ColorSpace::YCCK => 4,
        }
    }
}

struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

struct YCbCr {
    y: u8,
    cb: u8,
    cr: u8,
}

struct Cmyk {
    c: u8,
    m: u8,
    y: u8,
    k: u8,
}

struct Ycck {
    ycbcr: YCbCr,
    k: u8,
}

impl From<YCbCr> for Rgb {
    fn from(color: YCbCr) -> Rgb {
        // ITU-R BT.601

        let y  = color.y as f32;
        let cb = color.cb as f32 - 128.0;
        let cr = color.cr as f32 - 128.0;

        let r = y                + 1.40200 * cr;
        let g = y - 0.34414 * cb - 0.71414 * cr;
        let b = y + 1.77200 * cb;

        Rgb {
            r: clamp((r + 0.5) as i32, 0, 255) as u8,
            g: clamp((g + 0.5) as i32, 0, 255) as u8,
            b: clamp((b + 0.5) as i32, 0, 255) as u8,
        }
    }
}

impl From<Ycck> for Cmyk {
    fn from(color: Ycck) -> Cmyk {
        let rgb = Rgb::from(color.ycbcr);

        Cmyk {
            c: rgb.r,
            m: rgb.g,
            y: rgb.b,
            k: color.k,
        }
    }
}

fn clamp<T: PartialOrd>(value: T, min: T, max: T) -> T {
    if value < min { return min; }
    if value > max { return max; }
    value
}

pub trait ConvertColorSpace<To> {
    fn convert(&self, to: &To, data: &mut [u8], length: usize);
}

impl ConvertColorSpace<ColorSpace> for ColorSpace {
    fn convert(&self, to: &ColorSpace, data: &mut [u8], length: usize) {
        match (*self, *to) {
            (ColorSpace::RGB, ColorSpace::RGB) => {
                // Nothing to be done.
            },
            (ColorSpace::YCbCr, ColorSpace::RGB) => {
                for i in 0 .. length {
                    let rgb = Rgb::from(YCbCr {
                        y:  data[i * 3],
                        cb: data[i * 3 + 1],
                        cr: data[i * 3 + 2],
                    });

                    data[i * 3]     = rgb.r;
                    data[i * 3 + 1] = rgb.g;
                    data[i * 3 + 2] = rgb.b;
                }
            },
            (ColorSpace::CMYK, ColorSpace::CMYK) => {
                for i in 0 .. length {
                    // CMYK is stored inversed.
                    data[i * 4]     = 255 - data[i * 4];
                    data[i * 4 + 1] = 255 - data[i * 4 + 1];
                    data[i * 4 + 2] = 255 - data[i * 4 + 2];
                    data[i * 4 + 3] = 255 - data[i * 4 + 3];
                }
            },
            (ColorSpace::YCCK, ColorSpace::CMYK) => {
                for i in 0 .. length {
                    let cmyk = Cmyk::from(Ycck {
                        ycbcr: YCbCr {
                            y:  data[i * 4],
                            cb: data[i * 4 + 1],
                            cr: data[i * 4 + 2],
                        },
                        // K is stored inversed, same as CMYK.
                        k: 255 - data[i * 4 + 3],
                    });

                    data[i * 4]     = cmyk.c;
                    data[i * 4 + 1] = cmyk.m;
                    data[i * 4 + 2] = cmyk.y;
                    data[i * 4 + 3] = cmyk.k;
                }
            },
            (_, _) => panic!(),
        }
    }
}
