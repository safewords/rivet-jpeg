//! DCT scans: the sequential procedures of F.2 and the progressive ones of
//! G.2, Huffman (F.2.2, G.1.2) and arithmetic (F.2.4, G.1.3), with the
//! restart handling of E.2.4 and F.2.4.4.

use super::{Decoder, Frame, Scan, ScanEnd, Tables};
use crate::arith::{ArithDecoder, Context};
use crate::bits::BitReader;
use crate::dct::idct_to_samples;
use crate::error::{Result, invalid};
use crate::huffman::DecodeTable;
use crate::tables::ZIGZAG;

/// What a restart marker search found.
pub(super) enum Restart {
    /// The expected marker, at `pos` (the byte after it).
    Found { pos: usize },
    /// A restart marker, but `skipped` intervals later than expected.
    Skipped { pos: usize, skipped: usize },
    /// No restart marker: another marker, or the end of the data.
    End,
}

/// Look for RST`expected` from `pos`, as F.2.4.4 suggests: stepping over
/// anything that is not a marker, and using the marker's number to tell how
/// many intervals were lost.
pub(super) fn find_restart(data: &[u8], mut pos: usize, expected: u8) -> (Restart, bool) {
    let start = pos;
    loop {
        while pos < data.len() && data[pos] != 0xFF {
            pos += 1;
        }
        while pos + 1 < data.len() && data[pos + 1] == 0xFF {
            pos += 1;
        }
        if pos + 1 >= data.len() {
            return (Restart::End, true);
        }
        let m = data[pos + 1];
        if m == 0 {
            pos += 2;
            continue;
        }
        let clean = data[start..pos].iter().all(|&b| b == 0xFF);
        if (0xD0..=0xD7).contains(&m) {
            let n = m - 0xD0;
            let skipped = usize::from((n + 8 - expected) % 8);
            if skipped == 0 {
                return (Restart::Found { pos: pos + 2 }, clean);
            }
            return (
                Restart::Skipped {
                    pos: pos + 2,
                    skipped,
                },
                false,
            );
        }
        return (Restart::End, clean);
    }
}

enum Entropy<'a> {
    Huff(HuffState<'a>),
    Arith(Box<ArithState<'a>>),
}

struct HuffState<'a> {
    r: BitReader<'a>,
    dc: Vec<Option<&'a DecodeTable>>,
    ac: Vec<Option<&'a DecodeTable>>,
    eobrun: u32,
}

struct ArithState<'a> {
    d: ArithDecoder<'a>,
    dc_stats: [[Context; 64]; 4],
    ac_stats: [[Context; 256]; 4],
    /// Da: the DC difference last coded, per scan component.
    da: Vec<i32>,
}

/// The data-unit geometry of a scan.
struct Geometry {
    mcus_x: usize,
    mcus_y: usize,
    single: bool,
}

