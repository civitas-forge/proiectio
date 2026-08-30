use camino::Utf8Path;

use crate::{Desired, Error, Limits, Result, load_archive, load_tree};

/// Loads whatever `source` names — a directory through
/// [`load_tree`], anything else through [`load_archive`] — under `limits`.
pub fn load_source(source: &Utf8Path, strip: Option<u32>, limits: Limits) -> Result<Desired> {
    let source = crate::absolutize(source)?;
    let meta = source.metadata().map_err(|error| Error::Io {
        path: source.clone(),
        source: error,
    })?;
    if meta.is_dir() {
        return match strip {
            Some(_) => Err(Error::StripOnDirectory { path: source }),
            None => load_tree(&source, limits),
        };
    }
    if !meta.is_file() {
        return Err(Error::TreeNodeKind { path: source });
    }
    load_archive(&source, strip.unwrap_or(0), limits)
}

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;
