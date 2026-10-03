//! The encoder: 8-bit RGB, RGBA or grey to baseline sequential or
//! progressive JPEG (and, on request, the arithmetic-coded processes).

mod arith;
mod huff;

use crate::dct::fdct;
use crate::error::{Result, config};
use crate::huffman::TableSpec;
use crate::metadata::{EXIF_MAX, ICC_CHUNK};
use crate::tables::{
    CHROMA_AC_BITS, CHROMA_AC_VALUES, CHROMA_DC_BITS, CHROMA_DC_VALUES, CHROMA_QUANT, LUMA_AC_BITS, LUMA_AC_VALUES,
    LUMA_DC_BITS, LUMA_DC_VALUES, LUMA_QUANT, ZIGZAG,
};

/// The layout of the pixels given to [`encode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// R, G, B, one byte each.
    Rgb,
    /// R, G, B, A, one byte each. Alpha is ignored (JPEG has none): flatten
    /// a transparent picture onto its background first.
    Rgba,
    /// One byte of grey per pixel; written as a one-component frame.
    Luma,
}

impl PixelFormat {
    fn bytes(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Rgba => 4,
            Self::Luma => 1,
        }
    }
}

/// Chroma subsampling: the luma sampling factors, the chroma ones being 1x1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Subsampling {
    /// 4:4:4, luma 1x1: chroma at full resolution.
    S444,
    /// 4:2:2, luma 2x1: chroma halved horizontally.
    S422,
    /// 4:2:0, luma 2x2: chroma halved both ways.
    S420,
    /// 4:4:0, luma 1x2: chroma halved vertically.
    S440,
}

impl Subsampling {
    fn luma_factors(self) -> (usize, usize) {
        match self {
            Self::S444 => (1, 1),
            Self::S422 => (2, 1),
            Self::S420 => (2, 2),
            Self::S440 => (1, 2),
        }
    }
}

/// How to encode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodeSettings {
    /// 1–100. Scales the Annex K example tables: below 50 by 50/quality,
    /// above by (100 - quality)/50, so 50 is Annex K itself, 100 is all 1s.
    pub quality: u8,
    /// Chroma subsampling, for colour input.
    pub subsampling: Subsampling,
    /// Progressive (SOF2): spectral selection and successive approximation
    /// in ten scans (six for grey). Implies optimised Huffman tables, one
    /// set per scan.
    pub progressive: bool,
    /// Huffman tables built for the picture (Annex K.2) instead of the
    /// Annex K examples: smaller files, a second pass over the
    /// coefficients.
    pub optimize_huffman: bool,
    /// A restart marker every this many MCUs (0: none).
    pub restart_interval: u16,
    /// Write a JFIF APP0 segment (T.871) first. On by default.
    pub jfif: bool,
    /// An ICC profile to embed, in as many APP2 chunks as it needs.
    pub icc_profile: Option<Vec<u8>>,
    /// EXIF to embed in APP1: the TIFF structure, with or without the
    /// `Exif\0\0` prefix. At most 65527 bytes.
    pub exif: Option<Vec<u8>>,
    /// Arithmetic coding (SOF9, or SOF10 when progressive) instead of
    /// Huffman. Valid T.81, but many decoders do not implement it; for
    /// testing decoders, not for publishing.
    pub arithmetic: bool,
}

impl Default for EncodeSettings {
    fn default() -> Self {
        Self {
            quality: 85,
            subsampling: Subsampling::S420,
            progressive: false,
            optimize_huffman: true,
            restart_interval: 0,
            jfif: true,
            icc_profile: None,
            exif: None,
            arithmetic: false,
        }
    }
}

/// The quantisation table for `quality`: `base` scaled and clamped to
/// 1–255 (so that it fits 8-bit precision and the baseline process).
pub(crate) fn scaled_table(base: &[u16; 64], quality: u8) -> [u16; 64] {
    let q = u32::from(quality.clamp(1, 100));
    let scale = if q < 50 { 5000 / q } else { 200 - 2 * q };
    base.map(|b| ((u32::from(b) * scale + 50) / 100).clamp(1, 255) as u16)
}

/// One component's quantised coefficients, 64 per block in zig-zag order,
/// blocks covering whole MCUs.
pub(crate) struct CompCoefs {
    pub(crate) h: usize,
    pub(crate) v: usize,
    pub(crate) blocks_w: usize,
    /// Blocks covering the component alone (non-interleaved scans).
    pub(crate) units_w: usize,
    pub(crate) units_h: usize,
    pub(crate) coefs: Vec<[i16; 64]>,
    /// Table slot: 0 luma, 1 chroma.
    pub(crate) table: usize,
}

