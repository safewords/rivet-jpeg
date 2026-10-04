//! From decoded planes to pixels: upsampling a subsampled component to the
//! frame's resolution, and the colour transforms.
//!
//! Upsampling interpolates linearly between the samples of a component,
//! with each sample sited at the centre of the full-resolution pixels it
//! covers (T.871 6.1 places chroma samples centred between luma samples).
//! It works for any ratio of sampling factors, integral or not; a component
//! at full resolution is copied.
//!
//! YCbCr to RGB is T.871 clause 7, at the frame's precision (centred on
//! 2^(P-1)). CMYK and YCCK are converted to RGB naively (no colour
//! management): R = (1 - C)(1 - K), and so on.

use crate::decode::{ColourSpace, Image, Plane, Process};

/// Linear interpolation weights for one axis: for each output position, two
/// source indices and their weights over `denom`.
struct Axis {
    taps: Vec<(u32, u32, u32, u32)>,
    denom: u32,
    /// Whether every output position but the first and last is the 2:1
    /// pattern: x = 2k + 1 takes samples k and k + 1 weighted 3:1, x = 2k + 2
    /// takes them 1:3 (over a denominator of 4).
    halves: bool,
}

impl Axis {
    fn new(out_len: usize, src_len: usize, factor: usize, max: usize) -> Self {
        let denom = (2 * max) as u32;
        let last = src_len.saturating_sub(1) as i64;
        let taps = (0..out_len)
            .map(|x| {
                // Source position of output x, in units of 1/denom:
                // (x + 1/2) * factor / max - 1/2.
                let num = (2 * x as i64 + 1) * factor as i64 - max as i64;
                if num <= 0 {
                    return (0, 0, denom, 0);
                }
                let i0 = (num / i64::from(denom)).min(last);
                let frac = (num % i64::from(denom)) as u32;
                let i1 = (i0 + 1).min(last);
                (i0 as u32, i1 as u32, denom - frac, frac)
            })
            .collect::<Vec<_>>();
        let halves = denom == 4
            && taps.len() >= 3
            && taps[1..taps.len() - 1].iter().enumerate().all(|(i, &t)| {
                let x = i + 1;
                let k = ((x - 1) / 2) as u32;
                t == if x % 2 == 1 { (k, k + 1, 3, 1) } else { (k, k + 1, 1, 3) }
            });
        Self { taps, denom, halves }
    }
}

/// Upsamples the planes of an image one line at a time.
pub(crate) struct Rows<'a> {
    planes: &'a [Plane],
    axes: Vec<(Axis, Axis)>,
    identity: Vec<bool>,
    lines: Vec<Vec<u16>>,
    /// One component line after the vertical pass, before the horizontal.
    cols: Vec<u32>,
    width: usize,
}

impl<'a> Rows<'a> {
    pub(crate) fn new(img: &'a Image) -> Self {
        let info = &img.info;
        let (w, h) = (info.width as usize, info.height as usize);
        let hmax = info.components.iter().map(|c| usize::from(c.h)).max().unwrap_or(1);
        let vmax = info.components.iter().map(|c| usize::from(c.v)).max().unwrap_or(1);
        let mut axes = Vec::new();
        let mut identity = Vec::new();
        for (c, p) in info.components.iter().zip(&img.planes) {
            let (ch, cv) = (usize::from(c.h), usize::from(c.v));
            identity.push(ch == hmax && cv == vmax);
            axes.push((Axis::new(w, p.width, ch, hmax), Axis::new(h, p.height, cv, vmax)));
        }
        let cols = vec![0; img.planes.iter().map(|p| p.width).max().unwrap_or(0)];
        Self { planes: &img.planes, axes, identity, lines: vec![vec![0; w]; img.planes.len()], cols, width: w }
    }

