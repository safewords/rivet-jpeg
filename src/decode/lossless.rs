//! Lossless scans (Annex H): prediction (H.1.2.1), Huffman-coded
//! differences (H.1.2.2) and arithmetic-coded differences with the
//! two-dimensional statistical model (H.1.2.3).

use super::scan::{Restart, find_restart};
use super::{Decoder, Frame, Scan, ScanEnd};
use crate::arith::{ArithDecoder, Context};
use crate::bits::BitReader;
use crate::error::{Result, invalid};

/// The difference category of H.1.2.3.1 (0, +S, -S, +L, -L as 0–4).
fn category(d: i32, l: u8, u: u8) -> usize {
    let lower = if l == 0 { 0 } else { 1i32 << (l - 1) };
    let upper = 1i32 << u;
    let a = d.abs();
    if a <= lower {
        0
    } else if a <= upper {
        if d > 0 { 1 } else { 2 }
    } else if d > 0 {
        3
    } else {
        4
    }
}

pub(super) fn decode(dec: &mut Decoder<'_>, frame: &Frame, scan: &Scan) -> Result<ScanEnd> {
    let data = dec.data;
    let start = dec.pos;
    let strict = dec.opts.strict;
    let ri = usize::from(dec.restart_interval);
    let Decoder { tables, planes, warnings, .. } = dec;
    let mut warn = |msg: String| -> Result<()> {
        if strict {
            return Err(invalid(msg));
        }
        if warnings.len() < 64 && !warnings.contains(&msg) {
            warnings.push(msg);
        }
        Ok(())
    };
    let single = scan.comps.len() == 1;
    let (mcus_x, mcus_y) = if single {
        let c = &frame.comps[scan.comps[0].ci];
        (c.width, c.height)
    } else {
        (frame.mcus_x, frame.mcus_y)
    };
    if ri > 0 && ri % mcus_x != 0 {
        warn("lossless restart interval is not a whole number of MCU rows".into())?;
    }
    let pt = u32::from(scan.al);
    let psv = scan.ss;
    let precision = u32::from(frame.precision);
    let initial = 1i32 << (precision - pt - 1).min(15);
    let arithmetic = frame.arithmetic;
    let mut huff = (!arithmetic).then(|| BitReader::new(data, start));
    let mut ari = arithmetic.then(|| ArithDecoder::new(data, start));
    let mut stats = vec![[Context::default(); 158]; 4];
    // For the arithmetic model: per scan component, the differences coded
    // most recently at each column: the line above (Db) at `x` until it is
    // overwritten, the current line (Da) at `x - 1`.
    let mut above: Vec<Vec<i32>> = scan.comps.iter().map(|c| vec![0; frame.comps[c.ci].stride]).collect();
    // The line, per component, where the current restart interval began.
    let mut interval_row = vec![0usize; scan.comps.len()];
    let mut next_rst = 0u8;
    let mut end = ScanEnd::Complete;
    let total = mcus_x * mcus_y;
    let mut m = 0usize;
    'mcus: while m < total {
        if ri > 0 && m > 0 && m % ri == 0 {
            let pos = huff.as_ref().map_or_else(|| ari.as_ref().map_or(0, |a| a.pos()), |h| h.pos());
            let (found, clean) = find_restart(data, pos, next_rst);
            if !clean {
                warn(format!("data before restart marker RST{next_rst}"))?;
            }
            let pos = match found {
                Restart::Found { pos } => pos,
                Restart::Skipped { pos, skipped } => {
                    warn(format!("restart marker RST{next_rst} missing; {skipped} interval(s) lost"))?;
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
            if let Some(h) = huff.as_mut() {
                h.reset_at(pos);
            }
            if let Some(a) = ari.as_mut() {
                a.reset_at(pos);
                stats.iter_mut().for_each(|s| *s = [Context::default(); 158]);
            }
            for a in &mut above {
                a.iter_mut().for_each(|d| *d = 0);
            }
            let my = m / mcus_x;
            for (si, sc) in scan.comps.iter().enumerate() {
                let v = if single { 1 } else { frame.comps[sc.ci].v };
                interval_row[si] = my * v;
            }
        }
        let (mx, my) = (m % mcus_x, m / mcus_x);
        for (si, sc) in scan.comps.iter().enumerate() {
            let fc = &frame.comps[sc.ci];
            let (bh, bv) = if single { (1, 1) } else { (fc.h, fc.v) };
            let stride = fc.stride;
            for v in 0..bv {
                for h in 0..bh {
                    let (x, y) = (mx * bh + h, my * bv + v);
                    let plane = &planes[sc.ci];
                    let at = y * stride + x;
                    let ra = if x > 0 { i32::from(plane[at - 1]) } else { 0 };
                    let rb = if y > 0 { i32::from(plane[at - stride]) } else { 0 };
                    let rc = if x > 0 && y > 0 { i32::from(plane[at - stride - 1]) } else { 0 };
                    let first_line = y == interval_row[si];
                    let px = if first_line {
                        if x == 0 { initial } else { ra }
                    } else if x == 0 {
                        rb
                    } else {
                        match psv {
                            1 => ra,
                            2 => rb,
                            3 => rc,
                            4 => ra + rb - rc,
                            5 => ra + ((rb - rc) >> 1),
                            6 => rb + ((ra - rc) >> 1),
                            _ => (ra + rb) >> 1,
                        }
                    };
                    let diff = if let Some(r) = huff.as_mut() {
                        let t = tables.dc[sc.td].as_ref().ok_or_else(|| invalid("no Huffman table"))?;
                        match t.decode(r) {
                            Ok(s) => {
                                let s = u32::from(s);
                                if s > 16 {
                                    warn(format!("lossless difference category {s}"))?;
                                    end = ScanEnd::Short;
                                    break 'mcus;
                                }
                                r.receive_extend(s)
                            }
                            Err(e) => {
                                warn(format!("{e}"))?;
                                end = ScanEnd::Short;
                                break 'mcus;
                            }
                        }
                    } else {
                        let a = ari.as_mut().ok_or_else(|| invalid("no decoder"))?;
                        let (l, u) = tables.dc_cond[sc.td];
                        let db = if first_line { 0 } else { above[si][x] };
                        let da = if x == 0 { 0 } else { above[si][x - 1] };
                        match arith_diff(a, &mut stats[sc.td], category(da, l, u), category(db, l, u)) {
                            Ok(d) => d,
                            Err(e) => {
                                warn(format!("{e}"))?;
                                end = ScanEnd::Short;
                                break 'mcus;
                            }
                        }
                    };
                    above[si][x] = diff;
                    let sample = (px + diff) & 0xFFFF;
                    planes[sc.ci][at] = sample as u16;
                }
            }
        }
        let exhausted = match (&huff, &ari) {
            (Some(h), _) => h.overrun() && !matches!(h.marker(), Some(0xD0..=0xD7)),
            (_, Some(a)) => a.exhausted(),
            _ => false,
        };
        if exhausted {
            warn("the data ends inside a scan".into())?;
            end = ScanEnd::Short;
            break;
        }
        m += 1;
    }
    // Undo the point transform (H.2.2).
    if pt > 0 {
        for sc in &scan.comps {
            for s in planes[sc.ci].iter_mut() {
                *s = ((u32::from(*s) << pt) & 0xFFFF) as u16;
            }
        }
    }
    let pos = if let Some(h) = huff {
        if end == ScanEnd::Complete {
            let (bits, n) = h.leftover();
            if n >= 8 || bits != (1 << n) - 1 {
                warn("scan: data left over, or padding that is not 1-bits".into())?;
            }
        }
        h.pos()
    } else if let Some(a) = ari {
        let mut p = a.pos();
        while p + 1 < data.len() && !(data[p] == 0xFF && data[p + 1] != 0 && data[p + 1] != 0xFF) {
            p += 1;
        }
        p
    } else {
        start
    };
    dec.pos = pos;
    Ok(end)
}

/// H.1.2.3: one difference with the two-dimensional model.
fn arith_diff(d: &mut ArithDecoder<'_>, st: &mut [Context; 158], ca: usize, cb: usize) -> Result<i32> {
    let s0 = 20 * ca + 4 * cb;
    if !d.decode(&mut st[s0]) {
        return Ok(0);
    }
    let neg = d.decode(&mut st[s0 + 1]);
    let mut s = s0 + 2 + usize::from(neg);
    let x1 = if cb >= 3 { 129 } else { 100 };
    let mut m: i32 = 1;
    if d.decode(&mut st[s]) {
        s = x1;
        m = 2;
        if d.decode(&mut st[s]) {
            s = x1 + 1;
            m = 4;
            while d.decode(&mut st[s]) {
                m <<= 1;
                s += 1;
                if s > x1 + 14 {
                    return Err(invalid("arithmetic lossless magnitude out of range"));
                }
            }
        }
    }
    let mut sz = m >> 1;
    let mut bit = sz >> 1;
    s += 14;
    while bit != 0 {
        if d.decode(&mut st[s]) {
            sz |= bit;
        }
        bit >>= 1;
    }
    let v = sz + 1;
    Ok(if neg { -v } else { v })
}