pub(super) fn decode(dec: &mut Decoder<'_>, frame: &Frame, scan: &Scan) -> Result<ScanEnd> {
    let single = scan.comps.len() == 1;
    let geo = if single {
        let c = &frame.comps[scan.comps[0].ci];
        Geometry {
            mcus_x: c.units_w,
            mcus_y: c.units_h,
            single,
        }
    } else {
        Geometry {
            mcus_x: frame.mcus_x,
            mcus_y: frame.mcus_y,
            single,
        }
    };
    let data = dec.data;
    let start = dec.pos;
    let strict = dec.opts.strict;
    let ri = usize::from(dec.restart_interval);
    let total = geo.mcus_x * geo.mcus_y;
    // Restart intervals of a sequential Huffman scan are independent: each
    // starts with fresh predictions at a marker found by its number. Decode
    // them side by side when there are several and the scan is big enough.
    if !frame.arithmetic && !frame.progressive && ri > 0 && total > ri {
        let threads = crate::par::threads(dec.opts.threads);
        let per_mcu: usize = if single {
            1
        } else {
            scan.comps
                .iter()
                .map(|c| frame.comps[c.ci].h * frame.comps[c.ci].v)
                .sum()
        };
        if threads > 1
            && total * per_mcu >= PARALLEL_MIN_BLOCKS
            && let Some(pos) = intervals_in_parallel(
                data,
                start,
                &dec.tables,
                frame,
                scan,
                &geo,
                ri,
                threads,
                &mut dec.coefs,
            )
        {
            dec.pos = pos;
            return Ok(ScanEnd::Complete);
        }
    }
    let Decoder {
        tables,
        coefs,
        warnings,
        ..
    } = dec;
    let tables: &Tables = tables;
    let mut ent = if frame.arithmetic {
        Entropy::Arith(Box::new(ArithState {
            d: ArithDecoder::new(data, start),
            dc_stats: [[Context::default(); 64]; 4],
            ac_stats: [[Context::default(); 256]; 4],
            da: vec![0; scan.comps.len()],
        }))
    } else {
        Entropy::Huff(HuffState {
            r: BitReader::new(data, start),
            dc: scan
                .comps
                .iter()
                .map(|c| tables.dc[c.td].as_ref())
                .collect(),
            ac: scan
                .comps
                .iter()
                .map(|c| tables.ac[c.ta].as_ref())
                .collect(),
            eobrun: 0,
        })
    };
    let mut pred = vec![0i32; scan.comps.len()];
    let mut warn = |msg: String| -> Result<()> {
        if strict {
            return Err(invalid(msg));
        }
        if warnings.len() < 64 && !warnings.contains(&msg) {
            warnings.push(msg);
        }
        Ok(())
    };
    let mut next_rst = 0u8;
    let mut block = [0i16; 64];
    let mut m = 0usize;
    let mut end = ScanEnd::Complete;
    'mcus: while m < total {
        if ri > 0 && m > 0 && m.is_multiple_of(ri) {
            // Restart: the entropy decoder stops at the marker; find it.
            let (pos, clean_bits) = match &ent {
                Entropy::Huff(h) => {
                    let (bits, n) = h.r.leftover();
                    (h.r.pos(), n < 8 && bits == (1 << n) - 1)
                }
                Entropy::Arith(a) => (a.d.pos(), true),
            };
            if !clean_bits {
                warn("restart interval: data left over, or padding that is not 1-bits".into())?;
            }
            let (found, clean) = find_restart(data, pos, next_rst);
            if !clean {
                warn(format!("data before restart marker RST{next_rst}"))?;
            }
            let pos = match found {
                Restart::Found { pos } => pos,
                Restart::Skipped { pos, skipped } => {
                    warn(format!(
                        "restart marker RST{next_rst} missing; {skipped} interval(s) lost"
                    ))?;
                    next_rst = (next_rst + skipped as u8) % 8;
                    m += skipped * ri;
                    if m >= total {
                        break 'mcus;
                    }
                    pos
                }
                Restart::End => {
                    warn(format!("the scan ends before restart marker RST{next_rst}"))?;
                    end = ScanEnd::Short;
                    break 'mcus;
                }
            };
            next_rst = (next_rst + 1) % 8;
            pred.iter_mut().for_each(|p| *p = 0);
            match &mut ent {
                Entropy::Huff(h) => {
                    h.r.reset_at(pos);
                    h.eobrun = 0;
                }
                Entropy::Arith(a) => {
                    a.d.reset_at(pos);
                    a.dc_stats = [[Context::default(); 64]; 4];
                    a.ac_stats = [[Context::default(); 256]; 4];
                    a.da.iter_mut().for_each(|d| *d = 0);
                }
            }
        }
        let (mx, my) = (m % geo.mcus_x, m / geo.mcus_x);
        for (si, sc) in scan.comps.iter().enumerate() {
            let fc = &frame.comps[sc.ci];
            let (bh, bv) = if geo.single { (1, 1) } else { (fc.h, fc.v) };
            for v in 0..bv {
                for h in 0..bh {
                    let (bx, by) = (mx * bh + h, my * bv + v);
                    let result = if frame.progressive {
                        let at = (by * fc.blocks_stride + bx) * 64;
                        let blk = &mut coefs[sc.ci][at..at + 64];
                        decode_block(
                            &mut ent,
                            tables,
                            scan,
                            si,
                            sc.td,
                            sc.ta,
                            &mut pred[si],
                            blk,
                            true,
                        )
                    } else {
                        block.fill(0);
                        let r = decode_block(
                            &mut ent,
                            tables,
                            scan,
                            si,
                            sc.td,
                            sc.ta,
                            &mut pred[si],
                            &mut block,
                            false,
                        );
                        if r.is_ok() {
                            // Kept for the transform after the last scan
                            // (`idct_all`), which runs on several threads.
                            let at = (by * fc.blocks_stride + bx) * 64;
                            coefs[sc.ci][at..at + 64].copy_from_slice(&block);
                        }
                        r
                    };
                    if let Err(e) = result {
                        warn(format!("{e}"))?;
                        end = ScanEnd::Short;
                        break 'mcus;
                    }
                }
            }
        }
        let exhausted = match &ent {
            Entropy::Huff(h) => h.r.overrun() && !matches!(h.r.marker(), Some(0xD0..=0xD7)),
            Entropy::Arith(a) => a.d.exhausted(),
        };
        if exhausted {
            warn("the data ends inside a scan".into())?;
            end = ScanEnd::Short;
            break;
        }
        m += 1;
    }
    let pos = match ent {
        Entropy::Huff(h) => {
            if end == ScanEnd::Complete {
                let (bits, n) = h.r.leftover();
                if n >= 8 || bits != (1 << n) - 1 {
                    warn("scan: data left over, or padding that is not 1-bits".into())?;
                }
                if h.r.overrun() {
                    warn("the data ends inside a scan".into())?;
                    end = ScanEnd::Short;
                }
            }
            h.r.pos()
        }
        Entropy::Arith(a) => {
            if a.d.at_eof() && end == ScanEnd::Complete {
                warn("the data ends inside a scan".into())?;
                end = ScanEnd::Short;
            }
            // The coder may stop short of bytes it did not need; the next
            // marker is found by searching.
            let mut p = a.d.pos();
            let d = a.d.data();
            while p + 1 < d.len() && !(d[p] == 0xFF && d[p + 1] != 0 && d[p + 1] != 0xFF) {
                p += 1;
            }
            p.min(d.len())
        }
    };
    dec.pos = pos;
    Ok(end)
}