impl CompCoefs {
    pub(crate) fn block(&self, bx: usize, by: usize) -> &[i16; 64] {
        &self.coefs[by * self.blocks_w + bx]
    }
}

/// A scan of the encoder's progression.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScanSpec {
    /// Bit mask of components.
    pub(crate) comps: u8,
    pub(crate) ss: usize,
    pub(crate) se: usize,
    pub(crate) ah: u8,
    pub(crate) al: u8,
}

/// Geometry shared by the entropy coders.
pub(crate) struct Layout {
    pub(crate) mcus_x: usize,
    pub(crate) mcus_y: usize,
    pub(crate) restart: usize,
}

/// Encode `pixels` (`width * height` of `format`, row by row) as a JPEG
/// file.
pub fn encode(pixels: &[u8], width: u32, height: u32, format: PixelFormat, settings: &EncodeSettings) -> Result<Vec<u8>> {
    if width == 0 || height == 0 || width > 65535 || height > 65535 {
        return Err(config(format!("a JPEG frame is 1 to 65535 pixels each way, not {width}x{height}")));
    }
    if !(1..=100).contains(&settings.quality) {
        return Err(config(format!("quality {} is outside 1-100", settings.quality)));
    }
    let (w, h) = (width as usize, height as usize);
    let need = w * h * format.bytes();
    if pixels.len() != need {
        return Err(config(format!("{} bytes of pixels for {w}x{h} {format:?}; expected {need}", pixels.len())));
    }
    let exif = settings.exif.as_deref().map(|e| e.strip_prefix(b"Exif\0\0").unwrap_or(e));
    if let Some(e) = exif
        && e.len() > EXIF_MAX
    {
        return Err(config(format!("EXIF of {} bytes; one APP1 segment holds {EXIF_MAX}", e.len())));
    }
    if let Some(p) = &settings.icc_profile
        && p.len().div_ceil(ICC_CHUNK) > 255
    {
        return Err(config("an ICC profile larger than 255 APP2 chunks"));
    }

    let grey = format == PixelFormat::Luma;
    let (hy, vy) = if grey { (1, 1) } else { settings.subsampling.luma_factors() };
    let mcus_x = w.div_ceil(8 * hy);
    let mcus_y = h.div_ceil(8 * vy);
    let qtables = [scaled_table(&LUMA_QUANT, settings.quality), scaled_table(&CHROMA_QUANT, settings.quality)];
    let comps = prepare(pixels, w, h, format, hy, vy, mcus_x, mcus_y, &qtables);
    let layout = Layout { mcus_x, mcus_y, restart: usize::from(settings.restart_interval) };

    let mut out = Vec::with_capacity(w * h / 4 + 1024);
    out.extend_from_slice(&[0xFF, 0xD8]);
    if settings.jfif {
        segment(&mut out, 0xE0, &[b'J', b'F', b'I', b'F', 0, 1, 2, 0, 0, 1, 0, 1, 0, 0]);
    }
    if let Some(e) = exif {
        let mut p = b"Exif\0\0".to_vec();
        p.extend_from_slice(e);
        segment(&mut out, 0xE1, &p);
    }
    if let Some(icc) = &settings.icc_profile {
        let n = icc.len().div_ceil(ICC_CHUNK).max(1);
        for (i, chunk) in icc.chunks(ICC_CHUNK).enumerate() {
            let mut p = b"ICC_PROFILE\0".to_vec();
            p.push(i as u8 + 1);
            p.push(n as u8);
            p.extend_from_slice(chunk);
            segment(&mut out, 0xE2, &p);
        }
    }
    // DQT: both tables (one for grey), 8-bit, zig-zag order.
    let mut dqt = Vec::new();
    for (t, q) in qtables.iter().enumerate().take(if grey { 1 } else { 2 }) {
        dqt.push(t as u8);
        dqt.extend(ZIGZAG.iter().map(|&n| q[n] as u8));
    }
    segment(&mut out, 0xDB, &dqt);
    // SOF.
    let sof = match (settings.progressive, settings.arithmetic) {
        (false, false) => 0xC0,
        (true, false) => 0xC2,
        (false, true) => 0xC9,
        (true, true) => 0xCA,
    };
    let mut f = vec![8u8];
    f.extend_from_slice(&(height as u16).to_be_bytes());
    f.extend_from_slice(&(width as u16).to_be_bytes());
    f.push(comps.len() as u8);
    for (i, c) in comps.iter().enumerate() {
        f.extend_from_slice(&[i as u8 + 1, ((c.h << 4) | c.v) as u8, c.table as u8]);
    }
    segment(&mut out, sof, &f);
    if layout.restart > 0 {
        segment(&mut out, 0xDD, &settings.restart_interval.to_be_bytes());
    }

    let scans: Vec<ScanSpec> = if settings.progressive {
        progression(comps.len())
    } else {
        vec![ScanSpec { comps: if grey { 1 } else { 7 }, ss: 0, se: 63, ah: 0, al: 0 }]
    };
    for scan in &scans {
        if settings.arithmetic {
            sos(&mut out, &comps, scan);
            arith::encode_scan(&mut out, &comps, &layout, scan, settings.progressive);
        } else {
            let optimise = settings.optimize_huffman || settings.progressive;
            let tables = if optimise {
                huff::optimal_tables(&comps, &layout, scan, settings.progressive)
            } else {
                standard_tables()
            };
            write_dht(&mut out, &comps, scan, &tables);
            sos(&mut out, &comps, scan);
            huff::encode_scan(&mut out, &comps, &layout, scan, &tables, settings.progressive)?;
        }
    }
    out.extend_from_slice(&[0xFF, 0xD9]);
    Ok(out)
}

