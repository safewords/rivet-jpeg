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
    /// A symbol and then `n` (at most 16) raw bits.
    #[inline]
    fn symbol_bits(&mut self, slot: usize, ac: bool, sym: u8, value: u32, n: u32) {
        self.symbol(slot, ac, sym);
        self.bits(value, n);
    }
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

/// Bits without byte stuffing, for a piece of a scan coded on its own
/// thread; [`Writer::append`] stuffs them when the pieces are joined.
struct RawWriter<'a> {
    out: Vec<u8>,
    acc: u64,
    n: u32,
    codes: &'a [[EncodeTable; 2]; 2],
    missing: bool,
    /// The stretches before each restart marker the piece holds: their bits
    /// (whole bytes, then `n` more in `acc`) and the marker's number.
    before_restarts: Vec<(Vec<u8>, u64, u32, u8)>,
}

impl RawWriter<'_> {
    #[inline]
    fn put(&mut self, code: u32, len: u32) {
        if len == 0 {
            return;
        }
        self.acc = (self.acc << len) | u64::from(code & (((1u64 << len) - 1) as u32));
        self.n += len;
        while self.n >= 8 {
            self.n -= 8;
            self.out.push((self.acc >> self.n) as u8);
        }
        self.acc &= (1u64 << self.n) - 1;
    }
}

impl Sink for RawWriter<'_> {
    #[inline]
    fn symbol(&mut self, slot: usize, ac: bool, sym: u8) {
        let (len, code) = self.codes[slot][usize::from(ac)].codes[usize::from(sym)];
        if len == 0 {
            self.missing = true;
        }
        self.put(u32::from(code), u32::from(len));
    }
    #[inline]
    fn bits(&mut self, value: u32, n: u32) {
        self.put(value, n);
    }
    #[inline]
    fn symbol_bits(&mut self, slot: usize, ac: bool, sym: u8, value: u32, n: u32) {
        let (len, code) = self.codes[slot][usize::from(ac)].codes[usize::from(sym)];
        if len == 0 {
            self.missing = true;
        }
        let value = value & ((1u32 << n) - 1);
        self.put((u32::from(code) << n) | value, u32::from(len) + n);
    }
    fn restart(&mut self, m: u8) {
        // The bits so far end at the marker (padded there when appended).
        self.before_restarts.push((std::mem::take(&mut self.out), self.acc, self.n, m));
        self.acc = 0;
        self.n = 0;
    }
    fn finish(&mut self) {}
}

