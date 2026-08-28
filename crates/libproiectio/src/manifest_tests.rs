use camino::Utf8PathBuf;

use super::*;

fn entry(kind: EntryKind, hash: &str, executable: bool, owners: &[&str]) -> ManifestEntry {
    ManifestEntry {
        kind,
        hash: hash.to_owned(),
        executable,
        owners: owners.iter().map(|owner| (*owner).to_owned()).collect(),
    }
}

fn manifest() -> Manifest {
    let mut manifest = Manifest::new();
    manifest.entries.insert(
        Utf8PathBuf::from("bin/tool"),
        entry(EntryKind::File, "aa11", true, &["site"]),
    );
    manifest.entries.insert(
        Utf8PathBuf::from("current"),
        entry(EntryKind::Symlink, "bb22", false, &["site", "harness"]),
    );
    manifest.entries.insert(
        Utf8PathBuf::from("shared/.zshrc"),
        entry(EntryKind::Block, "cc33", false, &["dotfiles"]),
    );
    manifest
}

#[test]
fn json_round_trip_preserves_the_manifest() {
    let original = manifest();

    let json = serde_json::to_string_pretty(&original).expect("serialize");
    let restored: Manifest = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(restored, original);
}

#[test]
fn serialization_is_sorted_by_path() {
    let json = serde_json::to_string(&manifest()).expect("serialize");

    let bin = json.find("bin/tool").expect("bin/tool");
    let current = json.find("current").expect("current");
    let shared = json.find("shared/.zshrc").expect("shared/.zshrc");
    assert!(bin < current && current < shared);
}

#[test]
fn new_manifest_is_empty_at_the_current_version() {
    let manifest = Manifest::new();

    assert_eq!(manifest.version, MANIFEST_VERSION);
    assert!(manifest.entries.is_empty());
}
