//! Huffman entropy coding of the encoder's scans: sequential (F.1.2) and
//! progressive (G.1.2), with a counting pass that gathers the symbol
//! statistics for optimal tables (Annex K.2).

use super::{CompCoefs, Layout, ScanSpec, Tables};
use crate::bits::BitWriter;
use crate::error::Result;
use crate::huffman::{EncodeTable, optimal_table};

/// Where symbols and bits go: counted, or written.
trait Sink {
    /// A symbol of the DC (`ac == false`) or AC table in `slot`.
    fn symbol(&mut self, slot: usize, ac: bool, sym: u8);
    /// Raw bits.
    fn bits(&mut self, value: u32, n: u32);
    /// End a restart interval with RSTm.
    fn restart(&mut self, m: u8);
    fn finish(&mut self);
}

struct Counter {
    freq: [[[u64; 256]; 2]; 2],
}

impl Sink for Counter {
    fn symbol(&mut self, slot: usize, ac: bool, sym: u8) {
        self.freq[slot][usize::from(ac)][usize::from(sym)] += 1;
    }
    fn bits(&mut self, _: u32, _: u32) {}
    fn restart(&mut self, _: u8) {}
    fn finish(&mut self) {}
}

struct Writer<'a> {
    w: BitWriter<'a>,
    codes: [[EncodeTable; 2]; 2],
    missing: bool,
}

impl Sink for Writer<'_> {
    fn symbol(&mut self, slot: usize, ac: bool, sym: u8) {
        let (len, code) = self.codes[slot][usize::from(ac)].codes[usize::from(sym)];
        if len == 0 {
            self.missing = true;
        }
        self.w.put(u32::from(code), u32::from(len));
    }
    fn bits(&mut self, value: u32, n: u32) {
        self.w.put(value, n);
    }
    fn restart(&mut self, m: u8) {
        self.w.marker(0xD0 + m);
    }
    fn finish(&mut self) {
        self.w.flush();
    }
}

/// Magnitude category (SSSS) of a value and its extra bits (F.1.2.1).
#[inline]
fn category(v: i32) -> (u32, u32) {
    if v == 0 {
        return (0, 0);
    }
    let a = v.unsigned_abs();
    let s = 32 - a.leading_zeros();
    let bits = if v < 0 { (v - 1) as u32 & ((1 << s) - 1) } else { v as u32 };
    (s, bits)
}

/// The optimal tables for one scan: a counting pass over it.
pub(super) fn optimal_tables(comps: &[CompCoefs], layout: &Layout, scan: &ScanSpec, progressive: bool) -> Tables {
    let mut c = Counter { freq: [[[0; 256]; 2]; 2] };
    run(&mut c, comps, layout, scan, progressive);
    let t = |slot: usize, ac: usize| optimal_table(&c.freq[slot][ac]);
    [(t(0, 0), t(0, 1)), (t(1, 0), t(1, 1))]
}

pub(super) fn encode_scan(
    out: &mut Vec<u8>,
    comps: &[CompCoefs],
    layout: &Layout,
    scan: &ScanSpec,
    tables: &Tables,
    progressive: bool,
) -> Result<()> {
    let codes = [
        [EncodeTable::new(&tables[0].0)?, EncodeTable::new(&tables[0].1)?],
        [EncodeTable::new(&tables[1].0)?, EncodeTable::new(&tables[1].1)?],
    ];
    let mut w = Writer { w: BitWriter::new(out), codes, missing: false };
    run(&mut w, comps, layout, scan, progressive);
    if w.missing {
        return Err(crate::error::invalid("a symbol with no Huffman code (encoder bug)"));
    }
    Ok(())
}

/// Per-scan coding state.
struct State {
    pred: Vec<i32>,
    eobrun: u32,
    /// Correction bits waiting on the EOB run (BE of Figure G.7).
    be: Vec<u8>,
    slot: usize,
}

impl State {
    fn emit_eobrun<S: Sink>(&mut self, s: &mut S) {
        if self.eobrun > 0 {
            let n = 31 - self.eobrun.leading_zeros();
            s.symbol(self.slot, true, (n << 4) as u8);
            s.bits(self.eobrun - (1 << n), n);
            self.eobrun = 0;
            for &b in &self.be {
                s.bits(u32::from(b), 1);
            }
            self.be.clear();
        }
    }
}

