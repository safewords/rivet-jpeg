//! Reading and writing the bits of a Huffman-coded entropy-coded segment
//! (F.1.2.3, F.2.2.5): MSB first, a 0x00 stuffed after every 0xFF data byte,
//! and the segment ended by a marker.

/// Reads an entropy-coded segment. At a marker, or at the end of the data,
/// it supplies 0-bits, as F.2.2.5 directs, and records that it did; reading
/// past the real data is "overrun", which the scan decoders check to stop at
/// the end of a truncated file instead of decoding zeros forever.
pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    /// The next byte to load.
    pos: usize,
    /// Valid bits, left-aligned.
    buf: u64,
    nbits: u32,
    /// Bits at the bottom of the valid ones that are padding (not data).
    pad: u32,
    /// The marker that stopped loading, if one has.
    marker: Option<u8>,
    overrun: bool,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8], pos: usize) -> Self {
        Self {
            data,
            pos,
            buf: 0,
            nbits: 0,
            pad: 0,
            marker: None,
            overrun: false,
        }
    }

    /// Load bytes until at least `n` (at most 57) bits are buffered.
    #[inline]
    pub(crate) fn ensure(&mut self, n: u32) {
        while self.nbits < n {
            self.load_byte();
        }
    }

    fn load_byte(&mut self) {
        let byte = if self.marker.is_some() || self.pos >= self.data.len() {
            self.pad += 8;
            0
        } else {
            let b = self.data[self.pos];
            if b != 0xFF {
                self.pos += 1;
                b
            } else {
                // 0xFF: a stuffed data byte, fill bytes before a marker, or a
                // marker.
                let mut p = self.pos + 1;
                while p < self.data.len() && self.data[p] == 0xFF {
                    p += 1;
                }
                if p >= self.data.len() {
                    // The data ends inside a marker prefix.
                    self.pos = self.data.len();
                    self.pad += 8;
                    0
                } else if self.data[p] == 0x00 && p == self.pos + 1 {
                    self.pos += 2;
                    0xFF
                } else if self.data[p] == 0x00 {
                    // 0xFF 0xFF 0x00: not valid syntax; read it as one
                    // stuffed 0xFF and carry on.
                    self.pos = p + 1;
                    0xFF
                } else {
                    self.marker = Some(self.data[p]);
                    self.pos = p - 1;
                    self.pad += 8;
                    0
                }
            }
        };
        self.buf |= u64::from(byte) << (56 - self.nbits);
        self.nbits += 8;
    }

    /// The next `n` bits (1–32), without consuming them. Call `ensure` first.
    #[inline]
    pub(crate) fn peek(&self, n: u32) -> u32 {
        (self.buf >> (64 - n)) as u32
    }

    #[inline]
    pub(crate) fn consume(&mut self, n: u32) {
        self.buf <<= n;
        self.nbits -= n;
        if self.nbits < self.pad {
            self.overrun = true;
            self.pad = self.nbits;
        }
    }

    /// `n` bits (0–16) as an unsigned value.
    #[inline]
    pub(crate) fn bits(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        self.ensure(n);
        let v = self.peek(n);
        self.consume(n);
        v
    }

    #[inline]
    pub(crate) fn bit(&mut self) -> bool {
        self.bits(1) != 0
    }

    /// RECEIVE(s) followed by EXTEND (F.2.2.1): `s` bits as a signed value
    /// of magnitude category `s`.
    #[inline]
    pub(crate) fn receive_extend(&mut self, s: u32) -> i32 {
        if s == 0 {
            return 0;
        }
        if s >= 16 {
            // Category 16 only occurs in lossless coding, with no extra bits
            // (Table H.2); a DCT category above 15 is invalid, read as this.
            return 32768;
        }
        let v = self.bits(s) as i32;
        if v < (1 << (s - 1)) {
            v - (1 << s) + 1
        } else {
            v
        }
    }

    /// Whether 0-bits standing in for missing data have been consumed.
    pub(crate) fn overrun(&self) -> bool {
        self.overrun
    }

    /// The marker that ended the segment, if loading has reached one.
    pub(crate) fn marker(&self) -> Option<u8> {
        self.marker
    }

    /// The bits still buffered that came from the data, and how many: what is
    /// left of the last byte after a scan, which T.81 (F.1.2.3) says is
    /// padded with 1-bits.
    pub(crate) fn leftover(&self) -> (u32, u32) {
        let n = self.nbits.saturating_sub(self.pad);
        if n == 0 {
            (0, 0)
        } else {
            ((self.buf >> (64 - n)) as u32, n)
        }
    }

    /// Discard buffered bits and move to `pos`, for a restart.
    pub(crate) fn reset_at(&mut self, pos: usize) {
        self.pos = pos;
        self.buf = 0;
        self.nbits = 0;
        self.pad = 0;
        self.marker = None;
        self.overrun = false;
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }
}

/// Writes an entropy-coded segment: stuffs a 0x00 after each 0xFF and pads
/// the last byte with 1-bits.
pub(crate) struct BitWriter<'a> {
    out: &'a mut Vec<u8>,
    acc: u64,
    n: u32,
}

impl<'a> BitWriter<'a> {
    pub(crate) fn new(out: &'a mut Vec<u8>) -> Self {
        Self { out, acc: 0, n: 0 }
    }

    /// Append the low `len` bits of `code` (len up to 32).
    #[inline]
    pub(crate) fn put(&mut self, code: u32, len: u32) {
        if len == 0 {
            return;
        }
        self.acc = (self.acc << len) | u64::from(code & (((1u64 << len) - 1) as u32));
        self.n += len;
        while self.n >= 8 {
            self.n -= 8;
            let b = (self.acc >> self.n) as u8;
            self.out.push(b);
            if b == 0xFF {
                self.out.push(0);
            }
        }
        self.acc &= (1u64 << self.n) - 1;
    }

    /// Pad to a byte boundary with 1-bits.
    pub(crate) fn flush(&mut self) {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put((1 << pad) - 1, pad);
        }
    }

    /// Flush, then append a marker (for RSTm).
    pub(crate) fn marker(&mut self, m: u8) {
        self.flush();
        self.out.push(0xFF);
        self.out.push(m);
    }
}
