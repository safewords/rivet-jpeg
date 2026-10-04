//! Huffman tables: generating the codes from BITS and HUFFVAL (Annex C),
//! decoding (F.2.2.3, with a lookup table in front), encoding, and building
//! an optimal table from symbol statistics (Annex K.2).

use crate::bits::BitReader;
use crate::error::{Result, invalid};

/// A table as it is carried in a DHT segment: the number of codes of each
/// length 1–16 and the symbols in order of increasing code length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TableSpec {
    pub(crate) bits: [u8; 16],
    pub(crate) values: Vec<u8>,
}

impl TableSpec {
    pub(crate) fn new(bits: &[u8; 16], values: &[u8]) -> Self {
        Self {
            bits: *bits,
            values: values.to_vec(),
        }
    }

    /// HUFFSIZE and HUFFCODE of Annex C (Figures C.1 and C.2): the length
    /// and the code of each symbol, in HUFFVAL order. Fails if the lengths
    /// describe more codes than fit (the code overflows its length).
    /// `all_ones` is set when some code of some length is all 1-bits, which
    /// Annex C reserves; a decoder can still use such a table.
    pub(crate) fn codes(&self) -> Result<(Vec<(u8, u16)>, bool)> {
        let total: usize = self.bits.iter().map(|&b| usize::from(b)).sum();
        if total != self.values.len() || total > 256 {
            return Err(invalid(format!(
                "Huffman table: BITS count {total} symbols, {} given",
                self.values.len()
            )));
        }
        let mut out = Vec::with_capacity(total);
        let mut code: u32 = 0;
        let mut all_ones = false;
        for len in 1..=16u8 {
            for _ in 0..self.bits[usize::from(len) - 1] {
                if code >= (1 << len) {
                    return Err(invalid("Huffman table: more codes than the lengths allow"));
                }
                if code == (1 << len) - 1 {
                    all_ones = true;
                }
                out.push((len, code as u16));
                code += 1;
            }
            code <<= 1;
        }
        Ok((out, all_ones))
    }
}

/// Bits of lookahead in the fast decoding table.
const LOOKAHEAD: u32 = 9;

/// A table ready for decoding.
#[derive(Debug, Clone)]
pub(crate) struct DecodeTable {
    /// Indexed by the next `LOOKAHEAD` bits: `(length << 8) | symbol`, or 0
    /// when the code is longer than `LOOKAHEAD`.
    fast: Vec<u16>,
    /// MAXCODE(L) of F.2.2.3, -1 when no code has length L; index 17 is a
    /// sentinel.
    maxcode: [i32; 18],
    /// VALPTR(L) - MINCODE(L): add a code of length L to get its index into
    /// `values`.
    offset: [i32; 17],
    values: Vec<u8>,
    /// Whether the table uses a reserved all-ones code.
    pub(crate) all_ones: bool,
}

impl DecodeTable {
    pub(crate) fn new(spec: &TableSpec) -> Result<Self> {
        let (codes, all_ones) = spec.codes()?;
        let mut fast = vec![0u16; 1 << LOOKAHEAD];
        let mut maxcode = [-1i32; 18];
        let mut offset = [0i32; 17];
        let mut k = 0usize;
        for len in 1..=16usize {
            let n = usize::from(spec.bits[len - 1]);
            if n > 0 {
                let mincode = i32::from(codes[k].1);
                offset[len] = k as i32 - mincode;
                maxcode[len] = i32::from(codes[k + n - 1].1);
            }
            k += n;
        }
        maxcode[17] = i32::MAX;
        for (i, &(len, code)) in codes.iter().enumerate() {
            let len = u32::from(len);
            if len <= LOOKAHEAD {
                let shift = LOOKAHEAD - len;
                let base = usize::from(code) << shift;
                for j in 0..(1usize << shift) {
                    fast[base + j] = ((len as u16) << 8) | u16::from(spec.values[i]);
                }
            }
        }
        Ok(Self {
            fast,
            maxcode,
            offset,
            values: spec.values.clone(),
            all_ones,
        })
    }

    /// Decode one symbol. A bit pattern that matches no code is an error.
    #[inline]
    pub(crate) fn decode(&self, r: &mut BitReader<'_>) -> Result<u8> {
        r.ensure(16);
        let peek = r.peek(LOOKAHEAD) as usize;
        let e = self.fast[peek];
        if e != 0 {
            r.consume(u32::from(e >> 8));
            return Ok(e as u8);
        }
        let bits16 = r.peek(16) as i32;
        for len in (LOOKAHEAD as usize + 1)..=16 {
            let code = bits16 >> (16 - len);
            if code <= self.maxcode[len] {
                r.consume(len as u32);
                let idx = code + self.offset[len];
                return self
                    .values
                    .get(idx as usize)
                    .copied()
                    .ok_or_else(|| invalid("Huffman code out of table"));
            }
        }
        Err(invalid("a Huffman code that matches no table entry"))
    }
}

/// A table ready for encoding: the code and length of each symbol.
#[derive(Debug, Clone)]
pub(crate) struct EncodeTable {
    /// `(length, code)` by symbol; length 0 means the symbol has no code.
    pub(crate) codes: [(u8, u16); 256],
}

