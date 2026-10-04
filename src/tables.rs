//! Tables from T.81: the zig-zag order (Figure A.6), the example
//! quantisation tables of Annex K.1 and Huffman tables of Annex K.3, and the
//! arithmetic coder's probability estimation table (Table D.3).
//!
//! Annex K's tables are examples ("not necessarily suitable for any
//! particular application"); the encoder starts from them. The AC Huffman
//! tables were derived from the codewords listed in Tables K.5 and K.6 (each
//! symbol's position in HUFFVAL is its codeword's rank by length, then
//! value), and `tests/tables.rs` regenerates every codeword by the
//! procedure of Annex C and checks it against the spec's listing.

/// `ZIGZAG[k]` is the natural (row-major) index of the coefficient at
/// zig-zag position `k` (Figure A.6).
pub(crate) const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Table K.1, the example luminance quantisation table, in natural order.
pub const LUMA_QUANT: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, //
    12, 12, 14, 19, 26, 58, 60, 55, //
    14, 13, 16, 24, 40, 57, 69, 56, //
    14, 17, 22, 29, 51, 87, 80, 62, //
    18, 22, 37, 56, 68, 109, 103, 77, //
    24, 35, 55, 64, 81, 104, 113, 92, //
    49, 64, 78, 87, 103, 121, 120, 101, //
    72, 92, 95, 98, 112, 100, 103, 99, //
];

/// Table K.2, the example chrominance quantisation table, in natural order.
pub const CHROMA_QUANT: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, //
    18, 21, 26, 66, 99, 99, 99, 99, //
    24, 26, 56, 99, 99, 99, 99, 99, //
    47, 66, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
];

/// Table K.3: luminance DC differences. BITS (codes of each length 1–16).
pub(crate) const LUMA_DC_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
/// Table K.3: HUFFVAL.
pub(crate) const LUMA_DC_VALUES: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
/// Table K.4: chrominance DC differences.
pub(crate) const CHROMA_DC_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
/// Table K.4: HUFFVAL.
pub(crate) const CHROMA_DC_VALUES: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
/// Table K.5: luminance AC coefficients.
pub(crate) const LUMA_AC_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 125];
/// Table K.5: HUFFVAL.
#[rustfmt::skip]
pub(crate) const LUMA_AC_VALUES: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14,
    0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09,
    0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a,
    0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65,
    0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88,
    0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9,
    0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca,
    0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea,
    0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
];
/// Table K.6: chrominance AC coefficients.
pub(crate) const CHROMA_AC_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 119];
/// Table K.6: HUFFVAL.
#[rustfmt::skip]
pub(crate) const CHROMA_AC_VALUES: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13, 0x22, 0x32,
    0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0, 0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16,
    0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39,
    0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64,
    0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86,
    0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
    0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8,
    0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9,
    0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
];

