//! Arithmetic entropy coding of the encoder's scans: the statistical models
//! of F.1.4 (sequential) and G.1.3 (progressive), with the default
//! conditioning (L = 0, U = 1, Kx = 5), so no DAC segment is written.

use super::{CompCoefs, Layout, ScanSpec};
use crate::arith::{ArithEncoder, Context};

struct Coder<'a> {
    e: ArithEncoder<'a>,
    dc: [[Context; 64]; 2],
    ac: [[Context; 256]; 2],
    pred: Vec<i32>,
    da: Vec<i32>,
}

pub(super) fn encode_scan(
    out: &mut Vec<u8>,
    comps: &[CompCoefs],
    layout: &Layout,
    scan: &ScanSpec,
    progressive: bool,
) {
    let members: Vec<usize> = (0..comps.len())
        .filter(|i| scan.comps & (1 << i) != 0)
        .collect();
    let single = members.len() == 1;
    let (mx, my) = if single {
        let c = &comps[members[0]];
        (c.units_w, c.units_h)
    } else {
        (layout.mcus_x, layout.mcus_y)
    };
    let total = mx * my;
    let mut rst = 0u8;
    let mut segment = Vec::new();
    let mut m = 0;
    while m < total {
        let end = if layout.restart > 0 {
            (m + layout.restart).min(total)
        } else {
            total
        };
        segment.clear();
        {
            let mut c = Coder {
                e: ArithEncoder::new(&mut segment),
                dc: [[Context::default(); 64]; 2],
                ac: [[Context::default(); 256]; 2],
                pred: vec![0; comps.len()],
                da: vec![0; comps.len()],
            };
            for mcu in m..end {
                let (x, y) = (mcu % mx, mcu / mx);
                for &ci in &members {
                    let comp = &comps[ci];
                    let (bh, bv) = if single { (1, 1) } else { (comp.h, comp.v) };
                    for v in 0..bv {
                        for h in 0..bh {
                            let blk = comp.block(x * bh + h, y * bv + v);
                            c.block(ci, comp.table, blk, scan, progressive);
                        }
                    }
                }
            }
            c.e.finish();
        }
        out.extend_from_slice(&segment);
        if end < total {
            out.extend_from_slice(&[0xFF, 0xD0 + rst]);
            rst = (rst + 1) % 8;
        }
        m = end;
    }
}

impl Coder<'_> {
    fn block(&mut self, ci: usize, t: usize, blk: &[i16; 64], scan: &ScanSpec, progressive: bool) {
        if !progressive {
            let dc = i32::from(blk[0]);
            self.dc_diff(ci, t, dc - self.pred[ci]);
            self.pred[ci] = dc;
            self.ac_first(t, blk, 1, 63, 0);
        } else if scan.ss == 0 {
            if scan.ah == 0 {
                let dc = i32::from(blk[0]) >> scan.al;
                self.dc_diff(ci, t, dc - self.pred[ci]);
                self.pred[ci] = dc;
            } else {
                self.e.encode_fixed((i32::from(blk[0]) >> scan.al) & 1 != 0);
            }
        } else if scan.ah == 0 {
            self.ac_first(t, blk, scan.ss, scan.se, scan.al);
        } else {
            self.ac_refine(t, blk, scan.ss, scan.se, scan.al);
        }
    }

    /// Figure F.4 with the DC model of F.1.4.4.1.
    fn dc_diff(&mut self, ci: usize, t: usize, v: i32) {
        let da = self.da[ci];
        let s0 = match da {
            0 => 0,
            1 | 2 => 4,
            -2 | -1 => 8,
            d if d > 0 => 12,
            _ => 16,
        };
        let st = &mut self.dc[t];
        self.da[ci] = v;
        if v == 0 {
            self.e.encode(&mut st[s0], false);
            return;
        }
        self.e.encode(&mut st[s0], true);
        self.e.encode(&mut st[s0 + 1], v < 0);
        let s = s0 + 2 + usize::from(v < 0);
        magnitude(&mut self.e, st, s, 20, 21, v.unsigned_abs() - 1);
    }

    /// Figure F.5 (with Kmin = Ss for a progressive band).
    fn ac_first(&mut self, t: usize, blk: &[i16; 64], ss: usize, se: usize, al: u8) {
        let val = |k: usize| -> i32 {
            let c = i32::from(blk[k]);
            if c < 0 { -((-c) >> al) } else { c >> al }
        };
        let eob = (ss..=se).rev().find(|&k| val(k) != 0).map_or(ss, |k| k + 1);
        let st = &mut self.ac[t];
        let mut k = ss;
        while k <= se {
            if k == eob {
                self.e.encode(&mut st[3 * (k - 1)], true);
                break;
            }
            self.e.encode(&mut st[3 * (k - 1)], false);
            while val(k) == 0 {
                self.e.encode(&mut st[3 * (k - 1) + 1], false);
                k += 1;
            }
            self.e.encode(&mut st[3 * (k - 1) + 1], true);
            let v = val(k);
            self.e.encode_fixed(v < 0);
            let x2 = if k <= 5 { 189 } else { 217 };
            magnitude(
                &mut self.e,
                st,
                3 * (k - 1) + 2,
                3 * (k - 1) + 2,
                x2,
                v.unsigned_abs() - 1,
            );
            k += 1;
        }
    }

    /// Figures G.10 and G.11.
    fn ac_refine(&mut self, t: usize, blk: &[i16; 64], ss: usize, se: usize, al: u8) {
        let abs = |k: usize| -> u32 { i32::from(blk[k]).unsigned_abs() >> al };
        let eobx = (ss..=se).rev().find(|&k| abs(k) > 1).map_or(ss, |k| k + 1);
        let eob = (ss..=se).rev().find(|&k| abs(k) != 0).map_or(ss, |k| k + 1);
        let st = &mut self.ac[t];
        let mut k = ss;
        while k <= se {
            if k >= eobx {
                if k == eob {
                    self.e.encode(&mut st[3 * (k - 1)], true);
                    break;
                }
                self.e.encode(&mut st[3 * (k - 1)], false);
            }
            loop {
                let a = abs(k);
                if a > 1 {
                    self.e.encode(&mut st[3 * (k - 1) + 2], a & 1 != 0);
                    break;
                }
                if a == 1 {
                    self.e.encode(&mut st[3 * (k - 1) + 1], true);
                    self.e.encode_fixed(blk[k] < 0);
                    break;
                }
                self.e.encode(&mut st[3 * (k - 1) + 1], false);
                k += 1;
            }
            k += 1;
        }
    }
}

/// Figures F.8 and F.9: the magnitude category of `sz` and its low bits,
/// starting at bin `s`, with X1 and X2 as given.
fn magnitude(
    e: &mut ArithEncoder<'_>,
    st: &mut [Context],
    mut s: usize,
    x1: usize,
    x2: usize,
    sz: u32,
) {
    let mut m: u32 = 1;
    if sz >= m {
        e.encode(&mut st[s], true);
        m = 2;
        s = x1;
        if sz >= m {
            e.encode(&mut st[s], true);
            m = 4;
            s = x2;
            while sz >= m {
                e.encode(&mut st[s], true);
                m <<= 1;
                s += 1;
            }
        }
    }
    e.encode(&mut st[s], false);
    m >>= 1;
    s += 14;
    m >>= 1;
    while m != 0 {
        e.encode(&mut st[s], sz & m != 0);
        m >>= 1;
    }
}
