//! A JPEG decoder and encoder, written from ITU-T T.81 | ISO/IEC 10918-1,
//! T.871 (JFIF), and the EXIF, ICC and Adobe APP-segment conventions.
//!
//! **Decoding** handles every non-hierarchical process of T.81: baseline
//! and extended sequential DCT at 8 and 12 bits, progressive DCT (spectral
//! selection and successive approximation), and lossless (2–16 bits), each
//! with Huffman or arithmetic coding; any sampling factors 1–4; restart
//! intervals, with resynchronisation on the marker numbers; DNL. The result
//! is the components as stored ([`Image::planes`]), which
//! [`Image::to_rgb8`] and its siblings upsample and colour-convert (YCbCr,
//! RGB, CMYK and YCCK with Adobe's APP14 transform flag). Metadata comes
//! back in [`Info`]: JFIF, Adobe, EXIF and its orientation (reported, not
//! applied), the ICC profile reassembled from its chunks, XMP, comments.
//! A file that ends early, or breaks, decodes as far as it goes, with
//! [`Image::complete`] false.
//!
//! **Encoding** ([`encode`]) writes baseline or progressive JPEG from 8-bit
//! RGB, RGBA or grey, at 4:4:4, 4:2:2, 4:2:0 or 4:4:0, with quality-scaled
//! Annex K quantisation tables, the Annex K Huffman tables or optimised
//! ones (Annex K.2), restart intervals, and JFIF, EXIF and ICC segments.
//!
//! ```
//! let rgb = vec![200u8; 16 * 16 * 3];
//! let settings = jpeg::EncodeSettings { quality: 90, ..Default::default() };
//! let file = jpeg::encode(&rgb, 16, 16, jpeg::PixelFormat::Rgb, &settings).unwrap();
//! let img = jpeg::decode(&file).unwrap();
//! assert_eq!((img.info.width, img.info.height), (16, 16));
//! let pixels = img.to_rgb8();
//! assert!(pixels.iter().all(|&v| v.abs_diff(200) <= 2));
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod arith;
mod bits;
mod convert;
pub mod dct;
mod decode;
mod encode;
mod error;
mod huffman;
mod metadata;
mod tables;

pub use decode::{
    Coding, ColourSpace, Component, DecodeOptions, Image, Info, Plane, Process, decode, decode_with, read_info,
};
pub use encode::{EncodeSettings, PixelFormat, Subsampling, encode};
pub use error::{Error, Result};
pub use metadata::{Adobe, Jfif, Orientation};
pub use tables::{CHROMA_QUANT, LUMA_QUANT};
