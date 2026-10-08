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
}
