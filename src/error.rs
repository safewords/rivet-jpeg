//! The crate's one error type.

/// What went wrong. Every malformed input comes back as one of these; the
/// decoder never panics on bytes it is given.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The data breaks the syntax or the semantics of T.81: a marker segment
    /// of the wrong length, a parameter out of range, a table that is not
    /// defined when a scan needs it, a Huffman code that matches nothing.
    #[error("invalid JPEG data: {0}")]
    Invalid(String),
    /// Valid JPEG this crate does not implement, named: the hierarchical
    /// processes (DHP, EXP and the differential frames).
    #[error("unsupported JPEG feature: {0}")]
    Unsupported(String),
    /// The data ends before the first scan has started, so there is no
    /// picture to return at all. (Data that ends inside or after a scan is
    /// decoded as far as it goes, and the result says so.)
    #[error("truncated JPEG data: {0}")]
    Truncated(String),
    /// A request the caller made that cannot be met: an encoder setting out
    /// of range, a pixel buffer of the wrong size, a picture larger than a
    /// JPEG frame can describe.
    #[error("invalid JPEG configuration: {0}")]
    Config(String),
    /// The image is larger than the limit the caller set.
    #[error("the image is {width}x{height}, over the limit of {limit} pixels")]
    TooLarge {
        /// Width in samples, from the frame header.
        width: u32,
        /// Height in lines, from the frame header (or the DNL segment).
        height: u32,
        /// The limit that was set, in pixels.
        limit: u64,
    },
}

pub(crate) fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}

pub(crate) fn unsupported(msg: impl Into<String>) -> Error {
    Error::Unsupported(msg.into())
}

pub(crate) fn config(msg: impl Into<String>) -> Error {
    Error::Config(msg.into())
}

/// `Result` with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
