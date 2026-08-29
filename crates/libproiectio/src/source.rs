use camino::Utf8Path;

use crate::{Desired, Error, Result, load_archive, load_tree};

pub fn load_source(path: &Utf8Path, strip: Option<u32>) -> Result<Desired> {
    let path = crate::absolutize(path)?;
    if path.metadata().is_ok_and(|meta| meta.is_dir()) {
        return match strip {
            Some(_) => Err(Error::StripOnDirectory { path }),
            None => load_tree(&path),
        };
    }
    load_archive(&path, strip.unwrap_or(0))
}

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;
