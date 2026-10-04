//! The application segments a decoder reports and an encoder writes: JFIF
//! (APP0, T.871), EXIF (APP1, CIPA DC-008), ICC profiles (APP2, ICC.1 Annex
//! B.4) and Adobe's colour-transform segment (APP14).

/// The JFIF APP0 segment (T.871 10.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Jfif {
    /// Major and minor version, e.g. `(1, 2)`.
    pub version: (u8, u8),
    /// 0: no units (the densities give the pixel aspect ratio only); 1: dots
    /// per inch; 2: dots per centimetre.
    pub units: u8,
    /// Horizontal pixel density.
    pub x_density: u16,
    /// Vertical pixel density.
    pub y_density: u16,
}

impl Jfif {
    pub(crate) fn parse(p: &[u8]) -> Option<Self> {
        if p.len() < 14 || &p[..5] != b"JFIF\0" {
            return None;
        }
        Some(Self {
            version: (p[5], p[6]),
            units: p[7],
            x_density: u16::from_be_bytes([p[8], p[9]]),
            y_density: u16::from_be_bytes([p[10], p[11]]),
        })
    }
}

/// Adobe's APP14 segment, as described in Adobe's note on the DCT filters
/// (Technical Note 5116): the colour transform flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adobe {
    /// The DCTEncode version (100 or 101).
    pub version: u16,
    /// flags0.
    pub flags0: u16,
    /// flags1.
    pub flags1: u16,
    /// 0: the components are stored untransformed (RGB or CMYK); 1: YCbCr
    /// (three components); 2: YCCK (four components).
    pub transform: u8,
}

impl Adobe {
    pub(crate) fn parse(p: &[u8]) -> Option<Self> {
        if p.len() < 12 || &p[..5] != b"Adobe" {
            return None;
        }
        Some(Self {
            version: u16::from_be_bytes([p[5], p[6]]),
            flags0: u16::from_be_bytes([p[7], p[8]]),
            flags1: u16::from_be_bytes([p[9], p[10]]),
            transform: p[11],
        })
    }
}

/// The EXIF orientation (TIFF tag 0x0112), 1–8: how the stored picture is
/// to be turned to be upright. Reported, never applied by this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Orientation {
    /// 1: as stored.
    Normal,
    /// 2: mirrored left to right.
    FlipHorizontal,
    /// 3: turned 180 degrees.
    Rotate180,
    /// 4: mirrored top to bottom.
    FlipVertical,
    /// 5: mirrored about the top-left to bottom-right diagonal.
    Transpose,
    /// 6: to be turned 90 degrees clockwise to be upright.
    Rotate90,
    /// 7: mirrored about the top-right to bottom-left diagonal.
    Transverse,
    /// 8: to be turned 90 degrees anticlockwise (270 clockwise).
    Rotate270,
}

impl Orientation {
    /// From the tag value; `None` outside 1–8.
    pub fn from_exif(v: u16) -> Option<Self> {
        Some(match v {
            1 => Self::Normal,
            2 => Self::FlipHorizontal,
            3 => Self::Rotate180,
            4 => Self::FlipVertical,
            5 => Self::Transpose,
            6 => Self::Rotate90,
            7 => Self::Transverse,
            8 => Self::Rotate270,
            _ => return None,
        })
    }

    /// The tag value, 1–8.
    pub fn to_exif(self) -> u16 {
        self as u16 + 1
    }

    /// Whether making the picture upright swaps its width and height.
    pub fn swaps_dimensions(self) -> bool {
        matches!(
            self,
            Self::Transpose | Self::Rotate90 | Self::Transverse | Self::Rotate270
        )
    }
}

/// The orientation tag of IFD0 in an EXIF payload (the TIFF structure after
/// `Exif\0\0`).
pub(crate) fn exif_orientation(tiff: &[u8]) -> Option<Orientation> {
    if tiff.len() < 8 {
        return None;
    }
    let le = match &tiff[..4] {
        b"II*\0" => true,
        b"MM\0*" => false,
        _ => return None,
    };
    let u16_at = |o: usize| -> Option<u16> {
        let b = tiff.get(o..o + 2)?;
        Some(if le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b = tiff.get(o..o + 4)?;
        let a = [b[0], b[1], b[2], b[3]];
        Some(if le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    };
    let ifd = usize::try_from(u32_at(4)?).ok()?;
    let count = usize::from(u16_at(ifd)?);
    for i in 0..count {
        let e = ifd + 2 + i * 12;
        if u16_at(e)? == 0x0112 {
            // SHORT, count 1: the value sits in the first two bytes of the
            // value field.
            let kind = u16_at(e + 2)?;
            if kind != 3 {
                return None;
            }
            return Orientation::from_exif(u16_at(e + 8)?);
        }
    }
    None
}

/// Collects `ICC_PROFILE` chunks (ICC.1 B.4): each carries its sequence
/// number (from 1) and the chunk count; the profile is their payloads in
/// sequence order.
#[derive(Debug, Default)]
pub(crate) struct IccChunks {
    chunks: Vec<(u8, u8, Vec<u8>)>,
}

impl IccChunks {
    pub(crate) fn push(&mut self, p: &[u8]) -> bool {
        if p.len() < 14 || &p[..12] != b"ICC_PROFILE\0" {
            return false;
        }
        self.chunks.push((p[12], p[13], p[14..].to_vec()));
        true
    }

    /// The profile, or `None` (with the reason) when there is none or the
    /// chunks do not make a whole one.
    pub(crate) fn assemble(&self) -> Result<Option<Vec<u8>>, String> {
        if self.chunks.is_empty() {
            return Ok(None);
        }
        let count = self.chunks[0].1;
        if count == 0 || self.chunks.iter().any(|c| c.1 != count) {
            return Err("ICC profile chunks disagree on their count".into());
        }
        let mut out = Vec::new();
        for seq in 1..=count {
            let Some(c) = self.chunks.iter().find(|c| c.0 == seq) else {
                return Err(format!("ICC profile chunk {seq} of {count} is missing"));
            };
            out.extend_from_slice(&c.2);
        }
        Ok(Some(out))
    }
}

/// The most profile bytes one APP2 segment carries: 65535 less the length
/// field, the identifier and the two sequence bytes.
pub(crate) const ICC_CHUNK: usize = 65535 - 2 - 12 - 2;

/// The most EXIF bytes one APP1 segment carries.
pub(crate) const EXIF_MAX: usize = 65535 - 2 - 6;