    /// Line `y` of every component, at full resolution.
    ///
    /// Separably: the two source lines are blended first (`v0 * r0 + v1 *
    /// r1` for every sample), then the blended line horizontally, which is
    /// the same integer as blending each output's four samples at once, so
    /// the result is the same.
    pub(crate) fn line(&mut self, y: usize) -> &[Vec<u16>] {
        for (i, p) in self.planes.iter().enumerate() {
            let out = &mut self.lines[i];
            if self.identity[i] {
                out.copy_from_slice(&p.data[y * p.stride..y * p.stride + self.width]);
                continue;
            }
            let (ax, ay) = &self.axes[i];
            let (j0, j1, v0, v1) = ay.taps[y];
            let r0 = &p.data[j0 as usize * p.stride..][..p.width];
            let r1 = &p.data[j1 as usize * p.stride..][..p.width];
            let cols = &mut self.cols[..p.width];
            crate::simd::with_wide_vectors(|| {
                for ((c, &a), &b) in cols.iter_mut().zip(r0).zip(r1) {
                    *c = u32::from(a) * v0 + u32::from(b) * v1;
                }
            });
            let d = ax.denom * ay.denom;
            horizontal(ax, cols, d, out);
        }
        &self.lines
    }
}

/// The horizontal pass of [`Rows::line`]: `(c[i0] * w0 + c[i1] * w1 + d / 2)
/// / d` for each output. The largest value, (2^16 - 1) * 8 * 8, fits in 32
/// bits.
fn horizontal(ax: &Axis, cols: &[u32], d: u32, out: &mut [u16]) {
    let half = d / 2;
    let generic = |out: &mut [u16], taps: &[(u32, u32, u32, u32)]| {
        for (o, &(i0, i1, w0, w1)) in out.iter_mut().zip(taps) {
            *o = ((cols[i0 as usize] * w0 + cols[i1 as usize] * w1 + half) / d) as u16;
        }
    };
    let n = out.len();
    if !ax.halves || !d.is_power_of_two() {
        generic(out, &ax.taps);
        return;
    }
    let shift = d.trailing_zeros();
    generic(&mut out[..1], &ax.taps[..1]);
    generic(&mut out[n - 1..], &ax.taps[n - 1..]);
    // Outputs 1 .. n-1 in pairs from samples k and k + 1.
    let inner = &mut out[1..n - 1];
    crate::simd::with_wide_vectors(|| {
        let (pairs, odd) = inner.as_chunks_mut::<2>();
        for (o, w) in pairs.iter_mut().zip(cols.windows(2)) {
            let (a, b) = (w[0], w[1]);
            o[0] = ((3 * a + b + half) >> shift) as u16;
            o[1] = ((a + 3 * b + half) >> shift) as u16;
        }
        if let Some(last) = odd.first_mut() {
            let k = pairs.len();
            *last = ((3 * cols[k] + cols[k + 1] + half) >> shift) as u16;
        }
    });
}

/// How to turn a line of components into RGB.
#[derive(Clone, Copy)]
enum Transform {
    Grey,
    YCbCr,
    Rgb,
    /// CMYK; `inverted` when stored as Adobe applications store it.
    Cmyk { inverted: bool },
    Ycck { inverted: bool },
}

fn transform(img: &Image) -> Transform {
    let adobe = img.info.adobe.is_some();
    match img.info.colour_space {
        ColourSpace::Grey | ColourSpace::Other => Transform::Grey,
        ColourSpace::YCbCr if img.info.process == Process::Lossless && img.info.jfif.is_none() && !adobe => {
            Transform::Rgb
        }
        ColourSpace::YCbCr => Transform::YCbCr,
        ColourSpace::Rgb => Transform::Rgb,
        ColourSpace::Cmyk => Transform::Cmyk { inverted: adobe },
        ColourSpace::Ycck => Transform::Ycck { inverted: adobe },
    }
}

/// T.871 clause 7 in 32-bit arithmetic, for precisions up to 12 bits
/// (where `y * 2^16 + 116130 * 2048` stays below 2^31): the same integers
/// as [`ycc_to_rgb`].
#[inline(always)]
fn ycc_to_rgb32(y: i32, cb: i32, cr: i32, centre: i32, max: i32) -> [i32; 3] {
    const ONE: i32 = 1 << 16;
    let (cb, cr) = (cb - centre, cr - centre);
    let r = y * ONE + 91881 * cr;
    let g = y * ONE - 22554 * cb - 46802 * cr;
    let b = y * ONE + 116130 * cb;
    let round = |v: i32| ((v + ONE / 2) >> 16).clamp(0, max);
    [round(r), round(g), round(b)]
}

