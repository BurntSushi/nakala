/// An error that occurs when reading a Nakala index.
///
/// A format error cannot be inspected for any detail other than its human
/// readable message. In general, a format error indicates a corrupt index or a
/// bug in Nakala. If it occurs due to corruption, the index probably needs to
/// be re-generated.
#[derive(Debug)]
pub struct FormatError(Box<FormatErrorInner>);

/// The internal representation of a format error.
///
/// It's essentially a simple version anyhow::Error. It allows nesting and
/// arbitrary chaining to make error composition while reading an index easy.
#[derive(Debug)]
enum FormatErrorInner {
    None,
    Cons {
        head: Box<dyn std::error::Error + Send + Sync + 'static>,
        tail: FormatError,
    },
}

impl std::error::Error for FormatError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self.0 {
            FormatErrorInner::None => None,
            FormatErrorInner::Cons { ref tail, .. } => Some(tail),
        }
    }
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match *self.0 {
            // It's not possible to reach this because it's impossible to
            // create a None format error from its public API, and the `source`
            // method will return a proper Option<FormatError>::None value
            // instead of a None format error.
            FormatErrorInner::None => unreachable!(),
            FormatErrorInner::Cons { ref head, .. } => head.fmt(f),
        }
    }
}

impl FormatError {
    /// Create a new format error with the given message.
    pub(crate) fn msg<S: Into<String>>(msg: S) -> FormatError {
        FormatError::none().context(msg)
    }

    /// Create a new format error from another underlying error.
    pub(crate) fn err<E>(err: E) -> FormatError
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        FormatError::none().wrap(err)
    }

    /// Create a new "none" error. This is only used internally to form a
    /// chain.
    fn none() -> FormatError {
        FormatError(Box::new(FormatErrorInner::None))
    }

    /// Wrap this error in the one given.
    pub(crate) fn wrap<E>(self, err: E) -> FormatError
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        FormatError(Box::new(FormatErrorInner::Cons {
            head: Box::new(err),
            tail: self,
        }))
    }

    /// Contextualize this error with a message.
    pub(crate) fn context<S: Into<String>>(self, msg: S) -> FormatError {
        FormatError(Box::new(FormatErrorInner::Cons {
            head: msg.into().into(),
            tail: self,
        }))
    }
}

pub trait FormatContext<T, E> {
    fn context<S: Into<String>>(self, msg: S) -> Result<T, FormatError>;
}

impl<T, E> FormatContext<T, E> for Result<T, E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn context<S: Into<String>>(self, msg: S) -> Result<T, FormatError> {
        self.map_err(|e| FormatError::err(e).context(msg))
    }
}
