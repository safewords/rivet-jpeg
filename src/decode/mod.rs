//! The decoder: the marker syntax of Annex B, the control procedures of
//! Annex E, and the scan decoders of Annexes F, G and H.

mod lossless;
mod scan;

use crate::error::{Error, Result, invalid, unsupported};
use crate::huffman::{DecodeTable, TableSpec};
use crate::metadata::{Adobe, IccChunks, Jfif, Orientation, exif_orientation};
use crate::tables::ZIGZAG;

/// The coding process of a frame (T.81 clause 4.11 and Table B.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Process {
    /// Baseline sequential DCT (SOF0): 8-bit, Huffman, two tables of each
    /// kind.
    Baseline,
    /// Extended sequential DCT (SOF1, SOF9): 8- or 12-bit.
    ExtendedSequential,
    /// Progressive DCT (SOF2, SOF10): spectral selection and successive
    /// approximation.
    Progressive,
    /// Lossless (SOF3, SOF11): predictive, 2 to 16 bits.
    Lossless,
}

/// The entropy coding of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Coding {
    /// Huffman coding (SOF0–SOF3).
    Huffman,
    /// Arithmetic coding (SOF9–SOF11).
    Arithmetic,
}

/// What the components mean, from the component count, the JFIF and Adobe
/// segments and the component identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColourSpace {
    /// One component.
    Grey,
    /// Y, Cb, Cr (T.871 7): JFIF, an Adobe transform of 1, or three
    /// components with no sign otherwise.
    YCbCr,
    /// R, G, B, untransformed: an Adobe transform of 0, or components
    /// identified as `R`, `G`, `B`.
    Rgb,
    /// C, M, Y, K, untransformed. With an Adobe segment the values are
    /// stored inverted (as Adobe applications write them: 0 is full ink).
    Cmyk,
    /// Y, Cb, Cr, K: CMYK whose first three components went through the
    /// YCbCr transform (Adobe transform 2).
    Ycck,
    /// Two components, or more than four: no defined meaning.
    Other,
}

/// One component as the frame header describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Component {
    /// Component identifier (Ci).
    pub id: u8,
    /// Horizontal sampling factor (Hi), 1–4.
    pub h: u8,
    /// Vertical sampling factor (Vi), 1–4.
    pub v: u8,
    /// Quantisation table selector (Tqi).
    pub quant_table: u8,
    /// Samples per line in this component: ceil(X * Hi / Hmax).
    pub width: u32,
    /// Lines in this component: ceil(Y * Vi / Vmax).
    pub height: u32,
}

/// What the headers say about a picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// Samples per line (X).
    pub width: u32,
    /// Lines (Y, or the DNL segment's count when Y is 0).
    pub height: u32,
    /// Sample precision in bits (P): 8 or 12 for the DCT processes, 2–16
    /// for lossless.
    pub precision: u8,
    /// The coding process.
    pub process: Process,
    /// The entropy coding.
    pub coding: Coding,
    /// The SOFn marker's code (0xC0–0xCF).
    pub sof_marker: u8,
    /// The components, in frame order.
    pub components: Vec<Component>,
    /// What the components mean.
    pub colour_space: ColourSpace,
    /// The restart interval in effect at the first scan (0: none).
    pub restart_interval: u16,
    /// Number of scans (all of them, after a full decode; 0 from
    /// [`read_info`]).
    pub scans: u32,
    /// The JFIF segment, if any.
    pub jfif: Option<Jfif>,
    /// The Adobe segment, if any.
    pub adobe: Option<Adobe>,
    /// The EXIF payload (the TIFF structure, after `Exif\0\0`), if any.
    pub exif: Option<Vec<u8>>,
    /// The EXIF orientation, if the EXIF payload has one. Reported only;
    /// the decoded samples are as stored.
    pub orientation: Option<Orientation>,
    /// The ICC profile, reassembled from its APP2 chunks, if any.
    pub icc_profile: Option<Vec<u8>>,
    /// The XMP packet (APP1 `http://ns.adobe.com/xap/1.0/`), if any.
    pub xmp: Option<Vec<u8>>,
    /// Comment (COM) segments, as bytes.
    pub comments: Vec<Vec<u8>>,
}