impl EncodeTable {
    pub(crate) fn new(spec: &TableSpec) -> Result<Self> {
        let (codes, _) = spec.codes()?;
        let mut out = [(0u8, 0u16); 256];
        for (i, &c) in codes.iter().enumerate() {
            out[usize::from(spec.values[i])] = c;
        }
        Ok(Self { codes: out })
    }
}

/// Figure K.1: Huffman code sizes for the counts `f` (index 256 is the
/// reserved code point).
fn code_sizes(f: &[u64; 257]) -> [u32; 257] {
    let mut f = *f;
    let mut codesize = [0u32; 257];
    let mut others = [-1i32; 257];
    loop {
        // V1: least frequency > 0, the largest V among ties; V2: the next
        // least, by the same rule.
        let least = |exclude: Option<usize>, f: &[u64; 257]| -> Option<usize> {
            let mut best: Option<usize> = None;
            for v in 0..257 {
                if Some(v) == exclude || f[v] == 0 {
                    continue;
                }
                match best {
                    Some(b) if f[v] > f[b] => {}
                    _ => best = Some(v),
                }
            }
            best
        };
        let Some(mut v1) = least(None, &f) else { break };
        let Some(mut v2) = least(Some(v1), &f) else {
            break;
        };
        f[v1] += f[v2];
        f[v2] = 0;
        codesize[v1] += 1;
        while others[v1] != -1 {
            v1 = others[v1] as usize;
            codesize[v1] += 1;
        }
        others[v1] = v2 as i32;
        codesize[v2] += 1;
        while others[v2] != -1 {
            v2 = others[v2] as usize;
            codesize[v2] += 1;
        }
    }
    codesize
}

/// The optimal table for `freq` (counts by symbol 0–255), by the procedure of
/// Annex K.2 (Figures K.1 to K.4): Huffman code sizes with one code point
/// reserved so that no code is all 1-bits, limited to 16 bits.
pub(crate) fn optimal_table(freq: &[u64; 256]) -> TableSpec {
    let mut f = [0u64; 257];
    f[..256].copy_from_slice(freq);
    if f[..256].iter().all(|&x| x == 0) {
        // A table must code something; give it one symbol.
        f[0] = 1;
    }
    f[256] = 1;
    // Figure K.1. K.2 assumes no code comes out longer than 32 bits; with
    // counts skewed enough for that, halve them (keeping every used symbol)
    // and build again.
    let codesize = loop {
        let c = code_sizes(&f);
        if c.iter().all(|&l| l <= 32) {
            break c;
        }
        for v in &mut f[..256] {
            if *v > 0 {
                *v = (*v).div_ceil(2);
            }
        }
    };
    // Figure K.2.
    let mut bits = [0u32; 33];
    for &c in &codesize {
        if c > 0 {
            bits[c as usize] += 1;
        }
    }
    // Figure K.3.
    let mut i = 32usize;
    loop {
        if bits[i] > 0 {
            let mut j = i - 1;
            loop {
                j -= 1;
                if bits[j] > 0 {
                    break;
                }
            }
            bits[i] -= 2;
            bits[i - 1] += 1;
            bits[j + 1] += 2;
            bits[j] -= 1;
            continue;
        }
        i -= 1;
        if i == 16 {
            break;
        }
    }
    while bits[i] == 0 {
        i -= 1;
    }
    bits[i] -= 1;
    // Figure K.4.
    let mut values = Vec::new();
    for size in 1..=32 {
        for (v, &c) in codesize.iter().enumerate().take(256) {
            if c == size {
                values.push(v as u8);
            }
        }
    }
    let mut out = [0u8; 16];
    for l in 1..=16 {
        out[l - 1] = bits[l] as u8;
    }
    TableSpec { bits: out, values }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimal_tables_are_complete_prefix_codes_without_all_ones() {
        let mut freq = [0u64; 256];
        // A geometric distribution long enough to need the 16-bit limit.
        for (i, f) in freq.iter_mut().enumerate().take(40) {
            *f = 1u64 << (39 - i).min(60);
        }
        let t = optimal_table(&freq);
        let (codes, all_ones) = t.codes().unwrap();
        assert!(!all_ones);
        assert_eq!(codes.len(), 40);
        assert!(codes.iter().all(|&(l, _)| l <= 16));
        // Kraft sum, with the reserved point, is exactly one.
        let kraft: f64 = codes.iter().map(|&(l, _)| 2f64.powi(-i32::from(l))).sum();
        let longest = codes.iter().map(|c| c.0).max().unwrap();
        assert!(
            (kraft + 2f64.powi(-i32::from(longest)) - 1.0).abs() < 1e-12,
            "{kraft}"
        );
    }

    #[test]
    fn one_symbol_and_no_symbol() {
        let mut freq = [0u64; 256];
        freq[7] = 10;
        let t = optimal_table(&freq);
        assert_eq!(t.values, vec![7]);
        assert_eq!(t.bits[0], 1);
        let t = optimal_table(&[0; 256]);
        assert_eq!(t.values.len(), 1);
    }
}