/// Table D.3: (Qe_Value, Next_Index_LPS, Next_Index_MPS, Switch_MPS), by index.
#[rustfmt::skip]
pub(crate) const QE_TABLE: [(u16, u8, u8, bool); 113] = [
    (0x5A1D, 1, 1, true), // 0
    (0x2586, 14, 2, false), // 1
    (0x1114, 16, 3, false), // 2
    (0x080B, 18, 4, false), // 3
    (0x03D8, 20, 5, false), // 4
    (0x01DA, 23, 6, false), // 5
    (0x00E5, 25, 7, false), // 6
    (0x006F, 28, 8, false), // 7
    (0x0036, 30, 9, false), // 8
    (0x001A, 33, 10, false), // 9
    (0x000D, 35, 11, false), // 10
    (0x0006, 9, 12, false), // 11
    (0x0003, 10, 13, false), // 12
    (0x0001, 12, 13, false), // 13
    (0x5A7F, 15, 15, true), // 14
    (0x3F25, 36, 16, false), // 15
    (0x2CF2, 38, 17, false), // 16
    (0x207C, 39, 18, false), // 17
    (0x17B9, 40, 19, false), // 18
    (0x1182, 42, 20, false), // 19
    (0x0CEF, 43, 21, false), // 20
    (0x09A1, 45, 22, false), // 21
    (0x072F, 46, 23, false), // 22
    (0x055C, 48, 24, false), // 23
    (0x0406, 49, 25, false), // 24
    (0x0303, 51, 26, false), // 25
    (0x0240, 52, 27, false), // 26
    (0x01B1, 54, 28, false), // 27
    (0x0144, 56, 29, false), // 28
    (0x00F5, 57, 30, false), // 29
    (0x00B7, 59, 31, false), // 30
    (0x008A, 60, 32, false), // 31
    (0x0068, 62, 33, false), // 32
    (0x004E, 63, 34, false), // 33
    (0x003B, 32, 35, false), // 34
    (0x002C, 33, 9, false), // 35
    (0x5AE1, 37, 37, true), // 36
    (0x484C, 64, 38, false), // 37
    (0x3A0D, 65, 39, false), // 38
    (0x2EF1, 67, 40, false), // 39
    (0x261F, 68, 41, false), // 40
    (0x1F33, 69, 42, false), // 41
    (0x19A8, 70, 43, false), // 42
    (0x1518, 72, 44, false), // 43
    (0x1177, 73, 45, false), // 44
    (0x0E74, 74, 46, false), // 45
    (0x0BFB, 75, 47, false), // 46
    (0x09F8, 77, 48, false), // 47
    (0x0861, 78, 49, false), // 48
    (0x0706, 79, 50, false), // 49
    (0x05CD, 48, 51, false), // 50
    (0x04DE, 50, 52, false), // 51
    (0x040F, 50, 53, false), // 52
    (0x0363, 51, 54, false), // 53
    (0x02D4, 52, 55, false), // 54
    (0x025C, 53, 56, false), // 55
    (0x01F8, 54, 57, false), // 56
    (0x01A4, 55, 58, false), // 57
    (0x0160, 56, 59, false), // 58
    (0x0125, 57, 60, false), // 59
    (0x00F6, 58, 61, false), // 60
    (0x00CB, 59, 62, false), // 61
    (0x00AB, 61, 63, false), // 62
    (0x008F, 61, 32, false), // 63
    (0x5B12, 65, 65, true), // 64
    (0x4D04, 80, 66, false), // 65
    (0x412C, 81, 67, false), // 66
    (0x37D8, 82, 68, false), // 67
    (0x2FE8, 83, 69, false), // 68
    (0x293C, 84, 70, false), // 69
    (0x2379, 86, 71, false), // 70
    (0x1EDF, 87, 72, false), // 71
    (0x1AA9, 87, 73, false), // 72
    (0x174E, 72, 74, false), // 73
    (0x1424, 72, 75, false), // 74
    (0x119C, 74, 76, false), // 75
    (0x0F6B, 74, 77, false), // 76
    (0x0D51, 75, 78, false), // 77
    (0x0BB6, 77, 79, false), // 78
    (0x0A40, 77, 48, false), // 79
    (0x5832, 80, 81, true), // 80
    (0x4D1C, 88, 82, false), // 81
    (0x438E, 89, 83, false), // 82
    (0x3BDD, 90, 84, false), // 83
    (0x34EE, 91, 85, false), // 84
    (0x2EAE, 92, 86, false), // 85
    (0x299A, 93, 87, false), // 86
    (0x2516, 86, 71, false), // 87
    (0x5570, 88, 89, true), // 88
    (0x4CA9, 95, 90, false), // 89
    (0x44D9, 96, 91, false), // 90
    (0x3E22, 97, 92, false), // 91
    (0x3824, 99, 93, false), // 92
    (0x32B4, 99, 94, false), // 93
    (0x2E17, 93, 86, false), // 94
    (0x56A8, 95, 96, true), // 95
    (0x4F46, 101, 97, false), // 96
    (0x47E5, 102, 98, false), // 97
    (0x41CF, 103, 99, false), // 98
    (0x3C3D, 104, 100, false), // 99
    (0x375E, 99, 93, false), // 100
    (0x5231, 105, 102, false), // 101
    (0x4C0F, 106, 103, false), // 102
    (0x4639, 107, 104, false), // 103
    (0x415E, 103, 99, false), // 104
    (0x5627, 105, 106, true), // 105
    (0x50E7, 108, 107, false), // 106
    (0x4B85, 109, 103, false), // 107
    (0x5597, 110, 109, false), // 108
    (0x504F, 111, 107, false), // 109
    (0x5A10, 110, 111, true), // 110
    (0x5522, 112, 109, false), // 111
    (0x59EB, 112, 111, true), // 112
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::huffman::TableSpec;

    /// Every code word Annex C generates from BITS and HUFFVAL is the one
    /// T.81 lists in Tables K.3 to K.6.
    #[test]
    fn annex_k_huffman_tables_generate_the_listed_code_words() {
        let listing = include_str!("../tests/data/annex_k_codewords.txt");
        let tables = [
            ("K3", TableSpec::new(&LUMA_DC_BITS, &LUMA_DC_VALUES)),
            ("K4", TableSpec::new(&CHROMA_DC_BITS, &CHROMA_DC_VALUES)),
            ("K5", TableSpec::new(&LUMA_AC_BITS, &LUMA_AC_VALUES)),
            ("K6", TableSpec::new(&CHROMA_AC_BITS, &CHROMA_AC_VALUES)),
        ];
        let mut checked = 0;
        for (name, spec) in &tables {
            let (codes, all_ones) = spec.codes().unwrap();
            assert!(!all_ones, "{name} uses an all-ones code");
            let listed: Vec<(u8, &str)> = listing
                .lines()
                .filter(|l| l.starts_with(name))
                .map(|l| {
                    let mut f = l.split_whitespace().skip(1);
                    (
                        u8::from_str_radix(f.next().unwrap(), 16).unwrap(),
                        f.next().unwrap(),
                    )
                })
                .collect();
            assert_eq!(listed.len(), spec.values.len(), "{name}");
            for (sym, word) in listed {
                let i = spec.values.iter().position(|&v| v == sym).unwrap();
                let (len, code) = codes[i];
                assert_eq!(
                    format!("{code:0width$b}", width = usize::from(len)),
                    word,
                    "{name} symbol {sym:02X}"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 12 + 12 + 162 + 162);
    }

    #[test]
    fn zigzag_is_a_permutation_that_walks_the_antidiagonals() {
        let mut seen = [false; 64];
        for (k, &n) in ZIGZAG.iter().enumerate() {
            assert!(!seen[n]);
            seen[n] = true;
            if k > 0 {
                let (a, b) = (ZIGZAG[k - 1], n);
                let (ra, ca, rb, cb) = (a / 8, a % 8, b / 8, b % 8);
                assert!((ra + ca).abs_diff(rb + cb) <= 1);
            }
        }
    }

    /// Table D.3: the indices point inside the table, the switch is set
    /// exactly where Qe is about one half of 0x8000 * 4/3 (the states that
    /// start a new estimation chain), and the Qe values are 15-bit.
    #[test]
    fn qe_table_is_closed() {
        for (i, &(qe, nlps, nmps, _)) in QE_TABLE.iter().enumerate() {
            assert!(qe < 0x8000 && qe > 0, "{i}");
            assert!(
                usize::from(nlps) < QE_TABLE.len() && usize::from(nmps) < QE_TABLE.len(),
                "{i}"
            );
        }
        let switches: Vec<usize> = QE_TABLE
            .iter()
            .enumerate()
            .filter(|e| e.1.3)
            .map(|e| e.0)
            .collect();
        assert_eq!(switches, vec![0, 14, 36, 64, 80, 88, 95, 105, 110, 112]);
    }
}