/// Below this many blocks a scan's restart intervals are decoded on the
/// calling thread: the threads would cost more than they save.
const PARALLEL_MIN_BLOCKS: usize = 4096;

/// The next marker at or after `p`: its code and the position after it.
/// Stuffed bytes (0xFF 0x00) are data, and fill bytes (0xFF 0xFF ...) part
/// of the marker that follows.
fn next_marker(data: &[u8], mut p: usize) -> Option<(u8, usize)> {
    loop {
        p += data.get(p..)?.iter().position(|&b| b == 0xFF)?;
        let mut q = p + 1;
        while data.get(q) == Some(&0xFF) {
            q += 1;
        }
        let m = *data.get(q)?;
        if m != 0 {
            return Some((m, q + 1));
        }
        p = q + 1;
    }
}

/// A sequential Huffman scan with restart intervals, each interval decoded
/// on its own on up to `threads` threads, into `coefs` exactly as the
/// serial loop in [`decode`] would put them, returning where the scan ends.
///
/// Only a scan the serial loop would decode without a word to say is done
/// here: every restart marker present, in sequence, with nothing between
/// an interval's last bits and its marker, no Huffman code that matches
/// nothing, no interval running out of data, the scan padded with 1-bits.
/// Anything else — the cases where the serial loop warns, skips intervals
/// or stops short — is `None`, `coefs` untouched, and the scan is decoded
/// serially, so the picture and the warnings are the same either way.
#[allow(clippy::too_many_arguments)]
fn intervals_in_parallel(
    data: &[u8],
    start: usize,
    tables: &Tables,
    frame: &Frame,
    scan: &Scan,
    geo: &Geometry,
    ri: usize,
    threads: usize,
    coefs: &mut [Vec<i16>],
) -> Option<usize> {
    let total = geo.mcus_x * geo.mcus_y;
    let n = total.div_ceil(ri);
    // Where each interval's data starts: after RST0, RST1, ... in turn.
    let mut starts = Vec::with_capacity(n);
    starts.push(start);
    for k in 0..n - 1 {
        let (m, after) = next_marker(data, starts[k])?;
        if m != 0xD0 + (k % 8) as u8 {
            return None;
        }
        starts.push(after);
    }
    // Blocks per MCU, per scan component.
    let units: Vec<(usize, usize)> = scan
        .comps
        .iter()
        .map(|sc| {
            if geo.single {
                (1, 1)
            } else {
                (frame.comps[sc.ci].h, frame.comps[sc.ci].v)
            }
        })
        .collect();
    let per_mcu: usize = units.iter().map(|&(h, v)| h * v).sum();
    let dc: Vec<Option<&DecodeTable>> = scan
        .comps
        .iter()
        .map(|c| tables.dc[c.td].as_ref())
        .collect();
    let ac: Vec<Option<&DecodeTable>> = scan
        .comps
        .iter()
        .map(|c| tables.ac[c.ta].as_ref())
        .collect();
    let decoded = crate::par::map(n, threads, |k| -> Option<(Vec<i16>, usize)> {
        let mut h = HuffState {
            r: BitReader::new(data, starts[k]),
            dc: dc.clone(),
            ac: ac.clone(),
            eobrun: 0,
        };
        let mut pred = vec![0i32; units.len()];
        let mcus = k * ri..((k + 1) * ri).min(total);
        let mut out = vec![0i16; mcus.len() * per_mcu * 64];
        let mut blocks = out.as_chunks_mut::<64>().0.iter_mut();
        for _ in mcus {
            for (si, &(bh, bv)) in units.iter().enumerate() {
                for _ in 0..bh * bv {
                    let blk = blocks.next().expect("sized for the interval");
                    huff_sequential(&mut h, si, &mut pred[si], blk).ok()?;
                }
            }
            if h.r.overrun() && !matches!(h.r.marker(), Some(0xD0..=0xD7)) {
                return None;
            }
        }
        let (bits, nb) = h.r.leftover();
        if nb >= 8 || bits != (1 << nb) - 1 {
            return None;
        }
        if k + 1 < n {
            match find_restart(data, h.r.pos(), (k % 8) as u8) {
                (Restart::Found { pos }, true) if pos == starts[k + 1] => Some((out, 0)),
                _ => None,
            }
        } else if h.r.overrun() {
            None
        } else {
            Some((out, h.r.pos()))
        }
    });
    let mut end = 0;
    let mut m = 0usize;
    for interval in &decoded {
        let (out, pos) = interval.as_ref()?;
        end = *pos;
        let mut blocks = out.as_chunks::<64>().0.iter();
        for _ in 0..out.len() / (per_mcu * 64) {
            let (mx, my) = (m % geo.mcus_x, m / geo.mcus_x);
            for (sc, &(bh, bv)) in scan.comps.iter().zip(&units) {
                let fc = &frame.comps[sc.ci];
                for v in 0..bv {
                    for hh in 0..bh {
                        let (bx, by) = (mx * bh + hh, my * bv + v);
                        let at = (by * fc.blocks_stride + bx) * 64;
                        coefs[sc.ci][at..at + 64]
                            .copy_from_slice(blocks.next().expect("one per block"));
                    }
                }
            }
            m += 1;
        }
    }
    Some(end)
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn decode_block(
    ent: &mut Entropy<'_>,
    tables: &Tables,
    scan: &Scan,
    si: usize,
    td: usize,
    ta: usize,
    pred: &mut i32,
    blk: &mut [i16],
    progressive: bool,
) -> Result<()> {
    match ent {
        Entropy::Huff(h) => {
            if !progressive {
                huff_sequential(h, si, pred, blk)
            } else if scan.ss == 0 {
                if scan.ah == 0 {
                    let dc = h.dc[si].ok_or_else(|| invalid("no DC table"))?;
                    let s = u32::from(dc.decode(&mut h.r)?);
                    *pred = pred.wrapping_add(h.r.receive_extend(s));
                    blk[0] = (*pred << scan.al) as i16;
                } else if h.r.bit() {
                    blk[0] |= 1 << scan.al;
                }
                Ok(())
            } else if scan.ah == 0 {
                huff_ac_first(h, si, scan, blk)
            } else {
                huff_ac_refine(h, si, scan, blk)
            }
        }
        Entropy::Arith(a) => {
            if !progressive || scan.ss == 0 && scan.ah == 0 {
                let (l, u) = tables.dc_cond[td];
                let diff = a.dc_diff(td, si, l, u)?;
                *pred = pred.wrapping_add(diff);
                blk[0] = (*pred << scan.al) as i16;
                if !progressive {
                    return a.ac_first(ta, 1, 63, 0, tables.ac_kx[ta], blk);
                }
                Ok(())
            } else if scan.ss == 0 {
                if a.d.decode_fixed() {
                    blk[0] |= 1 << scan.al;
                }
                Ok(())
            } else if scan.ah == 0 {
                a.ac_first(ta, scan.ss, scan.se, scan.al, tables.ac_kx[ta], blk)
            } else {
                a.ac_refine(ta, scan.ss, scan.se, scan.al, blk)
            }
        }
    }
}

/// F.2.2: a whole block.
#[inline]
fn huff_sequential(
    h: &mut HuffState<'_>,
    si: usize,
    pred: &mut i32,
    blk: &mut [i16],
) -> Result<()> {
    let dc = h.dc[si].ok_or_else(|| invalid("no DC table"))?;
    let ac = h.ac[si].ok_or_else(|| invalid("no AC table"))?;
    let s = u32::from(dc.decode(&mut h.r)?);
    if s > 15 {
        return Err(invalid(format!("DC magnitude category {s}")));
    }
    *pred = pred.wrapping_add(h.r.receive_extend(s));
    blk[0] = *pred as i16;
    let mut k = 1usize;
    while k < 64 {
        let rs = ac.decode(&mut h.r)?;
        let (r, s) = (usize::from(rs >> 4), u32::from(rs & 15));
        if s == 0 {
            if r != 15 {
                break;
            }
            k += 16;
            continue;
        }
        k += r;
        if k > 63 {
            return Err(invalid("AC coefficients run past the end of the block"));
        }
        blk[k] = h.r.receive_extend(s) as i16;
        k += 1;
    }
    if k > 64 {
        return Err(invalid("AC coefficients run past the end of the block"));
    }
    Ok(())
}

/// G.1.2.2, decoding: the first scan of an AC band.
fn huff_ac_first(h: &mut HuffState<'_>, si: usize, scan: &Scan, blk: &mut [i16]) -> Result<()> {
    if h.eobrun > 0 {
        h.eobrun -= 1;
        return Ok(());
    }
    let ac = h.ac[si].ok_or_else(|| invalid("no AC table"))?;
    let mut k = scan.ss;
    while k <= scan.se {
        let rs = ac.decode(&mut h.r)?;
        let (r, s) = (u32::from(rs >> 4), u32::from(rs & 15));
        if s == 0 {
            if r < 15 {
                h.eobrun = (1 << r) - 1;
                if r > 0 {
                    h.eobrun += h.r.bits(r);
                }
                break;
            }
            k += 16;
            continue;
        }
        k += r as usize;
        if k > scan.se {
            return Err(invalid("AC coefficients run past the end of the band"));
        }
        blk[k] = (h.r.receive_extend(s) << scan.al) as i16;
        k += 1;
    }
    Ok(())
}

/// G.1.2.3, decoding: a refinement scan of an AC band.
fn huff_ac_refine(h: &mut HuffState<'_>, si: usize, scan: &Scan, blk: &mut [i16]) -> Result<()> {
    let p1: i16 = 1 << scan.al;
    let m1: i16 = -p1;
    let mut k = scan.ss;
    let refine = |r: &mut BitReader<'_>, c: &mut i16| {
        if r.bit() && (*c & p1) == 0 {
            *c = if *c >= 0 {
                c.wrapping_add(p1)
            } else {
                c.wrapping_add(m1)
            };
        }
    };
    if h.eobrun == 0 {
        let ac = h.ac[si].ok_or_else(|| invalid("no AC table"))?;
        while k <= scan.se {
            let rs = ac.decode(&mut h.r)?;
            let (mut r, s) = (u32::from(rs >> 4), u32::from(rs & 15));
            let mut val = 0i16;
            if s != 0 {
                if s != 1 {
                    return Err(invalid(format!(
                        "refinement scan with magnitude category {s}"
                    )));
                }
                val = if h.r.bit() { p1 } else { m1 };
            } else if r != 15 {
                h.eobrun = 1 << r;
                if r > 0 {
                    h.eobrun += h.r.bits(r);
                }
                break;
            }
            let mut placed = false;
            while k <= scan.se {
                if blk[k] != 0 {
                    refine(&mut h.r, &mut blk[k]);
                } else {
                    if r == 0 {
                        if val != 0 {
                            blk[k] = val;
                        }
                        k += 1;
                        placed = true;
                        break;
                    }
                    r -= 1;
                }
                k += 1;
            }
            if !placed && (val != 0 || r > 0) {
                return Err(invalid("refinement run past the end of the band"));
            }
        }
    }
    if h.eobrun > 0 {
        while k <= scan.se {
            if blk[k] != 0 {
                refine(&mut h.r, &mut blk[k]);
            }
            k += 1;
        }
        h.eobrun -= 1;
    }
    Ok(())
}

