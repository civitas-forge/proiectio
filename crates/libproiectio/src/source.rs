use camino::Utf8Path;

use crate::{Desired, Error, Result, load_archive, load_tree};

pub fn load_source(source: &Utf8Path, strip: Option<u32>) -> Result<Desired> {
    let source = crate::absolutize(source)?;
    let meta = source.metadata().map_err(|error| Error::Io {
        path: source.clone(),
        source: error,
    })?;
    if meta.is_dir() {
        return match strip {
            Some(_) => Err(Error::StripOnDirectory { path: source }),
            None => load_tree(&source),
        };
    }
    if !meta.is_file() {
        return Err(Error::TreeNodeKind { path: source });
    }
    load_archive(&source, strip.unwrap_or(0))
}

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;