/// Decoder settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeOptions {
    /// Refuse a frame of more than this many pixels before allocating
    /// anything. `None`: no limit beyond what the frame header can say.
    pub max_pixels: Option<u64>,
    /// Treat every departure from T.81 as an error instead of decoding past
    /// it: data between marker segments, a missing or misnumbered restart
    /// marker, a Huffman code that matches nothing, padding that is not
    /// 1-bits, a reserved all-1s Huffman code, a progression that breaks the
    /// rules of G.1.1.1, a missing EOI, bytes after EOI. Off by default; the
    /// tests turn it on to check this crate's encoder.
    pub strict: bool,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self { max_pixels: Some(1 << 30), strict: false }
    }
}

/// One decoded component at its own resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plane {
    /// Samples per line in this component.
    pub width: usize,
    /// Lines in this component.
    pub height: usize,
    /// Distance between lines in `data` (at least `width`; the decoder
    /// keeps the padding to whole blocks).
    pub stride: usize,
    /// Samples, `0..2^precision`, line by line.
    pub data: Vec<u16>,
}

impl Plane {
    /// One line, `width` samples.
    pub fn row(&self, y: usize) -> &[u16] {
        &self.data[y * self.stride..y * self.stride + self.width]
    }
}

/// A decoded picture: its components as stored (not upsampled, not
/// colour-converted) and what the headers said. [`Image::to_rgb8`] and its
/// siblings convert.
#[derive(Debug, Clone)]
pub struct Image {
    /// The headers.
    pub info: Info,
    /// One plane per frame component, in frame order.
    pub planes: Vec<Plane>,
    /// False when the data ended (or broke) before the picture was
    /// complete: what was decoded is there, the rest is mid-grey (or, for a
    /// progressive picture, at the precision the scans that arrived gave).
    pub complete: bool,
    /// What the decoder stepped past, in words: a restart marker out of
    /// place, a truncated file, an ICC profile with a chunk missing.
    pub warnings: Vec<String>,
}

/// The headers only: everything up to the first scan, without decoding.
pub fn read_info(data: &[u8]) -> Result<Info> {
    let mut d = Decoder::new(data, DecodeOptions { max_pixels: None, strict: false });
    d.run(true)?;
    d.info()
}

/// Decode with the default options: lenient, at most 2^30 pixels.
pub fn decode(data: &[u8]) -> Result<Image> {
    decode_with(data, &DecodeOptions::default())
}

/// Decode with `options`.
pub fn decode_with(data: &[u8], options: &DecodeOptions) -> Result<Image> {
    let mut d = Decoder::new(data, *options);
    d.run(false)?;
    d.finish()
}

#[derive(Debug, Clone)]
pub(crate) struct FrameComp {
    pub(crate) id: u8,
    pub(crate) h: usize,
    pub(crate) v: usize,
    pub(crate) tq: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// Blocks (or, lossless, samples) covering the component alone.
    pub(crate) units_w: usize,
    pub(crate) units_h: usize,
    /// Plane stride and height: whole MCUs.
    pub(crate) stride: usize,
    pub(crate) rows: usize,
    /// Blocks per line in the coefficient store.
    pub(crate) blocks_stride: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Frame {
    pub(crate) sof: u8,
    pub(crate) precision: u8,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) comps: Vec<FrameComp>,
    pub(crate) hmax: usize,
    pub(crate) vmax: usize,
    pub(crate) mcus_x: usize,
    pub(crate) mcus_y: usize,
    pub(crate) progressive: bool,
    pub(crate) lossless: bool,
    pub(crate) arithmetic: bool,
}

