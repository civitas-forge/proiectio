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
        body: b"managed".to_vec(),
    };

    assert_eq!(file.kind(), EntryKind::File);
    assert_eq!(link.kind(), EntryKind::Symlink);
    assert_eq!(block.kind(), EntryKind::Block);
}