fn run<S: Sink>(s: &mut S, comps: &[CompCoefs], layout: &Layout, scan: &ScanSpec, progressive: bool) {
    let members: Vec<usize> = (0..comps.len()).filter(|i| scan.comps & (1 << i) != 0).collect();
    let single = members.len() == 1;
    let (mx, my) = if single {
        let c = &comps[members[0]];
        (c.units_w, c.units_h)
    } else {
        (layout.mcus_x, layout.mcus_y)
    };
    let mut st = State { pred: vec![0; comps.len()], eobrun: 0, be: Vec::new(), slot: comps[members[0]].table };
    let total = mx * my;
    let mut rst = 0u8;
    for m in 0..total {
        if layout.restart > 0 && m > 0 && m % layout.restart == 0 {
            st.emit_eobrun(s);
            s.restart(rst);
            rst = (rst + 1) % 8;
            st.pred.iter_mut().for_each(|p| *p = 0);
        }
        let (x, y) = (m % mx, m / mx);
        for &ci in &members {
            let c = &comps[ci];
            let (bh, bv) = if single { (1, 1) } else { (c.h, c.v) };
            for v in 0..bv {
                for h in 0..bh {
                    let blk = c.block(x * bh + h, y * bv + v);
                    if !progressive {
                        sequential(s, &mut st, ci, c.table, blk);
                    } else if scan.ss == 0 {
                        if scan.ah == 0 {
                            let dc = i32::from(blk[0]) >> scan.al;
                            let (n, bits) = category(dc - st.pred[ci]);
                            st.pred[ci] = dc;
                            s.symbol(c.table, false, n as u8);
                            s.bits(bits, n);
                        } else {
                            s.bits(((i32::from(blk[0]) >> scan.al) & 1) as u32, 1);
                        }
                    } else if scan.ah == 0 {
                        ac_first(s, &mut st, scan, blk);
                    } else {
                        ac_refine(s, &mut st, scan, blk);
                    }
                }
            }
        }
    }
    st.emit_eobrun(s);
    s.finish();
}

/// F.1.2.1 and F.1.2.2: a whole block.
fn sequential<S: Sink>(s: &mut S, st: &mut State, ci: usize, slot: usize, blk: &[i16; 64]) {
    let dc = i32::from(blk[0]);
    let (n, bits) = category(dc - st.pred[ci]);
    st.pred[ci] = dc;
    s.symbol(slot, false, n as u8);
    s.bits(bits, n);
    let mut run = 0u32;
    for &c in &blk[1..] {
        if c == 0 {
            run += 1;
            continue;
        }
        while run > 15 {
            s.symbol(slot, true, 0xF0);
            run -= 16;
        }
        let (n, bits) = category(i32::from(c));
        s.symbol(slot, true, ((run << 4) | n) as u8);
        s.bits(bits, n);
        run = 0;
    }
    if run > 0 {
        s.symbol(slot, true, 0x00);
    }
}

/// The point transform of an AC coefficient: a divide by 2^Al, towards 0.
#[inline]
fn pt(c: i16, al: u8) -> i32 {
    let v = i32::from(c);
    if v < 0 { -((-v) >> al) } else { v >> al }
}

/// G.1.2.2: the first scan of a band.
fn ac_first<S: Sink>(s: &mut S, st: &mut State, scan: &ScanSpec, blk: &[i16; 64]) {
    let mut run = 0u32;
    for &c in &blk[scan.ss..=scan.se] {
        let v = pt(c, scan.al);
        if v == 0 {
            run += 1;
            continue;
        }
        st.emit_eobrun(s);
        while run > 15 {
            s.symbol(st.slot, true, 0xF0);
            run -= 16;
        }
        let (n, bits) = category(v);
        s.symbol(st.slot, true, ((run << 4) | n) as u8);
        s.bits(bits, n);
        run = 0;
    }
    if run > 0 {
        st.eobrun += 1;
        if st.eobrun == 0x7FFF {
            st.emit_eobrun(s);
        }
    }
}

/// G.1.2.3 (Figure G.7): a refinement scan of a band.
fn ac_refine<S: Sink>(s: &mut S, st: &mut State, scan: &ScanSpec, blk: &[i16; 64]) {
    let abs: Vec<u32> = blk[scan.ss..=scan.se].iter().map(|&c| (i32::from(c).unsigned_abs()) >> scan.al).collect();
    // EOB: the position after the last coefficient that becomes non-zero in
    // this scan.
    let eob = abs.iter().rposition(|&a| a == 1).map_or(0, |p| p + 1);
    let mut run = 0u32;
    let mut br: Vec<u8> = Vec::new();
    for (k, &a) in abs.iter().enumerate() {
        if a == 0 {
            run += 1;
            continue;
        }
        while run > 15 && k < eob {
            st.emit_eobrun(s);
            s.symbol(st.slot, true, 0xF0);
            run -= 16;
            for &b in &br {
                s.bits(u32::from(b), 1);
            }
            br.clear();
        }
        if a > 1 {
            br.push((a & 1) as u8);
            continue;
        }
        st.emit_eobrun(s);
        s.symbol(st.slot, true, ((run << 4) | 1) as u8);
        s.bits(u32::from(blk[scan.ss + k] > 0), 1);
        for &b in &br {
            s.bits(u32::from(b), 1);
        }
        br.clear();
        run = 0;
    }
    if run > 0 || !br.is_empty() {
        st.eobrun += 1;
        st.be.extend_from_slice(&br);
        if st.eobrun == 0x7FFF || st.be.len() > 937 {
            st.emit_eobrun(s);
        }
    }
}
