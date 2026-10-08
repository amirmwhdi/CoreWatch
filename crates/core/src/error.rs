use std::fmt;
use std::path::PathBuf;

/// Why a collector could not produce its data.
///
/// Collectors return this instead of panicking; the sampler logs it and the
/// other modules keep working.
#[derive(Debug, thiserror::Error)]
pub enum CollectError {
    #[error("cannot read {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot parse {what}: {detail}")]
    Parse { what: &'static str, detail: String },
}

impl CollectError {
    pub(crate) fn parse(what: &'static str, detail: impl Into<String>) -> Self {
        Self::Parse {
            what,
            detail: detail.into(),
        }
    }

    /// Stable, machine-readable category for logs and bug reports.
    ///
    /// Never rename an existing value: reports are grouped by it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Io { source, .. } => match source.kind() {
                std::io::ErrorKind::NotFound => "io.not_found",
                std::io::ErrorKind::PermissionDenied => "io.permission",
                _ => "io.other",
            },
            Self::Parse { .. } => "parse",
        }
    }
}

/// Displays an error followed by every `source()` in its chain, joined with
/// ": ", so the root cause is never lost in a log line.
pub struct ErrorChain<'a>(pub &'a (dyn std::error::Error + 'static));

impl fmt::Display for ErrorChain<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)?;
        let mut source = self.0.source();
        while let Some(cause) = source {
            // thiserror already prints `{source}` inside Io's message; skip an
            // exact repeat so the line stays readable.
            let text = cause.to_string();
            if !self.0.to_string().ends_with(&text) {
                write!(f, ": {text}")?;
            }
            source = cause.source();
        }
        Ok(())
    }
}

impl CollectError {
    /// This error and its whole cause chain, for logging.
    pub fn chain(&self) -> ErrorChain<'_> {
        ErrorChain(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn io_error(kind: io::ErrorKind) -> CollectError {
        CollectError::Io {
            path: "/proc/stat".into(),
            source: io::Error::new(kind, "boom"),
        }
    }

    #[test]
    fn io_kinds_are_stable() {
        assert_eq!(io_error(io::ErrorKind::NotFound).kind(), "io.not_found");
        assert_eq!(
            io_error(io::ErrorKind::PermissionDenied).kind(),
            "io.permission"
        );
        assert_eq!(io_error(io::ErrorKind::Interrupted).kind(), "io.other");
    }

    #[test]
    fn parse_kind_is_stable() {
        assert_eq!(CollectError::parse("/proc/stat", "x").kind(), "parse");
    }

    #[test]
    fn chain_names_the_path_and_the_cause_once() {
        let text = io_error(io::ErrorKind::NotFound).chain().to_string();
        assert_eq!(text, "cannot read /proc/stat: boom");
    }
}
