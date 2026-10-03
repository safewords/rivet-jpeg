//! The binary arithmetic decoder of Annex D.2 (and the matching encoder of
//! D.1, used to make arithmetic-coded files for the tests and on request).
//!
//! Registers follow Tables D.2 and D.5: A is the 16-bit interval (0x10000
//! at the start), C the 32-bit code register whose high half is Cx. The
//! probability estimation state machine is Table D.3.

use crate::tables::QE_TABLE;

/// One statistics bin: an index into Table D.3 and the MPS sense.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Context {
    index: u8,
    mps: u8,
}

/// The fixed estimate the sign and refinement decisions use (Qe = 0x5A1D,
/// MPS = 0, never updated).
const FIXED_QE: u32 = 0x5A1D;

pub(crate) struct ArithDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    c: u32,
    a: u32,
    ct: i32,
    marker: Option<u8>,
    /// 0-bytes supplied after the end of the data (not after a marker).
    eof_bytes: usize,
}

impl<'a> ArithDecoder<'a> {
    /// Initdec (Figure D.22), the segment starting at `pos`.
    pub(crate) fn new(data: &'a [u8], pos: usize) -> Self {
        let mut d = Self { data, pos, c: 0, a: 0, ct: 0, marker: None, eof_bytes: 0 };
        d.init();
        d
    }

    fn init(&mut self) {
        self.a = 0x10000;
        self.c = 0;
        self.byte_in();
        self.c <<= 8;
        self.byte_in();
        self.c <<= 8;
        self.ct = 0;
    }

    /// Byte_in and Unstuff_0 (Figures D.20 and D.21).
    fn byte_in(&mut self) {
        if self.marker.is_some() {
            return;
        }
        if self.pos >= self.data.len() {
            self.eof_bytes += 1;
            return;
        }
        let b = self.data[self.pos];
        if b != 0xFF {
            self.pos += 1;
            self.c = self.c.wrapping_add(u32::from(b) << 8);
            return;
        }
        let mut p = self.pos + 1;
        while p < self.data.len() && self.data[p] == 0xFF {
            p += 1;
        }
        if p >= self.data.len() {
            self.pos = self.data.len();
            self.eof_bytes += 1;
        } else if self.data[p] == 0 {
            self.pos = p + 1;
            self.c |= 0xFF00;
        } else {
            self.marker = Some(self.data[p]);
            self.pos = p - 1;
        }
    }

    fn renorm(&mut self) {
        loop {
            if self.ct == 0 {
                self.byte_in();
                self.ct = 8;
            }
            self.a <<= 1;
            self.c = self.c.wrapping_shl(1);
            self.ct -= 1;
            if self.a >= 0x8000 {
                break;
            }
        }
    }

    /// Decode(S), Figures D.16 to D.18, with the estimation of D.1.5.
    #[inline]
    pub(crate) fn decode(&mut self, s: &mut Context) -> bool {
        let (qe, nlps, nmps, switch) = QE_TABLE[usize::from(s.index)];
        let qe = u32::from(qe);
        self.a -= qe;
        let cx = self.c >> 16;
        let d;
        if cx < self.a {
            if self.a < 0x8000 {
                // Cond_MPS_exchange.
                if self.a < qe {
                    d = 1 - s.mps;
                    if switch {
                        s.mps = 1 - s.mps;
                    }
                    s.index = nlps;
                } else {
                    d = s.mps;
                    s.index = nmps;
                }
                self.renorm();
            } else {
                d = s.mps;
            }
        } else {
            // Cond_LPS_exchange.
            self.c = self.c.wrapping_sub(self.a << 16);
            if self.a < qe {
                d = s.mps;
                s.index = nmps;
            } else {
                d = 1 - s.mps;
                if switch {
                    s.mps = 1 - s.mps;
                }
                s.index = nlps;
            }
            self.a = qe;
            self.renorm();
        }
        d != 0
    }

    /// A decision with the fixed estimate (Qe = 0x5A1D, MPS = 0).
    #[inline]
    pub(crate) fn decode_fixed(&mut self) -> bool {
        let qe = FIXED_QE;
        self.a -= qe;
        let cx = self.c >> 16;
        let d;
        if cx < self.a {
            if self.a < 0x8000 {
                d = u32::from(self.a < qe);
                self.renorm();
            } else {
                d = 0;
            }
        } else {
            self.c = self.c.wrapping_sub(self.a << 16);
            d = u32::from(self.a >= qe);
            self.a = qe;
            self.renorm();
        }
        d != 0
    }

    pub(crate) fn marker(&self) -> Option<u8> {
        self.marker
    }

    /// Whether well over what a coder flush could leave out has been read
    /// past the end of the data: the file is truncated.
    pub(crate) fn exhausted(&self) -> bool {
        self.eof_bytes > 256
    }