impl Frame {
    /// Pixels per data unit edge: 8 for DCT, 1 for lossless.
    pub(crate) fn unit(&self) -> usize {
        if self.lossless { 1 } else { 8 }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ScanComp {
    /// Index into the frame's components.
    pub(crate) ci: usize,
    pub(crate) td: usize,
    pub(crate) ta: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Scan {
    pub(crate) comps: Vec<ScanComp>,
    pub(crate) ss: usize,
    pub(crate) se: usize,
    pub(crate) ah: u8,
    pub(crate) al: u8,
}

/// How a scan ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanEnd {
    /// Every MCU decoded.
    Complete,
    /// The data stopped short (truncated, or broken and lenient).
    Short,
}

/// Decoder tables and conditioning shared with the scan decoders.
pub(crate) struct Tables {
    pub(crate) dc: [Option<DecodeTable>; 4],
    pub(crate) ac: [Option<DecodeTable>; 4],
    /// DAC: (L, U) per DC/lossless table, Kx per AC table.
    pub(crate) dc_cond: [(u8, u8); 4],
    pub(crate) ac_kx: [u8; 4],
    pub(crate) quant: [Option<[u16; 64]>; 4],
}

pub(crate) struct Decoder<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) pos: usize,
    pub(crate) opts: DecodeOptions,
    pub(crate) tables: Tables,
    pub(crate) restart_interval: u16,
    pub(crate) first_restart_interval: Option<u16>,
    pub(crate) frame: Option<Frame>,
    /// Samples per component (stride x rows).
    pub(crate) planes: Vec<Vec<u16>>,
    /// Progressive: coefficients per component, 64 per block in zig-zag
    /// order.
    pub(crate) coefs: Vec<Vec<i16>>,
    /// The quantisation table each component was first decoded with.
    pub(crate) comp_quant: Vec<Option<[u16; 64]>>,
    /// Progressive bookkeeping for the strict checks: per component and
    /// coefficient, the Al of the last scan that coded it (-1: none yet).
    pub(crate) progression: Vec<[i8; 64]>,
    pub(crate) scans: u32,
    pub(crate) complete: bool,
    pub(crate) warnings: Vec<String>,
    jfif: Option<Jfif>,
    adobe: Option<Adobe>,
    exif: Option<Vec<u8>>,
    xmp: Option<Vec<u8>>,
    icc: IccChunks,
    comments: Vec<Vec<u8>>,
    seen_eoi: bool,
    /// Whether SOF0 was seen (the baseline restrictions apply).
    baseline: bool,
}

const SOI: u8 = 0xD8;
const EOI: u8 = 0xD9;
const SOS: u8 = 0xDA;
const DQT: u8 = 0xDB;
const DNL: u8 = 0xDC;
const DRI: u8 = 0xDD;
const DHP: u8 = 0xDE;
const EXP: u8 = 0xDF;
const DHT: u8 = 0xC4;
const DAC: u8 = 0xCC;
const COM: u8 = 0xFE;

impl<'a> Decoder<'a> {
    fn new(data: &'a [u8], opts: DecodeOptions) -> Self {
        Self {
            data,
            pos: 0,
            opts,
            tables: Tables {
                dc: Default::default(),
                ac: Default::default(),
                dc_cond: [(0, 1); 4],
                ac_kx: [5; 4],
                quant: [None; 4],
            },
            restart_interval: 0,
            first_restart_interval: None,
            frame: None,
            planes: Vec::new(),
            coefs: Vec::new(),
            comp_quant: Vec::new(),
            progression: Vec::new(),
            scans: 0,
            complete: true,
            warnings: Vec::new(),
            jfif: None,
            adobe: None,
            exif: None,
            xmp: None,
            icc: IccChunks::default(),
            comments: Vec::new(),
            seen_eoi: false,
            baseline: false,
        }
    }

    pub(crate) fn warn(&mut self, msg: impl Into<String>) -> Result<()> {
        let msg = msg.into();
        if self.opts.strict {
            return Err(invalid(msg));
        }
        if self.warnings.len() < 64 && !self.warnings.contains(&msg) {
            self.warnings.push(msg);
        }
        Ok(())
    }

    /// The next marker code at `pos`, skipping fill bytes; anything else
    /// before it is stepped over (an error when strict). `None` at the end
    /// of the data.
    fn next_marker(&mut self) -> Result<Option<u8>> {
        let start = self.pos;
        loop {
            while self.pos < self.data.len() && self.data[self.pos] != 0xFF {
                self.pos += 1;
            }
            while self.pos + 1 < self.data.len() && self.data[self.pos + 1] == 0xFF {
                self.pos += 1;
            }
            if self.pos + 1 >= self.data.len() {
                self.pos = self.data.len();
                return Ok(None);
            }
            let m = self.data[self.pos + 1];
            if m == 0x00 {
                self.pos += 2;
                continue;
            }
            if self.pos != start {
                // Fill bytes alone are fine; anything else is not.
                if self.data[start..self.pos].iter().any(|&b| b != 0xFF) {
                    self.warn(format!("{} bytes of data between marker segments", self.pos - start))?;
                }
            }
            self.pos += 2;
            return Ok(Some(m));
        }
    }

