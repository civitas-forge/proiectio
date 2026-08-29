use camino::{Utf8Path, Utf8PathBuf};

/// A destination directory paired with the state directory holding its
/// manifest.
///
/// This is the engine's handle: `plan`, `apply`, and `status` land on it as
/// the observe/decide/act stages are implemented. Both paths are absolute
/// and caller-chosen; the crate never consults the current directory.
///
/// `state_dir` may lie inside `target` (as a proper subdirectory, never
/// `target` itself). The projection's own state subtree is excluded from
/// classification — the manifest never reads as foreign — and a desired
/// path overlapping it is refused as
/// [`Containment`](crate::Error::Containment): one inside the subtree, and
/// one the state directory sits beneath.
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
    /// Panics if either path is relative — the crate never consults the
    /// current directory, so a relative path here has no meaning it could
    /// honor — or carries `..` components: this type reasons about the two
    /// paths lexically ([`state_prefix`](Projection::state_prefix) strips
    /// one from the other), and a `..` does not resolve lexically, so such
    /// a spelling could misplace the state subtree. (`.` components need
    /// no refusal: path comparison works component-wise and already treats
    /// them as absent.) Panics too if `state_dir` equals
    /// `target`: the state files would sit at the destination root with no
    /// subtree to exclude from classification, so the projection's own
    /// manifest would read as foreign. Keep the state in a subdirectory
    /// (the conventional `<target>/.proiectio`) or outside the target
    /// entirely.
    pub fn new(target: Utf8PathBuf, state_dir: Utf8PathBuf) -> Self {
        assert!(
            target.is_absolute(),
            "projection target must be absolute, got {target}"
        );
        assert!(
            state_dir.is_absolute(),
            "projection state_dir must be absolute, got {state_dir}"
        );
        assert!(
            is_normalized(&target),
            "projection target must not carry `..` components, got {target}"
        );
        assert!(
            is_normalized(&state_dir),
            "projection state_dir must not carry `..` components, got {state_dir}"
        );
        assert!(
            state_dir != target,
            "projection state_dir must not equal the target ({target}): \
             the projection's own state files would classify as foreign"
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
    /// the subtree under it never classifies, and a desired path claiming a
    /// location that overlaps it — inside the subtree, or with the state
    /// directory beneath the location — refuses as
    /// [`Containment`](crate::Error::Containment).
    ///
    /// `None` when the state directory lives outside the target — nothing
    /// in the destination is the projection's own state, so nothing is
    /// excluded. (A state directory equal to the target, which would leave
    /// its files inside the destination yet outside any excludable
    /// subtree, is rejected by [`new`](Projection::new).)
    pub fn state_prefix(&self) -> Option<&Utf8Path> {
        match self.state_dir.strip_prefix(&self.target) {
            Ok(prefix) if !prefix.as_str().is_empty() => Some(prefix),
            _ => None,
        }
    }
}

/// Whether an absolute path is free of `..` components — the shape
/// [`Projection`]'s lexical equality and prefix reasoning requires. (`.`
/// components never survive component iteration, so they need no check.)
fn is_normalized(path: &Utf8Path) -> bool {
    path.components()
        .all(|component| component != camino::Utf8Component::ParentDir)
}

#[cfg(test)]
#[path = "projection_tests.rs"]
mod tests;