    pub(crate) fn at_eof(&self) -> bool {
        self.eof_bytes > 0
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Restart at `pos`: Initdec again.
    pub(crate) fn reset_at(&mut self, pos: usize) {
        self.pos = pos;
        self.marker = None;
        self.eof_bytes = 0;
        self.init();
    }
}

/// The arithmetic encoder of Annex D.1 (Initenc, Code_0/Code_1 with the
/// conditional exchange, Renorm_e, Byte_out with carry and stuffing, Flush).
pub(crate) struct ArithEncoder<'a> {
    out: &'a mut Vec<u8>,
    c: u32,
    a: u32,
    ct: i32,
    /// Stacked 0xFF bytes waiting on a carry (ST).
    st: u32,
    /// The byte waiting to be output (B), or none yet.
    b: Option<u8>,
}

impl<'a> ArithEncoder<'a> {
    /// Initenc (Figure D.13).
    pub(crate) fn new(out: &'a mut Vec<u8>) -> Self {
        Self { out, c: 0, a: 0x10000, ct: 11, st: 0, b: None }
    }

    pub(crate) fn encode(&mut self, s: &mut Context, bit: bool) {
        let (qe, nlps, nmps, switch) = QE_TABLE[usize::from(s.index)];
        let qe = u32::from(qe);
        if u8::from(bit) == s.mps {
            // Code_MPS (Figure D.4).
            self.a -= qe;
            if self.a < 0x8000 {
                if self.a < qe {
                    self.c += self.a;
                    self.a = qe;
                }
                s.index = nmps;
                self.renorm();
            }
        } else {
            // Code_LPS (Figure D.3).
            self.a -= qe;
            if self.a >= qe {
                self.c += self.a;
                self.a = qe;
            }
            if switch {
                s.mps = 1 - s.mps;
            }
            s.index = nlps;
            self.renorm();
        }
    }

    pub(crate) fn encode_fixed(&mut self, bit: bool) {
        let qe = FIXED_QE;
        if !bit {
            self.a -= qe;
            if self.a < 0x8000 {
                if self.a < qe {
                    self.c += self.a;
                    self.a = qe;
                }
                self.renorm();
            }
        } else {
            self.a -= qe;
            if self.a >= qe {
                self.c += self.a;
                self.a = qe;
            }
            self.renorm();
        }
    }

    /// Renorm_e (Figure D.8).
    fn renorm(&mut self) {
        loop {
            self.a <<= 1;
            self.c <<= 1;
            self.ct -= 1;
            if self.ct == 0 {
                self.byte_out();
                self.ct = 8;
            }
            if self.a >= 0x8000 {
                break;
            }
        }
    }

    /// Byte_out (Figures D.9 to D.12): resolves carries into the stacked
    /// 0xFF bytes and stuffs a zero after every 0xFF written.
    fn byte_out(&mut self) {
        let t = self.c >> 19;
        if t > 0xFF {
            // Carry: B+1, then the stacked 0xFFs become 0x00s.
            if let Some(b) = self.b.as_mut() {
                *b = b.wrapping_add(1);
            }
            self.emit_b();
            for _ in 0..self.st {
                self.out.push(0x00);
            }
            self.st = 0;
            self.b = Some((t & 0xFF) as u8);
        } else if t == 0xFF {
            self.st += 1;
        } else {
            self.emit_b();
            for _ in 0..self.st {
                self.out.push(0xFF);
                self.out.push(0x00);
            }
            self.st = 0;
            self.b = Some(t as u8);
        }
        self.c &= 0x7FFFF;
    }

    fn emit_b(&mut self) {
        if let Some(b) = self.b {
            self.out.push(b);
            if b == 0xFF {
                self.out.push(0x00);
            }
        }
    }

    /// Flush (Figure D.14 with Clear_final_bits, D.15): final bytes, with
    /// the trailing zero bytes discarded.
    pub(crate) fn finish(mut self) {
        let start = self.out.len();
        // Clear_final_bits.
        let t = self.c.wrapping_add(self.a - 1);
        let t = t & 0xFFFF_0000;
        self.c = if t < self.c { t + 0x8000 } else { t };
        self.c <<= self.ct as u32;
        self.byte_out();
        self.c <<= 8;
        self.byte_out();
        self.emit_b();
        for _ in 0..self.st {
            self.out.push(0xFF);
            self.out.push(0x00);
        }
        // Discard final zeros (a stuffed 0xFF 0x00 pair is kept whole).
        while self.out.len() > start && self.out.last() == Some(&0) {
            let n = self.out.len();
            if n >= 2 && self.out[n - 2] == 0xFF {
                break;
            }
            self.out.pop();
        }
    }
}