impl Writer<'_> {
    /// Appends a piece's bits (whole bytes, then `n` more in `acc`),
    /// stuffing as they land.
    fn append(&mut self, bytes: &[u8], acc: u64, n: u32) {
        for &b in bytes {
            self.w.put(u32::from(b), 8);
        }
        self.w.put(acc as u32, n);
    }
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
    #[inline]
    fn symbol_bits(&mut self, slot: usize, ac: bool, sym: u8, value: u32, n: u32) {
        let (len, code) = self.codes[slot][usize::from(ac)].codes[usize::from(sym)];
        if len == 0 {
            self.missing = true;
        }
        let value = value & ((1u32 << n) - 1);
        self.w.put((u32::from(code) << n) | value, u32::from(len) + n);
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

/// MCUs per piece when a sequential scan is coded in pieces on several
/// threads.
const PIECE_MCUS: usize = 1024;

/// The pieces a scan is coded in: a sequential scan splits at MCU
/// boundaries (its only state across blocks is the DC predictions, which
/// the previous block gives), at restart interval boundaries when it has
/// them (where the predictions start afresh); a progressive scan is one
/// piece.
fn pieces(comps: &[CompCoefs], layout: &Layout, scan: &ScanSpec, progressive: bool) -> Vec<std::ops::Range<usize>> {
    let (mx, my) = geometry(comps, layout, scan);
    let total = mx * my;
    let step = match layout.restart {
        0 => PIECE_MCUS,
        ri => ri * (PIECE_MCUS / ri).max(1),
    };
    if progressive || total <= step {
        return std::iter::once(0..total).collect();
    }
    (0..total.div_ceil(step)).map(|i| i * step..((i + 1) * step).min(total)).collect()
}

/// The optimal tables for one scan: a counting pass over it (in pieces on
/// several threads where the scan allows, the counts summed).
pub(super) fn optimal_tables(comps: &[CompCoefs], layout: &Layout, scan: &ScanSpec, progressive: bool) -> Tables {
    let ranges = pieces(comps, layout, scan, progressive);
    let counts = crate::par::map(ranges.len(), layout.threads, |i| {
        let mut c = Counter { freq: [[[0; 256]; 2]; 2] };
        run(&mut c, comps, layout, scan, progressive, ranges[i].clone());
        c.freq
    });
    let mut freq = [[[0u64; 256]; 2]; 2];
    for c in &counts {
        for (f, c) in freq.as_flattened_mut().as_flattened_mut().iter_mut().zip(c.as_flattened().as_flattened()) {
            *f += c;
        }
    }
    let t = |slot: usize, ac: usize| optimal_table(&freq[slot][ac]);
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
    let ranges = pieces(comps, layout, scan, progressive);
    let mut w = if ranges.len() == 1 {
        let mut w = Writer { w: BitWriter::new(out), codes, missing: false };
        run(&mut w, comps, layout, scan, progressive, ranges[0].clone());
        w
    } else {
        let parts = crate::par::map(ranges.len(), layout.threads, |i| {
            let mut r =
                RawWriter { out: Vec::new(), acc: 0, n: 0, codes: &codes, missing: false, before_restarts: Vec::new() };
            run(&mut r, comps, layout, scan, progressive, ranges[i].clone());
            (r.before_restarts, r.out, r.acc, r.n, r.missing)
        });
        let mut w = Writer { w: BitWriter::new(out), codes, missing: false };
        for (before_restarts, bytes, acc, n, missing) in parts {
            for (bytes, acc, n, m) in before_restarts {
                w.append(&bytes, acc, n);
                w.restart(m);
            }
            w.append(&bytes, acc, n);
            w.missing |= missing;
        }
        w
    };
    w.finish();
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

/// The scan's MCUs across and down (blocks, for a single component).
fn geometry(comps: &[CompCoefs], layout: &Layout, scan: &ScanSpec) -> (usize, usize) {
    let members: Vec<usize> = (0..comps.len()).filter(|i| scan.comps & (1 << i) != 0).collect();
    if members.len() == 1 {
        let c = &comps[members[0]];
        (c.units_w, c.units_h)
    } else {
        (layout.mcus_x, layout.mcus_y)
    }
}

/// Codes MCUs `mcus` of a scan into `s`. A range that does not start the
/// scan (only sequential scans are split) takes its DC
/// predictions from the blocks just before it. Padding and the end of the
/// scan are left to the caller.
fn run<S: Sink>(s: &mut S, comps: &[CompCoefs], layout: &Layout, scan: &ScanSpec, progressive: bool, mcus: std::ops::Range<usize>) {
    let members: Vec<usize> = (0..comps.len()).filter(|i| scan.comps & (1 << i) != 0).collect();
    let single = members.len() == 1;
    let (mx, _) = geometry(comps, layout, scan);
    let mut st = State { pred: vec![0; comps.len()], eobrun: 0, be: Vec::new(), slot: comps[members[0]].table };
    if mcus.start > 0 {
        let (x, y) = ((mcus.start - 1) % mx, (mcus.start - 1) / mx);
        for &ci in &members {
            let c = &comps[ci];
            let (bh, bv) = if single { (1, 1) } else { (c.h, c.v) };
            st.pred[ci] = i32::from(c.block(x * bh + bh - 1, y * bv + bv - 1)[0]);
        }
    }
    // The number of the first marker this range writes: RSTn precedes
    // interval n + 1, counting from 0 mod 8.
    let mut rst = match layout.restart {
        0 => 0,
        ri => (mcus.start.saturating_sub(1) / ri % 8) as u8,
    };
    for m in mcus {
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
}

/// F.1.2.1 and F.1.2.2: a whole block.
fn sequential<S: Sink>(s: &mut S, st: &mut State, ci: usize, slot: usize, blk: &[i16; 64]) {
    let dc = i32::from(blk[0]);
    let (n, bits) = category(dc - st.pred[ci]);
    st.pred[ci] = dc;
    s.symbol_bits(slot, false, n as u8, bits, n);
    // The non-zero AC coefficients as a bit mask (bit k for zig-zag
    // position k), visited in order: the runs of zeros between them are the
    // gaps between set bits.
    let mut nonzero = 0u64;
    for (k, &c) in blk.iter().enumerate().skip(1) {
        nonzero |= u64::from(c != 0) << k;
    }
    let mut last = 0u32;
    while nonzero != 0 {
        let k = nonzero.trailing_zeros();
        nonzero &= nonzero - 1;
        let mut run = k - last - 1;
        while run > 15 {
            s.symbol(slot, true, 0xF0);
            run -= 16;
        }
        let (n, bits) = category(i32::from(blk[k as usize]));
        s.symbol_bits(slot, true, ((run << 4) | n) as u8, bits, n);
        last = k;
    }
    if last < 63 {
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