/// The scans of the progression: DC first at reduced precision, a low AC
/// band of luma early, chroma, the rest of luma, then the refinements.
fn progression(ncomps: usize) -> Vec<ScanSpec> {
    let all = if ncomps == 1 { 1 } else { 7 };
    let mut s = vec![
        ScanSpec { comps: all, ss: 0, se: 0, ah: 0, al: 1 },
        ScanSpec { comps: 1, ss: 1, se: 5, ah: 0, al: 2 },
    ];
    if ncomps > 1 {
        s.push(ScanSpec { comps: 4, ss: 1, se: 63, ah: 0, al: 1 });
        s.push(ScanSpec { comps: 2, ss: 1, se: 63, ah: 0, al: 1 });
    }
    s.push(ScanSpec { comps: 1, ss: 6, se: 63, ah: 0, al: 2 });
    s.push(ScanSpec { comps: 1, ss: 1, se: 63, ah: 2, al: 1 });
    s.push(ScanSpec { comps: all, ss: 0, se: 0, ah: 1, al: 0 });
    if ncomps > 1 {
        s.push(ScanSpec { comps: 4, ss: 1, se: 63, ah: 1, al: 0 });
        s.push(ScanSpec { comps: 2, ss: 1, se: 63, ah: 1, al: 0 });
    }
    s.push(ScanSpec { comps: 1, ss: 1, se: 63, ah: 1, al: 0 });
    s
}

/// The Annex K tables: (DC, AC) for slot 0 (luma) and slot 1 (chroma).
pub(crate) type Tables = [(TableSpec, TableSpec); 2];

fn standard_tables() -> Tables {
    [
        (TableSpec::new(&LUMA_DC_BITS, &LUMA_DC_VALUES), TableSpec::new(&LUMA_AC_BITS, &LUMA_AC_VALUES)),
        (TableSpec::new(&CHROMA_DC_BITS, &CHROMA_DC_VALUES), TableSpec::new(&CHROMA_AC_BITS, &CHROMA_AC_VALUES)),
    ]
}

/// The tables the scan uses, in one DHT segment.
fn write_dht(out: &mut Vec<u8>, comps: &[CompCoefs], scan: &ScanSpec, tables: &Tables) {
    let mut p = Vec::new();
    let mut slots: Vec<usize> =
        comps.iter().enumerate().filter(|(i, _)| scan.comps & (1 << i) != 0).map(|(_, c)| c.table).collect();
    slots.sort_unstable();
    slots.dedup();
    let dc = scan.ss == 0 && scan.ah == 0;
    let ac = scan.se > 0;
    for &slot in &slots {
        if dc {
            push_table(&mut p, 0, slot, &tables[slot].0);
        }
        if ac {
            push_table(&mut p, 1, slot, &tables[slot].1);
        }
    }
    if !p.is_empty() {
        segment(out, 0xC4, &p);
    }
}

fn push_table(p: &mut Vec<u8>, class: u8, slot: usize, t: &TableSpec) {
    p.push((class << 4) | slot as u8);
    p.extend_from_slice(&t.bits);
    p.extend_from_slice(&t.values);
}