/// T.871 clause 7, in fixed point (16 fraction bits), at any precision.
#[inline]
fn ycc_to_rgb(y: i64, cb: i64, cr: i64, centre: i64, max: i64) -> [i64; 3] {
    const ONE: i64 = 1 << 16;
    let (cb, cr) = (cb - centre, cr - centre);
    let r = y * ONE + 91881 * cr; // 1.402
    let g = y * ONE - 22554 * cb - 46802 * cr; // 0.344136, 0.714136
    let b = y * ONE + 116130 * cb; // 1.772
    let round = |v: i64| ((v + ONE / 2) >> 16).clamp(0, max);
    [round(r), round(g), round(b)]
}

impl Image {
    /// Component `index` upsampled to the frame's full resolution, `width x
    /// height` samples at the frame's precision. For a 4:4:4 picture this
    /// is the plane itself, without its padding.
    pub fn upsampled(&self, index: usize) -> Option<Vec<u16>> {
        if index >= self.planes.len() {
            return None;
        }
        let (w, h) = (self.info.width as usize, self.info.height as usize);
        let mut rows = Rows::new(self);
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            out.extend_from_slice(&rows.line(y)[index]);
        }
        Some(out)
    }

    /// RGB at the frame's precision, `width x height x 3`, through `f`.
    // The loops index several component lines at once.
    #[allow(clippy::needless_range_loop)]
    fn rgb_lines(&self, f: impl FnMut(usize, &[[u16; 3]])) {
        self.rgb_band(0..self.info.height as usize, f);
    }

    /// [`rgb_lines`](Self::rgb_lines) for lines `ys` only.
    // The loops index several component lines at once.
    #[allow(clippy::needless_range_loop)]
    fn rgb_band(&self, ys: std::ops::Range<usize>, mut f: impl FnMut(usize, &[[u16; 3]])) {
        let w = self.info.width as usize;
        let max = (1i64 << self.info.precision) - 1;
        let centre = 1i64 << (self.info.precision - 1);
        let t = transform(self);
        let mut rows = Rows::new(self);
        let mut rgb = vec![[0u16; 3]; w];
        for y in ys {
            let l = rows.line(y);
            match t {
                Transform::Grey => {
                    for (o, &v) in rgb.iter_mut().zip(&l[0]) {
                        *o = [v; 3];
                    }
                }
                Transform::Rgb => {
                    for x in 0..w {
                        rgb[x] = [l[0][x], l[1][x], l[2][x]];
                    }
                }
                Transform::YCbCr if self.info.precision <= 12 => {
                    let (c, m) = (centre as i32, max as i32);
                    crate::simd::with_wide_vectors(|| {
                        for (((o, &y), &cb), &cr) in rgb.iter_mut().zip(&l[0]).zip(&l[1]).zip(&l[2]) {
                            let v = ycc_to_rgb32(i32::from(y), i32::from(cb), i32::from(cr), c, m);
                            *o = v.map(|v| v as u16);
                        }
                    });
                }
                Transform::YCbCr => {
                    for x in 0..w {
                        let c = ycc_to_rgb(i64::from(l[0][x]), i64::from(l[1][x]), i64::from(l[2][x]), centre, max);
                        rgb[x] = c.map(|v| v as u16);
                    }
                }
                Transform::Cmyk { inverted } | Transform::Ycck { inverted } => {
                    let ycck = matches!(t, Transform::Ycck { .. });
                    for x in 0..w {
                        let ink = cmyk_ink(l, x, ycck, inverted, centre, max);
                        let k = max - ink[3];
                        rgb[x] = [0, 1, 2].map(|i| (((max - ink[i]) * k + max / 2) / max) as u16);
                    }
                }
            }
            f(y, &rgb);
        }
    }

    /// 8-bit RGB, `width * height * 3` bytes, converted from whatever the
    /// components are (grey is replicated; CMYK and YCCK are converted
    /// naively). 12- and 16-bit samples are scaled to 8 bits.
    pub fn to_rgb8(&self) -> Vec<u8> {
        self.to_8bit::<3>()
    }

    /// 8-bit RGB (`N` = 3) or RGBA with opaque alpha (`N` = 4), converted
    /// in bands of lines on several threads (each line's pixels depend only
    /// on the planes, so the result is the same).
    fn to_8bit<const N: usize>(&self) -> Vec<u8> {
        let (w, h) = (self.info.width as usize, self.info.height as usize);
        let mut out = vec![0u8; w * h * N];
        let scale = Scale::new(self.info.precision, 8);
        crate::par::bands(&mut out, w * N, 0, |ys, band| {
            let start = ys.start;
            self.rgb_band(ys, |y, line| {
                let o = &mut band[(y - start) * w * N..(y - start + 1) * w * N];
                if scale.from_max == scale.to_max {
                    for (o, p) in o.as_chunks_mut::<N>().0.iter_mut().zip(line) {
                        o[..3].copy_from_slice(&p.map(|v| v as u8));
                        if N == 4 {
                            o[3] = u8::MAX;
                        }
                    }
                } else {
                    for (o, p) in o.as_chunks_mut::<N>().0.iter_mut().zip(line) {
                        o[..3].copy_from_slice(&p.map(|v| scale.apply(v) as u8));
                        if N == 4 {
                            o[3] = u8::MAX;
                        }
                    }
                }
            });
        });
        out
    }

    /// 8-bit RGBA with opaque alpha, `width * height * 4` bytes.
    pub fn to_rgba8(&self) -> Vec<u8> {
        self.to_8bit::<4>()
    }

    /// 16-bit RGB, `width * height * 3` values scaled to the full 0–65535
    /// range, for 12- and 16-bit pictures.
    pub fn to_rgb16(&self) -> Vec<u16> {
        let (w, h) = (self.info.width as usize, self.info.height as usize);
        let mut out = Vec::with_capacity(w * h * 3);
        let scale = Scale::new(self.info.precision, 16);
        self.rgb_lines(|_, line| {
            for p in line {
                out.extend(p.iter().map(|&v| scale.apply(v) as u16));
            }
        });
        out
    }

    /// 8-bit grey, `width * height` bytes: the luma component of a YCbCr
    /// picture, the only component of a grey one, and BT.601 luma of RGB
    /// otherwise.
    pub fn to_luma8(&self) -> Vec<u8> {
        let (w, h) = (self.info.width as usize, self.info.height as usize);
        let scale = Scale::new(self.info.precision, 8);
        match transform(self) {
            Transform::Grey | Transform::YCbCr => {
                let mut out = Vec::with_capacity(w * h);
                let p = &self.planes[0];
                for y in 0..h {
                    out.extend(p.row(y).iter().map(|&v| scale.apply(v) as u8));
                }
                out
            }
            _ => {
                let mut out = Vec::with_capacity(w * h);
                self.rgb_lines(|_, line| {
                    for p in line {
                        let l = (19595 * u64::from(p[0]) + 38470 * u64::from(p[1]) + 7471 * u64::from(p[2]) + 32768) >> 16;
                        out.push(scale.apply(l as u16) as u8);
                    }
                });
                out
            }
        }
    }

    /// 8-bit CMYK ink amounts (0 = no ink), `width * height * 4` bytes, for
    /// a CMYK or YCCK picture: YCCK is transformed back and Adobe's
    /// inverted storage is undone. `None` for other colour spaces.
    pub fn to_cmyk8(&self) -> Option<Vec<u8>> {
        let (w, h) = (self.info.width as usize, self.info.height as usize);
        let (ycck, inverted) = match transform(self) {
            Transform::Cmyk { inverted } => (false, inverted),
            Transform::Ycck { inverted } => (true, inverted),
            _ => return None,
        };
        let max = (1i64 << self.info.precision) - 1;
        let centre = 1i64 << (self.info.precision - 1);
        let scale = Scale::new(self.info.precision, 8);
        let mut rows = Rows::new(self);
        let mut out = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            let l = rows.line(y);
            for x in 0..w {
                let ink = cmyk_ink(l, x, ycck, inverted, centre, max);
                out.extend(ink.map(|v| scale.apply(v as u16) as u8));
            }
        }
        Some(out)
    }
}