impl ArithState<'_> {
    /// F.2.4.1 and F.2.4.3: a DC difference, conditioned on the last one.
    fn dc_diff(&mut self, t: usize, si: usize, l: u8, u: u8) -> Result<i32> {
        let da = self.da[si];
        let lower = if l == 0 { 0 } else { 1i32 << (l - 1) };
        let upper = 1i32 << u;
        let a = da.abs();
        let s0 = if a <= lower {
            0
        } else if a <= upper {
            if da > 0 { 4 } else { 8 }
        } else if da > 0 {
            12
        } else {
            16
        };
        let st = &mut self.dc_stats[t];
        if !self.d.decode(&mut st[s0]) {
            self.da[si] = 0;
            return Ok(0);
        }
        let neg = self.d.decode(&mut st[s0 + 1]);
        let mut s = s0 + 2 + usize::from(neg);
        let mut m: i32 = 1;
        if self.d.decode(&mut st[s]) {
            s = 20;
            m = 2;
            if self.d.decode(&mut st[s]) {
                s = 21;
                m = 4;
                while self.d.decode(&mut st[s]) {
                    m <<= 1;
                    s += 1;
                    if s > 34 {
                        return Err(invalid("arithmetic DC magnitude out of range"));
                    }
                }
            }
        }
        let mut sz = m >> 1;
        let mut bit = sz >> 1;
        s += 14;
        while bit != 0 {
            if self.d.decode(&mut st[s]) {
                sz |= bit;
            }
            bit >>= 1;
        }
        let v = sz + 1;
        let v = if neg { -v } else { v };
        self.da[si] = v;
        Ok(v)
    }

    /// F.2.4.2 (and G.1.3.2 for a first progressive scan of a band).
    fn ac_first(
        &mut self,
        t: usize,
        ss: usize,
        se: usize,
        al: u8,
        kx: u8,
        blk: &mut [i16],
    ) -> Result<()> {
        let st = &mut self.ac_stats[t];
        let mut k = ss;
        while k <= se {
            if self.d.decode(&mut st[3 * (k - 1)]) {
                break;
            }
            while !self.d.decode(&mut st[3 * (k - 1) + 1]) {
                k += 1;
                if k > se {
                    return Err(invalid("arithmetic AC run past the end of the band"));
                }
            }
            let neg = self.d.decode_fixed();
            let mut s = 3 * (k - 1) + 2;
            let mut m: i32 = 1;
            if self.d.decode(&mut st[s]) {
                m = 2;
                if self.d.decode(&mut st[s]) {
                    m = 4;
                    s = if k <= usize::from(kx) { 189 } else { 217 };
                    let base = s;
                    while self.d.decode(&mut st[s]) {
                        m <<= 1;
                        s += 1;
                        if s > base + 13 {
                            return Err(invalid("arithmetic AC magnitude out of range"));
                        }
                    }
                }
            }
            let mut sz = m >> 1;
            let mut bit = sz >> 1;
            s += 14;
            while bit != 0 {
                if self.d.decode(&mut st[s]) {
                    sz |= bit;
                }
                bit >>= 1;
            }
            let v = sz + 1;
            let v = if neg { -v } else { v };
            blk[k] = (v << al) as i16;
            k += 1;
        }
        Ok(())
    }

    /// G.1.3.3: a refinement scan of an AC band.
    fn ac_refine(&mut self, t: usize, ss: usize, se: usize, al: u8, blk: &mut [i16]) -> Result<()> {
        let st = &mut self.ac_stats[t];
        let p1: i16 = 1 << al;
        let mut eobx = ss;
        for k in (ss..=se).rev() {
            if blk[k] != 0 {
                eobx = k + 1;
                break;
            }
        }
        let mut k = ss;
        while k <= se {
            if k >= eobx && self.d.decode(&mut st[3 * (k - 1)]) {
                break;
            }
            loop {
                if blk[k] != 0 {
                    if self.d.decode(&mut st[3 * (k - 1) + 2]) {
                        blk[k] = if blk[k] >= 0 {
                            blk[k].wrapping_add(p1)
                        } else {
                            blk[k].wrapping_sub(p1)
                        };
                    }
                    break;
                }
                if self.d.decode(&mut st[3 * (k - 1) + 1]) {
                    blk[k] = if self.d.decode_fixed() { -p1 } else { p1 };
                    break;
                }
                k += 1;
                if k > se {
                    return Err(invalid(
                        "arithmetic refinement run past the end of the band",
                    ));
                }
            }
            k += 1;
        }
        Ok(())
    }
}

