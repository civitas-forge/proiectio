use super::*;

#[test]
fn entry_kind_matches_variant() {
    let file = Entry::File {
        contents: b"x".to_vec(),
        executable: false,
    };
    let link = Entry::Symlink {
        target: "releases/1.2.3".to_owned(),
    };
    let block = Entry::Block {
        body: b"managed\n".to_vec(),
        marker: "# proiectio".to_owned(),
        placement: Placement::Append,
    };

    assert_eq!(file.kind(), EntryKind::File);
    assert_eq!(link.kind(), EntryKind::Symlink);
    // The kind carries the marker and the placement, so two owners share a
    // path only while agreeing on both.
    assert_eq!(
        block.kind(),
        EntryKind::Block {
            marker: "# proiectio".to_owned(),
            placement: Placement::Append,
        }
    );
    assert!(block.kind().is_block());
    assert!(!file.kind().is_block());
}