/// Ink amounts (0 = none) of one CMYK or YCCK pixel. YCCK: the first three
/// components are YCbCr of the complemented CMY values, K as stored; the
/// result is then in the same convention as an untransformed CMYK file from
/// the same writer (inverted, when there is an Adobe segment).
#[inline]
fn cmyk_ink(l: &[Vec<u16>], x: usize, ycck: bool, inverted: bool, centre: i64, max: i64) -> [i64; 4] {
    let mut v = [i64::from(l[0][x]), i64::from(l[1][x]), i64::from(l[2][x]), i64::from(l[3][x])];
    if ycck {
        let rgb = ycc_to_rgb(v[0], v[1], v[2], centre, max);
        v[0] = max - rgb[0];
        v[1] = max - rgb[1];
        v[2] = max - rgb[2];
    }
    if inverted {
        v = v.map(|c| max - c);
    }
    v
}

/// Rescales samples from one precision to another, rounding.
#[derive(Clone, Copy)]
struct Scale {
    from_max: u32,
    to_max: u32,
}

impl Scale {
    fn new(from_bits: u8, to_bits: u8) -> Self {
        Self { from_max: (1u32 << from_bits) - 1, to_max: (1u32 << to_bits) - 1 }
    }

    #[inline]
    fn apply(self, v: u16) -> u32 {
        if self.from_max == self.to_max {
            return u32::from(v);
        }
        ((u64::from(v) * u64::from(self.to_max) + u64::from(self.from_max / 2)) / u64::from(self.from_max)) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The upsampling as first written: both axes' weights at once, per
    /// output sample, in 64-bit arithmetic.
    fn reference_line(img: &Image, y: usize) -> Vec<Vec<u16>> {
        let info = &img.info;
        let (w, h) = (info.width as usize, info.height as usize);
        let hmax = info.components.iter().map(|c| usize::from(c.h)).max().unwrap_or(1);
        let vmax = info.components.iter().map(|c| usize::from(c.v)).max().unwrap_or(1);
        let mut out = Vec::new();
        for (c, p) in info.components.iter().zip(&img.planes) {
            let (ch, cv) = (usize::from(c.h), usize::from(c.v));
            if ch == hmax && cv == vmax {
                out.push(p.data[y * p.stride..y * p.stride + w].to_vec());
                continue;
            }
            let (ax, ay) = (Axis::new(w, p.width, ch, hmax), Axis::new(h, p.height, cv, vmax));
            let (j0, j1, v0, v1) = ay.taps[y];
            let r0 = &p.data[j0 as usize * p.stride..];
            let r1 = &p.data[j1 as usize * p.stride..];
            let d = u64::from(ax.denom * ay.denom);
            out.push(
                ax.taps
                    .iter()
                    .map(|&(i0, i1, w0, w1)| {
                        let (i0, i1) = (i0 as usize, i1 as usize);
                        let top = u64::from(r0[i0]) * u64::from(w0) + u64::from(r0[i1]) * u64::from(w1);
                        let bottom = u64::from(r1[i0]) * u64::from(w0) + u64::from(r1[i1]) * u64::from(w1);
                        ((top * u64::from(v0) + bottom * u64::from(v1) + d / 2) / d) as u16
                    })
                    .collect(),
            );
        }
        out
    }

    #[test]
    fn upsampling_matches_the_reference() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("corpus");
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .chain(std::fs::read_dir(dir.join("gen")).unwrap())
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "jpg"))
            .collect();
        files.sort();
        let mut seed = 0x0123_4567_89AB_CDEFu64;
        for f in &files {
            let mut img = crate::decode(&std::fs::read(f).unwrap()).unwrap();
            for pass in 0..2 {
                if pass == 1 {
                    // Random samples across the whole range, the extremes
                    // included.
                    let max = (1u32 << img.info.precision) - 1;
                    for p in &mut img.planes {
                        for v in &mut p.data {
                            seed ^= seed << 13;
                            seed ^= seed >> 7;
                            seed ^= seed << 17;
                            *v = match seed % 5 {
                                0 => 0,
                                1 => max,
                                _ => (seed >> 20) as u32 % (max + 1),
                            } as u16;
                        }
                    }
                }
                let mut rows = Rows::new(&img);
                for y in 0..img.info.height as usize {
                    assert_eq!(rows.line(y), &reference_line(&img, y)[..], "{} line {y}", f.display());
                }
            }
        }
    }
}