/// After the last scan of a DCT frame (or as much of it as arrived):
/// dequantise and transform every block into its plane. A progressive
/// frame's blocks are those covering each component; a sequential frame's
/// are all of its MCUs' blocks, padding included, as its scans decode
/// them. Rows of blocks are shared among threads; each block's samples
/// depend on that block alone, so the result is the same.
pub(super) fn idct_all(dec: &mut Decoder<'_>, frame: &Frame) {
    let level = 1i32 << (frame.precision - 1);
    let max = (1i32 << frame.precision) - 1;
    let threads = dec.opts.threads;
    for (ci, fc) in frame.comps.iter().enumerate() {
        let q = dec.comp_quant[ci].unwrap_or([1; 64]);
        let coefs = std::mem::take(&mut dec.coefs[ci]);
        let plane = &mut dec.planes[ci];
        let (units_w, units_h) = if frame.progressive {
            (fc.units_w, fc.units_h)
        } else {
            (fc.blocks_stride, fc.rows / 8)
        };
        let rows = 8 * fc.stride;
        let used = (units_h * rows).min(plane.len());
        crate::par::bands(&mut plane[..used], rows, threads, |bys, band| {
            for (i, by) in bys.enumerate() {
                for bx in 0..units_w {
                    let at = (by * fc.blocks_stride + bx) * 64;
                    let blk = &coefs[at..at + 64];
                    let mut c = [0i32; 64];
                    for k in 0..64 {
                        let n = ZIGZAG[k];
                        c[n] = i32::from(blk[k]) * i32::from(q[n]);
                    }
                    idct_to_samples(&c, level, max, &mut band[i * rows + bx * 8..], fc.stride);
                }
            }
        });
    }
}
