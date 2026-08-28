use camino::{Utf8Path, Utf8PathBuf};

/// A destination directory paired with the state directory holding its
/// manifest.
///
/// This is the engine's handle: `plan`, `apply`, and `status` land on it as
/// the observe/decide/act stages are implemented. Both paths are absolute
/// and caller-chosen; the crate never consults the current directory.
///
/// `state_dir` may lie inside `target`. The projection's own state subtree
/// is excluded from classification — the manifest never reads as foreign —
/// and a desired path entering it is refused as
/// [`Containment`](crate::Error::Containment).
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

    /// The state directory's path relative to the target, when it lies
    /// inside the target — the in-dest state prefix that
    /// [`decide`](crate::decide) and [`classify`](crate::classify) take:
    /// the subtree under it never classifies, and a desired path entering
    /// it refuses as [`Containment`](crate::Error::Containment).
    ///
    /// `None` when the state directory lives outside the target — nothing
    /// in the destination is the projection's own state, so nothing is
    /// excluded. A state directory *equal* to the target also yields
    /// `None`: there is no proper subtree to exclude, and excluding the
    /// whole destination would classify nothing at all.
    pub fn state_prefix(&self) -> Option<&Utf8Path> {
        match self.state_dir.strip_prefix(&self.target) {
            Ok(prefix) if !prefix.as_str().is_empty() => Some(prefix),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "projection_tests.rs"]
mod tests;
