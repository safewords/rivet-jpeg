# Provenance

Where every part of this crate came from. The short version: the code is
this repository's own, written from the Recommendations; the tables were
transcribed from ITU-T T.81 itself; and another JPEG implementation was
used only as a black box, offline, to make and render test files.

## Clean-room rules

- **No JPEG implementation's source was opened or read**, in either half:
  not libjpeg (IJG), libjpeg-turbo, mozjpeg, stb_image, jpeg-decoder,
  zune-jpeg, the `image` crate, `jpeg-encoder`, FFmpeg's mjpeg code, or any
  other. No implementation was searched for.
- **libjpeg-turbo 3.1.0 was used only as a binary library**, through the
  `imagecodecs` and Pillow Python wheels in a scratch virtual environment:
  to encode the files in `tests/corpus/gen` and render references for them
  (`tools/make_corpus.py`), and to decode this crate's encoder output for
  comparison (`tools/check_encoder.py`). Only their documented call
  signatures were looked at. (Docker, for running `cjpeg`/`djpeg`, did not
  respond on the machine.) FFmpeg was not used at all.
- No test runs another implementation: the references are committed data.
- The IJG and libjpeg-turbo test images were fetched as individual data
  files by name (the directory's file list was read through GitHub's API;
  no source file was opened).

## Sources

| part | source |
|---|---|
| Marker syntax, frame and scan headers, table segments, DNL, DRI, DAC | T.81 Annex B |
| Huffman table generation and decoding | T.81 Annex C; F.2.2.3 (DECODE, with MAXCODE / VALPTR) |
| Sequential DCT, Huffman | T.81 F.1.2, F.2.2; 12-bit: F.1.5, F.2.5 |
| Sequential DCT, arithmetic | T.81 F.1.4, F.2.4 (statistical models of Tables F.4, F.5) |
| Progressive DCT | T.81 Annex G: G.1.2 (Huffman, EOB runs and the correction bits of Figure G.7), G.1.3 (arithmetic, Figures G.10, G.11, Table G.2) |
| Lossless | T.81 Annex H (predictors of Table H.1, Table H.2, the two-dimensional model of H.1.2.3 and Figure H.2) |
| Arithmetic coder | T.81 Annex D: D.1 (encoder, Figures D.1–D.15), D.2 (decoder, Figures D.16–D.22), Table D.3 |
| Point transform | T.81 A.4 |
| DCT definition, level shift | T.81 A.3 |
| Restart handling and resynchronisation | T.81 E.2.4, F.2.4.4 |
| Optimal Huffman tables | T.81 K.2 (Figures K.1–K.4) |
| Quantisation and Huffman example tables | T.81 K.1 (Tables K.1, K.2), K.3 (Tables K.3–K.6) |
| YCbCr, chroma siting, APP0 | ITU-T T.871 (JFIF) |
| APP1 EXIF, orientation tag | CIPA DC-008 (EXIF 2.32), TIFF 6.0 IFD structure |
| APP2 ICC chunks | ICC.1, Annex B (embedding in JPEG) |
| APP14 colour transform | Adobe Technical Note 5116 (the DCT filters) |
| IDCT accuracy test | IEEE Std 1180-1990, as recalled |
| Quality scaling of the example tables | the widely published convention (50/q below 50, (100−q)/50 above), from general knowledge |

The copy of T.81 used is the ITU's 09/92 text as hosted by the W3C
(`https://www.w3.org/Graphics/JPEG/itu-t81.pdf`).

## How the tables were transcribed

- **Table D.3** (113 rows): typed from the rendered page, then checked row
  by row against the PDF's text layer (whose numbers carry a stray leading
  `1` from the typesetting; a value matched when equal with or without it).
  All 113 rows matched.
- **Tables K.5 and K.6**: BITS and HUFFVAL were derived from the code
  words the Recommendation lists (each symbol's place in HUFFVAL is its
  code word's rank by length, then value), and the derivation was checked
  to be canonical by Annex C. The listing itself is in
  `tests/data/annex_k_codewords.txt`, and a unit test regenerates every
  code word from the tables.
- **Tables K.1–K.4**: typed from the rendered pages; K.3 and K.4's code
  words are in the same listing and test.

## Choices T.81 leaves open, and what this crate does

- **Upsampling** is outside T.81. This crate interpolates linearly with
  samples centred as T.871 sites them, for any ratio. libjpeg-turbo agrees
  within 3 levels at 2:1 ratios and replicates at 4:1 (the 4:1:1 file
  differs by up to 38 levels at chroma edges).
- **Colour of 3-component files without JFIF or Adobe segments**: YCbCr,
  unless the component identifiers are `R`, `G`, `B` (then RGB); a lossless
  file with neither segment is converted as RGB.
- **CMYK and YCCK**: with an Adobe segment the values are taken as stored
  inverted (0 is full ink), as Adobe applications write them; YCCK's first
  three components invert to the complement of C, M and Y. libjpeg-turbo's
  black-box CMYK output for both kinds agreed within 3 levels, which
  settled the YCCK convention.
- **Optimal tables when counts are extreme**: K.2 assumes no code longer
  than 32 bits; if one would be, the counts are halved (keeping every used
  symbol) and the table rebuilt.
- **Arithmetic coder flush**: the encoder writes any 0xFF bytes still
  stacked at the end (as FF 00) before discarding trailing zeros, which is
  always decodable.