fn sos(out: &mut Vec<u8>, comps: &[CompCoefs], scan: &ScanSpec) {
    let mut p = Vec::new();
    let members: Vec<usize> = (0..comps.len()).filter(|i| scan.comps & (1 << i) != 0).collect();
    p.push(members.len() as u8);
    for &i in &members {
        let t = comps[i].table as u8;
        p.push(i as u8 + 1);
        p.push((t << 4) | t);
    }
    p.extend_from_slice(&[scan.ss as u8, scan.se as u8, (scan.ah << 4) | scan.al]);
    segment(out, 0xDA, &p);
}

fn segment(out: &mut Vec<u8>, marker: u8, payload: &[u8]) {
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(payload);
}

/// Colour conversion (T.871 clause 7), edge extension to whole MCUs, box
/// downsampling of chroma, FDCT and quantisation.
#[allow(clippy::too_many_arguments)]
fn prepare(
    pixels: &[u8],
    w: usize,
    h: usize,
    format: PixelFormat,
    hy: usize,
    vy: usize,
    mcus_x: usize,
    mcus_y: usize,
    qtables: &[[u16; 64]; 2],
) -> Vec<CompCoefs> {
    let bpp = format.bytes();
    let (pw, ph) = (mcus_x * 8 * hy, mcus_y * 8 * vy);
    let ncomp = if format == PixelFormat::Luma { 1 } else { 3 };
    // Full-resolution planes, level-shifted, edges replicated.
    let mut full = vec![vec![0f32; pw * ph]; ncomp];
    for y in 0..ph {
        let sy = y.min(h - 1);
        for x in 0..pw {
            let sx = x.min(w - 1);
            let p = &pixels[(sy * w + sx) * bpp..];
            if ncomp == 1 {
                full[0][y * pw + x] = f32::from(p[0]) - 128.0;
            } else {
                let (r, g, b) = (f32::from(p[0]), f32::from(p[1]), f32::from(p[2]));
                full[0][y * pw + x] = 0.299 * r + 0.587 * g + 0.114 * b - 128.0;
                full[1][y * pw + x] = -0.168_735_9 * r - 0.331_264_1 * g + 0.5 * b;
                full[2][y * pw + x] = 0.5 * r - 0.418_687_6 * g - 0.081_312_4 * b;
            }
        }
    }
    let mut out = Vec::with_capacity(ncomp);
    for (ci, plane) in full.into_iter().enumerate() {
        let (ch, cv) = if ci == 0 { (hy, vy) } else { (1, 1) };
        let (fx, fy) = (hy / ch, vy / cv);
        let (cw, chh) = (pw / fx, ph / fy);
        let plane = if fx == 1 && fy == 1 {
            plane
        } else {
            let mut d = vec![0f32; cw * chh];
            let n = (fx * fy) as f32;
            for y in 0..chh {
                for x in 0..cw {
                    let mut s = 0f32;
                    for dy in 0..fy {
                        for dx in 0..fx {
                            s += plane[(y * fy + dy) * pw + x * fx + dx];
                        }
                    }
                    d[y * cw + x] = s / n;
                }
            }
            d
        };
        let table = usize::from(ci > 0);
        let q = &qtables[table];
        let blocks_w = mcus_x * ch;
        let blocks_h = mcus_y * cv;
        let mut coefs = Vec::with_capacity(blocks_w * blocks_h);
        for by in 0..blocks_h {
            for bx in 0..blocks_w {
                let mut s = [0f32; 64];
                for y in 0..8 {
                    let row = (by * 8 + y) * cw + bx * 8;
                    s[y * 8..y * 8 + 8].copy_from_slice(&plane[row..row + 8]);
                }
                let f = fdct(&s);
                let mut z = [0i16; 64];
                for (k, &n) in ZIGZAG.iter().enumerate() {
                    z[k] = (f[n] / f32::from(q[n])).round() as i16;
                }
                coefs.push(z);
            }
        }
        // The component's own extent, as a decoder computes it from the
        // frame header (A.1.1).
        let hmax = hy;
        let vmax = vy;
        let comp_w = (w * ch).div_ceil(hmax);
        let comp_h = (h * cv).div_ceil(vmax);
        out.push(CompCoefs {
            h: ch,
            v: cv,
            blocks_w,
            units_w: comp_w.div_ceil(8),
            units_h: comp_h.div_ceil(8),
            coefs,
            table,
        });
    }
    out
}
