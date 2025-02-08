use std::convert::Infallible;
use std::fmt::{self, Display};

pub type Result<I, E = Error> = ::std::result::Result<I, E>;

/// reva error type
///
/// # Feature Interaction
///
/// If the feature `serde_json` is enabled an
/// additional error variant `Json` is added.
///
/// # Why not `failure`/`error-chain`?
///
/// Error from `error-chain` are not `Sync` which
/// can lead to problems e.g. when this is used
/// by a crate which use `failure`. Implementing
/// `Fail` on the other hand prevents the implementation
/// of `std::error::Error` until specialization lands
/// on stable. While errors impl. `Fail` can be
/// converted to a type impl. `std::error::Error`
/// using a adapter the benefits `failure` would
/// bring to this crate are small, which is why
/// `std::error::Error` was used.
///
#[non_exhaustive]
#[derive(Debug)]
pub enum Error {
    /// formatting error
    Fmt(fmt::Error),

    /// an error raised by using `?` in a template
    Custom(Box<dyn std::error::Error + Send + Sync>),
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Error::Fmt(ref err) => Some(err),
            Error::Custom(ref err) => Some(err.as_ref()),
        }
    }
}

impl Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Fmt(err) => write!(formatter, "formatting error: {err}"),
            Error::Custom(err) => write!(formatter, "{err}"),
        }
    }
}

impl From<fmt::Error> for Error {
    fn from(err: fmt::Error) -> Self {
        Error::Fmt(err)
    }
}

impl From<Infallible> for Error {
    #[inline]
    fn from(value: Infallible) -> Self {
        match value {}
    }
}

#[cfg(test)]
mod tests {
    use super::Error;

    #[allow(dead_code)]
    trait AssertSendSyncStatic: Send + Sync + 'static {}
    impl AssertSendSyncStatic for Error {}
}
