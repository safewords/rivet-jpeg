# Test corpus

JPEG files from elsewhere, used as data only: `tests/corpus.rs` decodes each
and compares it with a reference rendering where one exists. No other
implementation runs in the tests. Not part of the published crate.

## Public files

| file | SHA-256 | source | licence |
|---|---|---|---|
| `ijg-testorig.jpg` | `acc6ec555d41d15b368320edaa3b20958ee6fa97cb6e4a18d1213d5ae8bec73b` | IJG test image, as distributed in libjpeg-turbo's `testimages/` (fetched as a data file by name, 2026-10-02) | IJG licence |
| `ijg-testorig.ppm` | `4afe49cb62ba87be1a958d7fd29b822a2ba1a0e966d1136f616ee5353691a002` | the same; the IJG decoder's rendering of `testorig.jpg` (the reference) | IJG licence |
| `ijg-testimgint.jpg` | `491679b8057739b3c8e5bacd1e918efb1691d271cbbd69820ff8d480dcb90963` | the same; baseline, Huffman | IJG licence |
| `ijg-testimgari.jpg` | `4672c7f08864cd0a8c73a4fa4b66ca32b635d38464551c1ecf06564ae8c89b38` | the same; **arithmetic-coded** (SOF9), the same coefficients as `testimgint.jpg` | IJG licence |
| `ijg-monkey12.jpg` | `a3cb218412cecfa877a4adee520aa3d58ac0d6d0f36a7a2541e598324ce481e3` | libjpeg-turbo `testimages/`; **12-bit** extended sequential, with the ICC sRGB2014 profile | BSD-3-Clause (libjpeg-turbo); the profile, ICC (free to copy) |
| `dng-canon-5d3-lossless14.jpg` | `3c3487a3a50d23cc028139341a2d93ed9738c00c08ecc4c316b6b89dee5a8b90` | the raw tile of raw.pixls.us "Canon - EOS 5D Mark III - 14bit 14bit (2.3471882640587).dng" (file SHA-256 `1d77dcc6…b603e`), bytes 1184–1538164: **lossless** (SOF3), 14-bit | CC0 |
| `dng-blackmagic-ext12.jpg` | `2e2a613e29309c729b686291ae0376346080418a0ffe8dfc41b68b6415c85781` | the first JPEG stream of raw.pixls.us "Blackmagic - Micro Cinema Camera - 12bit (16:9).dng" (file SHA-256 `4c65b8cd…5b277`): 12-bit DCT, grey | CC0 |
| `dng-adobe-lossy-tile.jpg` | `bc827911839a84e0c47e54bc2e7c170a967fb545bbfe2821109ed4506778a543` | the first tile of raw.pixls.us "Adobe DNG Converter - Canon EOS 5D Mark III - Lossy JPEG compression, rgb (3:2).DNG" (file SHA-256 `b22f1e36…6a98`): baseline, restart interval 62 | CC0 |

The ITU-T T.83 compliance test data were not obtainable: the ITU sells T.83
(with its three diskettes of test data) and does not offer it for free.

No public CMYK or YCCK JPEG with a published licence turned up on
Wikimedia Commons (144 candidate files were checked for a four-component
frame); those cases are covered by the generated files below.

## Generated files (`gen/`)

Made by [`tools/make_corpus.py`](../../tools/make_corpus.py) with
libjpeg-turbo 3.1.0 used as a black box through the `imagecodecs` and
Pillow wheels (Docker, for `cjpeg`/`djpeg`, was unavailable on the machine).
Each `<name>.jpg` has a `<name>.ref`: libjpeg-turbo's own decode of the
file, or, for the lossless files, the exact source samples. `MANIFEST.txt`
lists the SHA-256 of each file and how it was made. The sources are
`ijg-testorig.ppm` and synthetic gradients.

They cover: 4:4:4, 4:2:2, 4:2:0, 4:4:0, 4:1:1; quality 5 to 100;
optimised tables; grey; progressive (colour and grey); restart intervals
in sequential and progressive files; Adobe RGB, CMYK and YCCK; 12-bit
colour and grey; lossless at 8, 12 and 16 bits with all seven predictors,
grey and RGB; EXIF orientation 6 with a 150 000-byte ICC profile in three
APP2 chunks.