    /// The payload of the marker segment at `pos` (after the length).
    fn segment(&mut self) -> Result<Option<&'a [u8]>> {
        if self.pos + 2 > self.data.len() {
            return Ok(None);
        }
        let len = usize::from(u16::from_be_bytes([self.data[self.pos], self.data[self.pos + 1]]));
        if len < 2 {
            return Err(invalid("a marker segment length below 2"));
        }
        if self.pos + len > self.data.len() {
            return Ok(None);
        }
        let p = &self.data[self.pos + 2..self.pos + len];
        self.pos += len;
        Ok(Some(p))
    }

    /// Parse markers: up to the first SOS when `header_only`, else to EOI
    /// (or the end of the data).
    fn run(&mut self, header_only: bool) -> Result<()> {
        if self.data.len() < 2 || self.data[0] != 0xFF || self.data[1] != SOI {
            return Err(invalid("not a JPEG file: no SOI marker"));
        }
        self.pos = 2;
        loop {
            let Some(m) = self.next_marker()? else {
                if self.frame.is_none() || self.scans == 0 {
                    return Err(Error::Truncated("the data ends before the first scan".into()));
                }
                if !self.seen_eoi {
                    self.warn("the data ends without an EOI marker")?;
                }
                return Ok(());
            };
            match m {
                EOI => {
                    self.seen_eoi = true;
                    if self.frame.is_none() || self.scans == 0 {
                        return Err(invalid("EOI before any scan"));
                    }
                    if self.opts.strict && self.pos != self.data.len() {
                        return Err(invalid("data after EOI"));
                    }
                    return Ok(());
                }
                SOI => self.warn("a second SOI marker")?,
                0xD0..=0xD7 => self.warn("a restart marker outside a scan")?,
                0x01 => {} // TEM: no segment.
                SOS => {
                    if self.frame.is_none() {
                        return Err(invalid("SOS before the frame header"));
                    }
                    if header_only {
                        return Ok(());
                    }
                    let Some(p) = self.segment()? else {
                        if self.scans == 0 {
                            return Err(Error::Truncated("the data ends in the first scan header".into()));
                        }
                        self.complete = false;
                        self.warn("the data ends in a scan header")?;
                        return Ok(());
                    };
                    let scan = self.parse_sos(p)?;
                    if self.first_restart_interval.is_none() {
                        self.first_restart_interval = Some(self.restart_interval);
                    }
                    self.scans += 1;
                    let end = self.decode_scan(&scan)?;
                    if end == ScanEnd::Short {
                        self.complete = false;
                        return Ok(());
                    }
                }
                _ => {
                    let Some(p) = self.segment()? else {
                        if self.frame.is_none() || self.scans == 0 {
                            return Err(Error::Truncated(format!("the data ends in a marker segment (0xFF{m:02X})")));
                        }
                        self.complete = false;
                        self.warn("the data ends in a marker segment")?;
                        return Ok(());
                    };
                    self.segment_payload(m, p)?;
                }
            }
        }
    }

    fn segment_payload(&mut self, m: u8, p: &'a [u8]) -> Result<()> {
        match m {
            0xC0..=0xCF if m != DHT && m != DAC && m != 0xC8 => self.parse_sof(m, p)?,
            DHT => self.parse_dht(p)?,
            DQT => self.parse_dqt(p)?,
            DAC => self.parse_dac(p)?,
            DRI => {
                if p.len() != 2 {
                    return Err(invalid("DRI segment length is not 4"));
                }
                self.restart_interval = u16::from_be_bytes([p[0], p[1]]);
            }
            DNL => {
                if p.len() != 2 {
                    return Err(invalid("DNL segment length is not 4"));
                }
            }
            DHP | EXP => return Err(unsupported("the hierarchical mode (DHP / EXP)")),
            0xE0 => {
                if self.jfif.is_none() {
                    self.jfif = Jfif::parse(p);
                }
            }
            0xE1 => {
                if p.starts_with(b"Exif\0\0") || p.starts_with(b"Exif\0\xFF") {
                    if self.exif.is_none() {
                        self.exif = Some(p[6..].to_vec());
                    }
                } else if let Some(rest) = p.strip_prefix(b"http://ns.adobe.com/xap/1.0/\0")
                    && self.xmp.is_none()
                {
                    self.xmp = Some(rest.to_vec());
                }
            }
            0xE2 => {
                self.icc.push(p);
            }
            0xEE => {
                if self.adobe.is_none() {
                    self.adobe = Adobe::parse(p);
                }
            }
            COM => self.comments.push(p.to_vec()),
            _ => {} // Other APPn, JPGn, reserved: skipped.
        }
        Ok(())
    }

    fn parse_sof(&mut self, m: u8, p: &[u8]) -> Result<()> {
        if self.frame.is_some() {
            return Err(unsupported("more than one frame (the hierarchical mode)"));
        }
        let (progressive, lossless, arithmetic) = match m {
            0xC0 | 0xC1 => (false, false, false),
            0xC2 => (true, false, false),
            0xC3 => (false, true, false),
            0xC9 => (false, false, true),
            0xCA => (true, false, true),
            0xCB => (false, true, true),
            _ => return Err(unsupported(format!("differential frames (SOF{}), the hierarchical mode", m - 0xC0))),
        };
        self.baseline = m == 0xC0;
        if p.len() < 6 {
            return Err(invalid("frame header too short"));
        }
        let precision = p[0];
        let mut height = usize::from(u16::from_be_bytes([p[1], p[2]]));
        let width = usize::from(u16::from_be_bytes([p[3], p[4]]));
        let nf = usize::from(p[5]);
        if p.len() != 6 + 3 * nf {
            return Err(invalid("frame header length does not match its component count"));
        }
        let ok_precision = if lossless {
            (2..=16).contains(&precision)
        } else if m == 0xC0 {
            precision == 8
        } else {
            precision == 8 || precision == 12
        };
        if !ok_precision {
            return Err(invalid(format!("sample precision {precision} for SOF{}", m - 0xC0)));
        }
        if nf == 0 || (progressive && nf > 4) {
            return Err(invalid(format!("{nf} components in the frame")));
        }
        if width == 0 {
            return Err(invalid("a frame of width 0"));
        }
        if height == 0 {
            height = self.find_dnl().ok_or_else(|| invalid("height 0 in the frame header and no DNL segment"))?;
        }
        if let Some(limit) = self.opts.max_pixels
            && (width as u64) * (height as u64) > limit
        {
            return Err(Error::TooLarge { width: width as u32, height: height as u32, limit });
        }
        let mut comps = Vec::with_capacity(nf);
        for i in 0..nf {
            let c = &p[6 + 3 * i..9 + 3 * i];
            let (h, v, tq) = (usize::from(c[1] >> 4), usize::from(c[1] & 15), usize::from(c[2]));
            if !(1..=4).contains(&h) || !(1..=4).contains(&v) {
                return Err(invalid(format!("sampling factors {h}x{v}")));
            }
            if tq > 3 || (lossless && tq != 0 && self.opts.strict) {
                return Err(invalid(format!("quantisation table selector {tq}")));
            }
            if comps.iter().any(|x: &FrameComp| x.id == c[0]) {
                self.warn(format!("component identifier {} used twice", c[0]))?;
            }
            comps.push(FrameComp {
                id: c[0],
                h,
                v,
                tq,
                width: 0,
                height: 0,
                units_w: 0,
                units_h: 0,
                stride: 0,
                rows: 0,
                blocks_stride: 0,
            });
        }
        let hmax = comps.iter().map(|c| c.h).max().unwrap_or(1);
        let vmax = comps.iter().map(|c| c.v).max().unwrap_or(1);
        let unit = if lossless { 1 } else { 8 };
        let mcus_x = width.div_ceil(hmax * unit);
        let mcus_y = height.div_ceil(vmax * unit);
        for c in &mut comps {
            c.width = (width * c.h).div_ceil(hmax);
            c.height = (height * c.v).div_ceil(vmax);
            c.units_w = c.width.div_ceil(unit);
            c.units_h = c.height.div_ceil(unit);
            c.blocks_stride = mcus_x * c.h;
            c.stride = mcus_x * c.h * unit;
            c.rows = mcus_y * c.v * unit;
        }
        let frame = Frame { sof: m, precision, width, height, comps, hmax, vmax, mcus_x, mcus_y, progressive, lossless, arithmetic };
        // Allocate: planes (mid-grey, so missing data shows as grey), and
        // the coefficient store of a progressive frame.
        let mid = if lossless { 0 } else { 1u16 << (precision - 1) };
        self.planes = frame.comps.iter().map(|c| vec![mid; c.stride * c.rows]).collect();
        if progressive {
            self.coefs = frame.comps.iter().map(|c| vec![0i16; c.stride * c.rows]).collect();
        }
        self.comp_quant = vec![None; frame.comps.len()];
        self.progression = vec![[-1i8; 64]; frame.comps.len()];
        self.frame = Some(frame);
        Ok(())
    }

    /// The line count of a DNL segment after the first scan, for a frame
    /// header that gives 0 (B.2.5). Entropy-coded data cannot contain the
    /// marker, so it is found by looking for it.
    fn find_dnl(&self) -> Option<usize> {
        let d = self.data;
        let mut i = self.pos;
        while i + 5 < d.len() {
            if d[i] == 0xFF && d[i + 1] == DNL && d[i + 2] == 0 && d[i + 3] == 4 {
                let n = usize::from(u16::from_be_bytes([d[i + 4], d[i + 5]]));
                return (n > 0).then_some(n);
            }
            i += 1;
        }
        None
    }

    fn parse_dht(&mut self, mut p: &[u8]) -> Result<()> {
        while !p.is_empty() {
            if p.len() < 17 {
                return Err(invalid("DHT segment too short"));
            }
            let (tc, th) = (p[0] >> 4, usize::from(p[0] & 15));
            if tc > 1 || th > 3 {
                return Err(invalid(format!("Huffman table class {tc}, destination {th}")));
            }
            if self.baseline && th > 1 {
                self.warn("a baseline frame with a Huffman table in destination 2 or 3")?;
            }
            let mut bits = [0u8; 16];
            bits.copy_from_slice(&p[1..17]);
            let n: usize = bits.iter().map(|&b| usize::from(b)).sum();
            if p.len() < 17 + n {
                return Err(invalid("DHT segment shorter than its table"));
            }
            let spec = TableSpec::new(&bits, &p[17..17 + n]);
            let table = DecodeTable::new(&spec)?;
            if table.all_ones {
                self.warn("a Huffman table uses a reserved all-1s code")?;
            }
            if tc == 0 {
                self.tables.dc[th] = Some(table);
            } else {
                self.tables.ac[th] = Some(table);
            }
            p = &p[17 + n..];
        }
        Ok(())
    }

    fn parse_dqt(&mut self, mut p: &[u8]) -> Result<()> {
        while !p.is_empty() {
            let (pq, tq) = (p[0] >> 4, usize::from(p[0] & 15));
            if pq > 1 || tq > 3 {
                return Err(invalid(format!("quantisation table precision {pq}, destination {tq}")));
            }
            let size = if pq == 0 { 64 } else { 128 };
            if p.len() < 1 + size {
                return Err(invalid("DQT segment shorter than its table"));
            }
            let mut q = [0u16; 64];
            for k in 0..64 {
                let v = if pq == 0 { u16::from(p[1 + k]) } else { u16::from_be_bytes([p[1 + 2 * k], p[2 + 2 * k]]) };
                if v == 0 {
                    self.warn("a quantisation table entry of 0")?;
                }
                q[ZIGZAG[k]] = v.max(1);
            }
            if pq == 1 && self.frame.as_ref().is_some_and(|f| f.precision == 8) {
                self.warn("a 16-bit quantisation table in an 8-bit frame")?;
            }
            self.tables.quant[tq] = Some(q);
            p = &p[1 + size..];
        }
        Ok(())
    }

    fn parse_dac(&mut self, p: &[u8]) -> Result<()> {
        if p.len() % 2 != 0 {
            return Err(invalid("DAC segment length is odd"));
        }
        for c in p.chunks_exact(2) {
            let (tc, tb, cs) = (c[0] >> 4, usize::from(c[0] & 15), c[1]);
            if tc > 1 || tb > 3 {
                return Err(invalid(format!("arithmetic conditioning class {tc}, destination {tb}")));
            }
            if tc == 0 {
                let (l, u) = (cs & 15, cs >> 4);
                if l > u {
                    self.warn("arithmetic DC conditioning with L above U")?;
                }
                self.tables.dc_cond[tb] = (l, u);
            } else {
                if !(1..=63).contains(&cs) {
                    return Err(invalid(format!("arithmetic AC conditioning Kx = {cs}")));
                }
                self.tables.ac_kx[tb] = cs;
            }
        }
        Ok(())
    }

    fn parse_sos(&mut self, p: &[u8]) -> Result<Scan> {
        let Some(frame) = self.frame.clone() else { return Err(invalid("SOS before the frame header")) };
        if p.is_empty() {
            return Err(invalid("empty scan header"));
        }
        let ns = usize::from(p[0]);
        if !(1..=4).contains(&ns) || p.len() != 4 + 2 * ns {
            return Err(invalid("scan header length does not match its component count"));
        }
        let mut comps = Vec::with_capacity(ns);
        for i in 0..ns {
            let (cs, t) = (p[1 + 2 * i], p[2 + 2 * i]);
            let ci = frame
                .comps
                .iter()
                .position(|c| c.id == cs)
                .ok_or_else(|| invalid(format!("scan component {cs} is not in the frame")))?;
            if comps.iter().any(|c: &ScanComp| c.ci == ci) {
                return Err(invalid("a component twice in one scan"));
            }
            if let Some(prev) = comps.last()
                && ci < prev.ci
                && self.opts.strict
            {
                return Err(invalid("scan components out of frame order"));
            }
            let (td, ta) = (usize::from(t >> 4), usize::from(t & 15));
            if td > 3 || ta > 3 {
                return Err(invalid(format!("entropy table selectors {td}/{ta}")));
            }
            comps.push(ScanComp { ci, td, ta });
        }
        let q = &p[1 + 2 * ns..];
        let (ss, se, ah, al) = (usize::from(q[0]), usize::from(q[1]), q[2] >> 4, q[2] & 15);
        let blocks: usize = comps.iter().map(|c| frame.comps[c.ci].h * frame.comps[c.ci].v).sum();
        if ns > 1 && blocks > 10 {
            return Err(invalid(format!("{blocks} data units in an MCU (at most 10)")));
        }
        if frame.lossless {
            if !(1..=7).contains(&ss) {
                return Err(invalid(format!("lossless predictor {ss}")));
            }
        } else if frame.progressive {
            if ss > se || se > 63 || (ss == 0 && se != 0) || (ss > 0 && ns != 1) || ah > 13 || al > 13 {
                return Err(invalid(format!("progressive scan Ss={ss} Se={se} Ah={ah} Al={al} with {ns} components")));
            }
        } else if ss != 0 || se != 63 || ah != 0 || al != 0 {
            self.warn(format!("sequential scan with Ss={ss} Se={se} Ah={ah} Al={al}"))?;
        }
        let scan = Scan { comps, ss, se: if frame.progressive || frame.lossless { se } else { 63 }, ah, al };
        Ok(scan)
    }

    /// The strict progression checks of G.1.1.1, and recording the scan.
    fn check_progression(&mut self, scan: &Scan) -> Result<()> {
        let mut problems = Vec::new();
        for sc in &scan.comps {
            let state = &mut self.progression[sc.ci];
            for k in scan.ss..=scan.se {
                let prev = state[k];
                let ok = if scan.ah == 0 { prev == -1 } else { prev >= 0 && prev as u8 == scan.ah && scan.al + 1 == scan.ah };
                if !ok {
                    problems.push(format!(
                        "component {} coefficient {k}: Ah={} Al={} after {}",
                        sc.ci,
                        scan.ah,
                        scan.al,
                        if prev < 0 { "no scan".to_string() } else { format!("Al={prev}") }
                    ));
                    break;
                }
                if k > 0 && state[0] < 0 {
                    problems.push(format!("component {}: AC scan before its DC scan", sc.ci));
                    break;
                }
                state[k] = scan.al as i8;
            }
        }
        for p in problems {
            self.warn(format!("progression: {p}"))?;
        }
        Ok(())
    }

    fn decode_scan(&mut self, scan: &Scan) -> Result<ScanEnd> {
        let frame = self.frame.clone().ok_or_else(|| invalid("no frame"))?;
        // Every table the scan uses must be there.
        for sc in &scan.comps {
            let fc = &frame.comps[sc.ci];
            if !frame.lossless {
                let q = self.tables.quant[fc.tq].ok_or_else(|| invalid(format!("quantisation table {} is not defined", fc.tq)))?;
                if self.comp_quant[sc.ci].is_none() {
                    self.comp_quant[sc.ci] = Some(q);
                }
            }
            if !frame.arithmetic {
                let need_dc = frame.lossless || scan.ss == 0 && scan.ah == 0;
                let need_ac = !frame.lossless && scan.se > 0;
                if need_dc && self.tables.dc[sc.td].is_none() {
                    return Err(invalid(format!("Huffman DC table {} is not defined", sc.td)));
                }
                if need_ac && self.tables.ac[sc.ta].is_none() {
                    return Err(invalid(format!("Huffman AC table {} is not defined", sc.ta)));
                }
                if self.baseline && (sc.td > 1 || sc.ta > 1) {
                    self.warn("a baseline scan selecting Huffman table 2 or 3")?;
                }
            }
        }
        if frame.progressive {
            self.check_progression(scan)?;
        }
        if frame.lossless {
            lossless::decode(self, &frame, scan)
        } else {
            scan::decode(self, &frame, scan)
        }
    }

    fn info(&mut self) -> Result<Info> {
        let frame = self.frame.as_ref().ok_or_else(|| invalid("no frame header"))?;
        let icc = match self.icc.assemble() {
            Ok(p) => p,
            Err(e) => {
                if self.warnings.len() < 64 {
                    self.warnings.push(e);
                }
                None
            }
        };
        let nf = frame.comps.len();
        let ids: Vec<u8> = frame.comps.iter().map(|c| c.id).collect();
        let colour_space = match nf {
            1 => ColourSpace::Grey,
            3 => {
                if self.jfif.is_some() {
                    ColourSpace::YCbCr
                } else if let Some(a) = self.adobe {
                    if a.transform == 0 { ColourSpace::Rgb } else { ColourSpace::YCbCr }
                } else if ids == b"RGB" || ids == b"rgb" {
                    ColourSpace::Rgb
                } else {
                    ColourSpace::YCbCr
                }
            }
            4 => {
                if self.adobe.is_some_and(|a| a.transform == 2) {
                    ColourSpace::Ycck
                } else {
                    ColourSpace::Cmyk
                }
            }
            _ => ColourSpace::Other,
        };
        let process = if frame.lossless {
            Process::Lossless
        } else if frame.progressive {
            Process::Progressive
        } else if frame.sof == 0xC0 {
            Process::Baseline
        } else {
            Process::ExtendedSequential
        };
        Ok(Info {
            width: frame.width as u32,
            height: frame.height as u32,
            precision: frame.precision,
            process,
            coding: if frame.arithmetic { Coding::Arithmetic } else { Coding::Huffman },
            sof_marker: frame.sof,
            components: frame
                .comps
                .iter()
                .map(|c| Component {
                    id: c.id,
                    h: c.h as u8,
                    v: c.v as u8,
                    quant_table: c.tq as u8,
                    width: c.width as u32,
                    height: c.height as u32,
                })
                .collect(),
            colour_space,
            restart_interval: self.first_restart_interval.unwrap_or(self.restart_interval),
            scans: self.scans,
            jfif: self.jfif,
            adobe: self.adobe,
            orientation: self.exif.as_deref().and_then(exif_orientation),
            exif: self.exif.clone(),
            icc_profile: icc,
            xmp: self.xmp.clone(),
            comments: self.comments.clone(),
        })
    }

    fn finish(mut self) -> Result<Image> {
        let frame = self.frame.clone().ok_or_else(|| invalid("no frame header"))?;
        if frame.progressive {
            scan::idct_all(&mut self, &frame);
        }
        if self.opts.strict && !self.complete {
            return Err(invalid(self.warnings.first().cloned().unwrap_or_else(|| "incomplete".into())));
        }
        let info = self.info()?;
        let planes = std::mem::take(&mut self.planes)
            .into_iter()
            .zip(&frame.comps)
            .map(|(data, c)| Plane { width: c.width, height: c.height, stride: c.stride, data })
            .collect();
        Ok(Image { info, planes, complete: self.complete, warnings: self.warnings })
    }
}
