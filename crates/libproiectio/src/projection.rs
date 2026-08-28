use camino::{Utf8Path, Utf8PathBuf};

/// A destination directory paired with the state directory holding its
/// manifest.
///
/// This is the engine's handle: `plan`, `apply`, and `status` land on it as
/// the observe/decide/act stages are implemented. Both paths are absolute
/// and caller-chosen; the crate never consults the current directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    target: Utf8PathBuf,
    state_dir: Utf8PathBuf,
}

impl Projection {
    /// A projection writing into `target`, with its manifest kept in
    /// `state_dir`.
    ///
    /// # Panics
    ///
    /// Panics if either path is relative: the crate never consults the
    /// current directory, so a relative path here has no meaning it could
    /// honor.
    pub fn new(target: Utf8PathBuf, state_dir: Utf8PathBuf) -> Self {
        assert!(
            target.is_absolute(),
            "projection target must be absolute, got {target}"
        );
        assert!(
            state_dir.is_absolute(),
            "projection state_dir must be absolute, got {state_dir}"
        );
        Projection { target, state_dir }
    }

    /// The directory the projection writes into.
    pub fn target(&self) -> &Utf8Path {
        &self.target
    }

    /// The directory holding the manifest.
    pub fn state_dir(&self) -> &Utf8Path {
        &self.state_dir
    }
}

#[cfg(test)]
#[path = "projection_tests.rs"]
mod tests;
