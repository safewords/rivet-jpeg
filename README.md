# rivet-jpeg

[![CI](https://github.com/safewords/rivet-jpeg/actions/workflows/ci.yml/badge.svg)](https://github.com/safewords/rivet-jpeg/actions/workflows/ci.yml)

A **JPEG** decoder and encoder in Rust: no C, no system libraries, no build
script, nothing to install on a build host. Written from ITU-T T.81 |
ISO/IEC 10918-1, T.871 (JFIF) and the EXIF, ICC and Adobe APP-segment
conventions, not translated from any other implementation. The decoder
handles every non-hierarchical process T.81 defines, Huffman and
arithmetic; the encoder writes baseline and progressive files that another
decoder reads within a few levels of this one (the figures are
[below](#how-it-is-checked)).

Written for the **[rivet](https://github.com/safewords/rivet)**
transcoder's still-image path, where it replaces the `image` crate's JPEG
decoder and `jpeg-encoder`. Usable on its own by anything that has JPEG
bytes and wants pixels, or pixels and wants JPEG.

Published as `rivet-jpeg`; **imported as `jpeg`** (`use jpeg::…`). One
dependency (`thiserror`), no build script; `unsafe` only to call code
compiled for AVX2 once the processor is known to have it
([`src/simd.rs`](src/simd.rs)), which the `force-scalar` feature turns off.

```toml
[dependencies]
jpeg = { package = "rivet-jpeg", git = "https://github.com/safewords/rivet-jpeg", branch = "develop" }
```

## What it decodes

| | supported | refused with `Error::Unsupported` |
|---|---|---|
| **Processes** | baseline (SOF0); extended sequential, 8- and 12-bit (SOF1, SOF9); progressive, spectral selection and successive approximation, 8- and 12-bit (SOF2, SOF10); lossless, 2–16 bits, predictors 1–7, point transform (SOF3, SOF11) | the hierarchical mode (DHP, EXP, SOF5–7, SOF13–15) |
| **Entropy coding** | Huffman (up to four tables of each class); arithmetic, with DAC conditioning | — |
| **Layout** | 1–4 components (any count for the sequential processes), sampling factors 1–4 each way, interleaved and non-interleaved scans, any scan order | — |
| **Restarts** | DRI and RST0–7; a missing or misnumbered marker resynchronises on the marker's number, leaving the lost intervals grey | — |
| **Other syntax** | DNL (a frame header with 0 lines), fill bytes, COM, every APPn | — |
| **Colour** | grey; YCbCr (T.871); RGB (Adobe transform 0, or component ids `R` `G` `B`); CMYK and YCCK (Adobe transform 0 and 2, Adobe's inverted storage) | — |
| **Metadata** | JFIF, Adobe APP14, EXIF (and its orientation, *reported, not applied*), ICC profile reassembled from APP2 chunks, XMP, comments | — |

The result is the components as stored, at their own resolution
(`Image::planes`, `u16` samples at the frame's precision), plus conversions:
`to_rgb8`, `to_rgba8`, `to_rgb16`, `to_luma8`, `to_cmyk8` (ink amounts),
and `upsampled(i)` for one component at full resolution. Upsampling is
linear interpolation with each chroma sample centred on the pixels it
covers (T.871 siting), for any ratio of sampling factors; CMYK and YCCK
convert to RGB naively, without colour management.

**Broken files.** A file that ends early, or breaks inside a scan, decodes
as far as it goes: `Image::complete` is false, `Image::warnings` says what
happened, missing blocks are mid-grey, and a progressive picture keeps the
precision its delivered scans gave. Only a file that ends before its first
scan is an error (`Error::Truncated`). `DecodeOptions::strict` turns every
departure from T.81 into an error instead — data between segments, padding
that is not 1-bits, a misnumbered restart, a reserved all-1s Huffman code,
a progression that breaks G.1.1.1, missing EOI, bytes after it. The decoder
refuses frames over `DecodeOptions::max_pixels` (2^28 by default) before
allocating, and never panics on any input.

## What it encodes

8-bit RGB, RGBA (alpha ignored) or grey to:

- **baseline sequential** (SOF0), or **progressive** (SOF2) in ten scans —
  DC at reduced precision, a low luma band, chroma, the rest of luma, then
  refinements (six scans for grey);
- 4:4:4, 4:2:2, 4:2:0 or 4:4:0 chroma (box-filtered);
- quantisation from the Annex K example tables scaled by quality 1–100
  (50/q below 50, (100−q)/50 above, clamped to 1–255);
- the Annex K Huffman tables, or tables optimised for the picture (Annex
  K.2), one set per scan when progressive;
- restart markers every N MCUs;
- JFIF APP0, EXIF APP1, and an ICC profile in as many APP2 chunks as it
  needs;
- and, for testing decoders, arithmetic coding (SOF9, SOF10).

Every file it writes passes this crate's strict decoder.

## Using it

```rust
// Decoding: what rivet needs for a still image.
let img = jpeg::decode(&bytes)?;
let rgba = img.to_rgba8();                     // width * height * 4
let icc = img.info.icc_profile.as_deref();     // Option<&[u8]>
let orientation = img.info.orientation;        // Option<jpeg::Orientation>
let header = jpeg::read_info(&bytes)?;         // headers only, no decode

// Encoding: a web JPEG.
let settings = jpeg::EncodeSettings {
    quality: 82,
    subsampling: jpeg::Subsampling::S420,
    progressive: true,
    icc_profile: icc.map(<[u8]>::to_vec),
    ..Default::default()
};
let file = jpeg::encode(&rgb, width, height, jpeg::PixelFormat::Rgb, &settings)?;
```

## How it is checked

All of it runs in `cargo test`; nothing but this crate runs in the tests.

- **IDCT accuracy, IEEE 1180-1990** (`tests/dct.rs`), the test T.83 points
  decoders to: 10 000 random blocks in each of the ranges ±256, ±5 and
  ±300, both signs. Worst figures: peak error 1 (limit 1), per-position
  mean square error 0.0001 (limit 0.06), overall 0.000005 (limit 0.02),
  per-position mean error 0.0001 (limit 0.015), overall 0.000005 (limit
  0.0015); zero in, zero out. The FDCT agrees with the definition to
  4.2e-5.
- **Tables**: every code word of Tables K.3–K.6, regenerated from BITS and
  HUFFVAL by Annex C, matches the Recommendation's listing (348 code
  words); Table D.3 was transcribed and cross-checked against the
  Recommendation's text layer, all 113 rows.
- **Third-party files** ([`tests/corpus`](tests/corpus/README.md), sources
  and SHA-256 there). The IJG test images: `testorig.jpg` against the IJG
  decoder's own rendering, max difference 3, mean 0.22, 53.1 dB; the
  arithmetic-coded `testimgari.jpg` decodes to exactly the pixels of its
  Huffman-coded twin; the 12-bit `monkey12.jpg` with its ICC profile. CC0
  camera files from raw.pixls.us: a 14-bit lossless (SOF3) raw tile, a
  12-bit DCT tile, a lossy DNG tile with restarts — each decodes completely
  and ends exactly at its EOI.
- **Generated files with references** (31 files; libjpeg-turbo 3.1.0 used
  as a black box offline to make them and render them,
  `tools/make_corpus.py`): every lossless file (8, 12, 16 bits, all seven
  predictors, grey and RGB) reproduces its source **exactly**; the DCT files
  agree with libjpeg-turbo's rendering to at most 1 level (grey, CMYK,
  Adobe RGB), 3 (4:4:4, 4:2:0, 4:2:2, 4:4:0, progressive, restarts, YCCK)
  and 3/4096 (12-bit); 4:1:1 differs by up to 38 at chroma edges because
  this crate interpolates 4:1 chroma where libjpeg-turbo replicates it
  (luma agrees within 1). See the table below.
- **The encoder**: quality sweep (sizes and PSNR below); progressive,
  optimised, arithmetic and restart variants decode to exactly the same
  pixels as the baseline file (they carry the same coefficients); every
  output passes the strict decoder; metadata round trips (a 200 000-byte
  ICC profile in four chunks, EXIF orientation); sizes 1x1 to 300x2 in
  every subsampling and mode. Offline, libjpeg-turbo decoded 122 encoder
  outputs (every subsampling and mode, q 30–95, with and without restarts
  and a 100 kB ICC profile) without complaint, within 4 levels of this
  crate's decode (`tools/check_encoder.py`).
- **Malformed input**: truncation at 300 points in each of seven files
  (sequential, progressive, restarts, arithmetic, arithmetic progressive,
  lossless, 12-bit) — a picture from any length past the first scan
  header, never a panic; 2 800 corrupted files per run (105 000 in a
  longer debug run with overflow checks) — never a panic; a lost restart
  interval is skipped and the rest lands in place; strict mode refuses
  each kind of departure that lenient mode steps over.

Generated corpus against libjpeg-turbo's rendering (max and mean absolute
difference in levels):

| file | max | mean | | file | max | mean |
|---|---|---|---|---|---|---|
| baseline 4:4:4 q75 | 3 | 0.049 | | grey | 1 | 0.014 |
| baseline 4:2:2 | 3 | 0.153 | | progressive grey | 1 | 0.014 |
| baseline 4:2:0 | 3 | 0.089 | | Adobe RGB | 1 | 0.012 |
| baseline 4:4:0 | 3 | 0.152 | | Adobe CMYK | 1 | 0.014 |
| baseline 4:1:1 | 38 | 1.351 | | Adobe YCCK | 3 | 0.069 |
| q100 4:4:4 | 3 | 0.022 | | 12-bit colour (of 4095) | 3 | 0.115 |
| q5 4:2:0 | 2 | 0.015 | | 12-bit grey (of 4095) | 1 | 0.027 |
| progressive 4:2:0 | 3 | 0.096 | | lossless, 15 files | 0 | 0 |
| progressive 4:4:4 | 3 | 0.026 | | restarts every 5 MCUs | 3 | 0.096 |
| progressive with restarts | 3 | 0.096 | | EXIF + 3-chunk ICC | 3 | 0.096 |

Encoder, the IJG photograph (224x144 crop), bytes and PSNR (RGB):

| quality | 4:4:4 baseline | optimised | progressive | PSNR | 4:2:0 baseline | optimised | progressive | PSNR |
|---|---|---|---|---|---|---|---|---|
| 10 | 2 235 | 1 563 | 1 912 | 28.17 | 1 775 | 1 249 | 1 577 | 27.00 |
| 50 | 4 795 | 4 408 | 4 743 | 35.24 | 3 757 | 3 402 | 3 690 | 33.32 |
| 75 | 6 986 | 6 619 | 6 841 | 37.78 | 5 371 | 5 046 | 5 226 | 35.59 |
| 85 | 9 236 | 8 832 | 8 887 | 39.63 | 6 984 | 6 636 | 6 691 | 37.33 |
| 95 | 16 093 | 15 276 | 14 895 | 43.87 | 12 279 | 11 646 | 11 302 | 40.85 |
| 100 | 35 535 | 33 737 | 32 110 | 53.53 | 23 118 | 22 126 | 21 198 | 44.04 |

**Not checked against T.83.** ITU-T T.83 (the compliance tests) and its
test data are sold by the ITU, not published; they were not used. The IDCT
test above is the accuracy test T.83 relies on.

## Speed

Release build, Ryzen 9 9950X, three 1080x720 RGB frames of camera video
(2.33 megapixels in all), megapixels a second:

| | before | now |
|---|---|---|
| decode to RGB, 4:2:0 q85 | 51 | 162 |
| decode to RGB, 4:4:4 q95 | 37 | 89 |
| decode to RGB, progressive 4:2:0 | 46 | 106 |
| encode 4:2:0 q85, optimised tables | 45 | 171 |
| encode 4:4:4 q95, optimised tables | 25 | 150 |
| encode progressive 4:2:0 | 26 | 119 |

The transforms and the colour loops are written eight lanes at a time for
the compiler to vectorise (and built for AVX2 where the processor has it),
with the same operations in the same order as the one-at-a-time code, so
the output is identical to the last bit on any processor. The decoder
transforms all blocks after the entropy-coded data, in rows on several
threads, and converts to RGB in bands of lines; the encoder prepares rows
of MCUs, codes a sequential scan in pieces and a progression's scans on
several threads. Results do not depend on the number of threads.

## Provenance and licensing

Written from the Recommendations' text; **no JPEG implementation's source
was read** — not libjpeg, libjpeg-turbo, mozjpeg, stb_image, jpeg-decoder,
zune-jpeg, image, jpeg-encoder, FFmpeg or any other. libjpeg-turbo was used
only as a black box, offline, to make and render test files.
[docs/PROVENANCE.md](docs/PROVENANCE.md) records every source.

## License

Open Encoding Attribution License v1.0 — a source-available (not OSI open-source)
license, royalty-free, with a commercial-attribution requirement. See
[LICENSE.md](LICENSE.md) and [NOTICE](NOTICE).
