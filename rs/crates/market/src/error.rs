#![forbid(unsafe_code)]
//! Typed market errors. Rejections (guard rails) are caller-visible
//! outcomes, not crashes; everything else carries context.

/// Market failure with enough context to tell a caller bug from bad input.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// Guard rail tripped (band, coverage, cap): normal rejection.
    Rejected(String),
    /// Unknown order, deal or item id.
    NotFound(String),
    /// Illegal state move or inconsistent value.
    Invalid(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Rejected(detail) => write!(formatter, "market rejected: {detail}"),
            Error::NotFound(detail) => write!(formatter, "market not found: {detail}"),
            Error::Invalid(detail) => write!(formatter, "market invalid: {detail}"),
        }
    }
}

impl std::error::Error for Error {}
