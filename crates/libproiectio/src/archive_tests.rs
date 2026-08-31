use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufWriter, Write};

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::fs_utf8::Dir as Utf8Dir;

use super::*;
use crate::test_support::{Fixture, MissingName, Tree, assert_tree, origins_of, state_at};
use crate::{
    Dropped, Manifest, Origin, PlanOptions, Refusal, RefusalKind, apply, block_markers, decide,
    load_manifest, observe,
};

// ---------------------------------------------------------------------------
// Building archives
//
// The corpus is built here rather than committed as binary fixtures: a
// hostile archive is exactly what a well-behaved writer refuses to produce,
// so the tar side is a hand-written ustar encoder — 512-byte headers, no
// opinions — which can spell an absolute name, a climbing name, a hardlink,
// and a device node. The zip side uses the `zip` writer, which does not
// sanitize the names it is given.
// ---------------------------------------------------------------------------

// ustar type flags, as the format spells them.
const REGULAR: u8 = b'0';
const HARDLINK: u8 = b'1';
const SYMLINK: u8 = b'2';
const CHARDEV: u8 = b'3';
const DIRECTORY: u8 = b'5';
const FIFO: u8 = b'6';

// One tar member: everything the header carries that this crate reads,
// plus the body.
struct Member {
    name: Vec<u8>,
    kind: u8,
    mode: u32,
    link: String,
    body: Vec<u8>,
    // The size the header declares, which need not be `body.len()` — a
    // header that lies is part of the corpus.
    declared: Option<u64>,
}

impl Member {
    fn new(name: impl AsRef<[u8]>, kind: u8) -> Self {
        Self {
            name: name.as_ref().to_vec(),
            kind,
            mode: 0o644,
            link: String::new(),
            body: Vec::new(),
            declared: None,
        }
    }

    fn file(name: &str, body: &str) -> Self {
        let mut member = Self::new(name, REGULAR);
        member.body = body.as_bytes().to_vec();
        member
    }

    fn executable(name: &str, body: &str) -> Self {
        let mut member = Self::file(name, body);
        member.mode = 0o755;
        member
    }

    fn dir(name: &str) -> Self {
        let mut member = Self::new(name, DIRECTORY);
        member.mode = 0o755;
        member
    }

    fn symlink(name: &str, target: &str) -> Self {
        let mut member = Self::new(name, SYMLINK);
        member.link = target.to_owned();
        member.mode = 0o777;
        member
    }

    fn mode(mut self, mode: u32) -> Self {
        self.mode = mode;
        self
    }

    fn declaring(mut self, size: u64) -> Self {
        self.declared = Some(size);
        self
    }
}

// Writes one member's header and body into a tar stream.
fn write_member(out: &mut impl Write, member: &Member) {
    let size = member.declared.unwrap_or(member.body.len() as u64);
    write_header(
        out,
        &member.name,
        member.kind,
        member.mode,
        &member.link,
        size,
    );
    out.write_all(&member.body).expect("write member body");
    let padding = (512 - member.body.len() % 512) % 512;
    out.write_all(&vec![0u8; padding]).expect("pad member body");
}

// Writes one 512-byte ustar header.
fn write_header(out: &mut impl Write, name: &[u8], kind: u8, mode: u32, link: &str, size: u64) {
    let mut header = [0u8; 512];
    let (prefix, name) = split_name(name);
    header[0..name.len()].copy_from_slice(name);
    header[345..345 + prefix.len()].copy_from_slice(prefix);
    put_octal(&mut header[100..108], u64::from(mode), 7);
    put_octal(&mut header[108..116], 0, 7);
    put_octal(&mut header[116..124], 0, 7);
    put_octal(&mut header[124..136], size, 11);
    put_octal(&mut header[136..148], 0, 11);
    header[156] = kind;
    header[157..157 + link.len()].copy_from_slice(link.as_bytes());
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    // The checksum is computed with its own field read as spaces.
    header[148..156].copy_from_slice(b"        ");
    let sum: u32 = header.iter().map(|&byte| u32::from(byte)).sum();
    put_octal(&mut header[148..156], u64::from(sum), 6);
    header[154] = 0;
    header[155] = b' ';
    out.write_all(&header).expect("write member header");
}

// Splits a member name across ustar's 155-byte `prefix` and 100-byte
// `name` fields, which is how the format spells a path longer than 100
// bytes without reaching for a GNU extension.
fn split_name(name: &[u8]) -> (&[u8], &[u8]) {
    if name.len() <= 100 {
        return (&[], name);
    }
    // The earliest separator that leaves at most 100 bytes on the right.
    let start = name.len() - 101;
    let split = start
        + name[start..]
            .iter()
            .position(|&byte| byte == b'/')
            .expect("a long test name splits on a separator");
    assert!(split <= 155, "test name too long for a ustar header");
    (&name[..split], &name[split + 1..])
}

// Writes `value` as a NUL-terminated octal string of `digits` digits.
fn put_octal(field: &mut [u8], value: u64, digits: usize) {
    let text = format!("{value:0digits$o}");
    field[..digits].copy_from_slice(text.as_bytes());
    field[digits] = 0;
}

// The two zero blocks that end a tar stream.
fn write_end(out: &mut impl Write) {
    out.write_all(&[0u8; 1024]).expect("write end-of-archive");
}

// A whole tar archive in memory.
fn tar(members: &[Member]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for member in members {
        write_member(&mut bytes, member);
    }
    write_end(&mut bytes);
    bytes
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).expect("gzip the archive");
    encoder.finish().expect("finish the gzip stream")
}

fn zstd_compress(bytes: &[u8]) -> Vec<u8> {
    zstd::stream::encode_all(bytes, 1).expect("zstd the archive")
}

// One zip member.
enum ZipMember {
    File {
        name: String,
        body: String,
        mode: u32,
    },
    Dir(String),
    Symlink {
        name: String,
        target: String,
    },
}

fn zip_file(name: &str, body: &str) -> ZipMember {
    ZipMember::File {
        name: name.to_owned(),
        body: body.to_owned(),
        mode: 0o644,
    }
}

fn zip_executable(name: &str, body: &str) -> ZipMember {
    ZipMember::File {
        name: name.to_owned(),
        body: body.to_owned(),
        mode: 0o755,
    }
}

fn zip_symlink(name: &str, target: &str) -> ZipMember {
    ZipMember::Symlink {
        name: name.to_owned(),
        target: target.to_owned(),
    }
}

fn zip(members: &[ZipMember]) -> Vec<u8> {
    let options = zip::write::SimpleFileOptions::default();
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for member in members {
        match member {
            ZipMember::File { name, body, mode } => {
                writer
                    .start_file(name.clone(), options.unix_permissions(*mode))
                    .expect("start a zip member");
                writer
                    .write_all(body.as_bytes())
                    .expect("write a zip member");
            }
            ZipMember::Dir(name) => {
                writer
                    .add_directory(name.clone(), options)
                    .expect("add a zip directory");
            }
            ZipMember::Symlink { name, target } => {
                writer
                    .add_symlink(name.clone(), target.clone(), options)
                    .expect("add a zip symlink");
            }
        }
    }
    writer
        .finish()
        .expect("finish the zip archive")
        .into_inner()
}

// ---------------------------------------------------------------------------
// Loading them
// ---------------------------------------------------------------------------

fn expand_at(name: &str, bytes: &[u8], strip: u32) -> (Fixture, Result<Desired>) {
    let fixture = Tree::new().file(name, bytes.to_vec()).materialize();
    let expanded = load_archive(&fixture.path(name), strip, crate::Limits::default());
    (fixture, expanded)
}

fn expand_bytes(name: &str, bytes: &[u8], strip: u32) -> Result<Desired> {
    expand_at(name, bytes, strip).1
}

fn dropped_members(desired: &Desired) -> Vec<&str> {
    desired
        .dropped()
        .iter()
        .map(|dropped| dropped.member.as_str())
        .collect()
}

fn from_archive(fixture: &Fixture, name: &str, entries: BTreeMap<Utf8PathBuf, Entry>) -> Desired {
    Desired::from_source(
        entries,
        Origin::Archive {
            path: fixture.path(name),
            via: None,
        },
    )
}

// The one logical tree every happy-path format carries: a plain file, an
// executable, an empty directory, a nested file, and a relative link into
// the nesting.
fn declared_tree() -> Tree {
    Tree::new()
        .file("config/settings.toml", "listen = \":8080\"\n")
        .executable("bin/tool", "#!/bin/sh\necho tool\n")
        .file("releases/1.2.3/marker", "release\n")
        .symlink("current", "releases/1.2.3")
}

fn tar_members() -> Vec<Member> {
    vec![
        Member::dir("bin/"),
        Member::executable("bin/tool", "#!/bin/sh\necho tool\n"),
        Member::dir("config/"),
        Member::file("config/settings.toml", "listen = \":8080\"\n"),
        Member::symlink("current", "releases/1.2.3"),
        Member::dir("empty/"),
        Member::dir("releases/1.2.3/"),
        Member::file("releases/1.2.3/marker", "release\n"),
    ]
}

fn zip_members() -> Vec<ZipMember> {
    vec![
        ZipMember::Dir("bin/".to_owned()),
        zip_executable("bin/tool", "#!/bin/sh\necho tool\n"),
        ZipMember::Dir("config/".to_owned()),
        zip_file("config/settings.toml", "listen = \":8080\"\n"),
        zip_symlink("current", "releases/1.2.3"),
        ZipMember::Dir("empty/".to_owned()),
        zip_file("releases/1.2.3/marker", "release\n"),
    ]
}

fn dir_at(root: &Utf8Path) -> Utf8Dir {
    Utf8Dir::open_ambient_dir(root, cap_std::ambient_authority()).expect("open fixture root")
}

// ---------------------------------------------------------------------------
// Happy paths
// ---------------------------------------------------------------------------

#[test]
fn every_tar_spelling_expands_to_one_tree() {
    let bytes = tar(&tar_members());
    let expected = declared_tree().entries();

    for (name, archive) in [
        ("skeleton.tar", bytes.clone()),
        ("skeleton.tar.gz", gzip(&bytes)),
        ("skeleton.tgz", gzip(&bytes)),
        ("skeleton.tar.zst", zstd_compress(&bytes)),
    ] {
        let (fixture, expanded) = expand_at(name, &archive, 0);
        assert_eq!(
            expanded.unwrap(),
            from_archive(&fixture, name, expected.clone()),
            "{name} expanded to a different tree"
        );
    }
}

#[test]
fn a_zip_expands_to_the_same_tree() {
    let (fixture, expanded) = expand_at("skeleton.zip", &zip(&zip_members()), 0);
    assert_eq!(
        expanded.unwrap(),
        from_archive(&fixture, "skeleton.zip", declared_tree().entries())
    );
}

// Directories carry no entry of their own, exactly as in a source tree: an
// `empty/` member is judged and then contributes nothing, so nothing is
// created at the destination for it.
#[test]
fn a_directory_member_projects_nothing() {
    let tree = expand_bytes("only-dirs.tar", &tar(&[Member::dir("empty/")]), 0).unwrap();
    assert_eq!(tree, Desired::new());
}

#[test]
fn a_leading_dot_expands_like_the_name_without_it() {
    let dotted = vec![
        Member::dir("./"),
        Member::dir("./x/"),
        Member::file("./x/a.txt", "a\n"),
        Member::file("././deep.txt", "deep\n"),
        Member::symlink("./current", "x/a.txt"),
    ];
    let plain = vec![
        Member::dir("x/"),
        Member::file("x/a.txt", "a\n"),
        Member::file("deep.txt", "deep\n"),
        Member::symlink("current", "x/a.txt"),
    ];
    let expected = Tree::new()
        .file("x/a.txt", "a\n")
        .file("deep.txt", "deep\n")
        .symlink("current", "x/a.txt")
        .entries();

    let (fixture, expanded) = expand_at("dot.tgz", &gzip(&tar(&dotted)), 0);
    assert_eq!(
        expanded.unwrap(),
        from_archive(&fixture, "dot.tgz", expected.clone())
    );
    let (fixture, expanded) = expand_at("plain.tgz", &gzip(&tar(&plain)), 0);
    assert_eq!(
        expanded.unwrap(),
        from_archive(&fixture, "plain.tgz", expected)
    );
}

#[test]
fn a_member_naming_only_the_root_projects_nothing() {
    for name in ["./", ".", "././"] {
        let tree = expand_bytes("root.tar", &tar(&[Member::dir(name)]), 0)
            .unwrap_or_else(|error| panic!("{name:?}: expected acceptance, got {error}"));
        assert_eq!(tree, Desired::new(), "{name:?}");
    }
}

#[test]
fn strip_never_spends_a_level_on_a_leading_dot() {
    let members = vec![
        Member::dir("./"),
        Member::dir("./top/"),
        Member::file("./top/x", "x\n"),
    ];
    let (fixture, expanded) = expand_at("dot.tar", &tar(&members), 1);
    assert_eq!(
        expanded.unwrap(),
        from_archive(&fixture, "dot.tar", Tree::new().file("x", "x\n").entries())
    );
}

#[test]
fn a_leading_dot_never_admits_an_escaping_member() {
    let members = vec![
        Member::file("./../escape", "out\n"),
        Member::file("./x/../../escape", "out\n"),
        Member::file("../plain-escape", "out\n"),
        Member::file("/etc/passwd", "root::0:0\n"),
        Member::file("./ok", "kept\n"),
    ];
    let refused = match expand_bytes("escape.tar", &tar(&members), 0).unwrap_err() {
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment => {
            origins_of(&refused)
        }
        other => panic!("expected a containment refusal, got {other}"),
    };
    let named: Vec<&str> = refused.keys().map(|path| path.as_str()).collect();
    assert_eq!(
        named,
        vec![
            "/etc/passwd",
            "./../escape",
            "./x/../../escape",
            "../plain-escape",
        ]
    );
}

#[test]
fn a_zips_leading_dot_normalizes_too() {
    let members = vec![
        ZipMember::Dir("./".to_owned()),
        zip_file("./README", "read me\n"),
    ];
    let (fixture, expanded) = expand_at("dot.zip", &zip(&members), 0);
    assert_eq!(
        expanded.unwrap(),
        from_archive(
            &fixture,
            "dot.zip",
            Tree::new().file("README", "read me\n").entries()
        )
    );
}

// The definition of done for the happy path: an archive projects, and the
// relative link inside it still resolves at the destination because the
// layout came along.
#[test]
fn an_expanded_archive_projects_and_its_relative_link_resolves() {
    let desired = expand_bytes("skeleton.tar.gz", &gzip(&tar(&tar_members())), 0).unwrap();

    let (dest, state) = (Tree::new().materialize(), Tree::new().materialize());
    let dest_dir = dir_at(dest.root());
    let state_dir = state_at(state.root());
    let manifest = load_manifest(&state_dir).expect("load manifest");
    let observations =
        observe(&dest_dir, &manifest, &block_markers(&desired)).expect("observe destination");
    let plan = decide(
        "archive",
        &desired,
        &manifest,
        &observations,
        None,
        PlanOptions::default(),
    )
    .expect("decide");
    apply(&dest_dir, &state_dir, &manifest, &plan).expect("apply the plan");

    assert_tree(dest.root(), &declared_tree());
    assert_eq!(
        fs::read(dest.path("current/marker")).expect("read through the projected link"),
        b"release\n",
    );

    // Nothing downstream remembers an archive existed: the manifest records
    // one ordinary entry per member.
    let manifest = load_manifest(&state_dir).expect("reload manifest");
    let recorded: Vec<&str> = manifest.entries.keys().map(|path| path.as_str()).collect();
    assert_eq!(
        recorded,
        vec![
            "bin/tool",
            "config/settings.toml",
            "current",
            "releases/1.2.3/marker"
        ]
    );
}

#[test]
fn strip_drops_the_wrapper_directory() {
    let members = vec![
        Member::dir("skeleton-1.2/"),
        Member::dir("skeleton-1.2/bin/"),
        Member::executable("skeleton-1.2/bin/tool", "#!/bin/sh\n"),
        Member::symlink("skeleton-1.2/current", "bin/tool"),
    ];
    let (fixture, expanded) = expand_at("skeleton-1.2.tar.gz", &gzip(&tar(&members)), 1);
    assert_eq!(
        expanded.unwrap(),
        from_archive(
            &fixture,
            "skeleton-1.2.tar.gz",
            Tree::new()
                .executable("bin/tool", "#!/bin/sh\n")
                .symlink("current", "bin/tool")
                .entries()
        )
    );
}

#[test]
fn strip_drops_a_zips_wrapper_directory_too() {
    let members = vec![
        ZipMember::Dir("skeleton-1.2/".to_owned()),
        zip_file("skeleton-1.2/README", "read me\n"),
    ];
    let (fixture, expanded) = expand_at("skeleton-1.2.zip", &zip(&members), 1);
    assert_eq!(
        expanded.unwrap(),
        from_archive(
            &fixture,
            "skeleton-1.2.zip",
            Tree::new().file("README", "read me\n").entries()
        )
    );
}

// `strip` erasing a *file* drops that member and keeps the archive, the way
// GNU tar's `--strip-components` does — stock macOS `tar` writes an
// AppleDouble `._pkg` beside every `pkg`, and one of those must not cost the
// whole load. The drop is named rather than silent.
#[test]
fn a_file_member_strip_erases_is_dropped_and_the_rest_loads() {
    let members = vec![
        Member::file("._pkg", "AppleDouble\n"),
        Member::dir("pkg/"),
        Member::file("pkg/README", "read me\n"),
    ];
    let (fixture, expanded) = expand_at("pkg.tar", &tar(&members), 1);
    let expanded = expanded.unwrap();
    assert_eq!(
        expanded.iter().collect::<BTreeMap<_, _>>(),
        Tree::new()
            .file("README", "read me\n")
            .entries()
            .iter()
            .collect::<BTreeMap<_, _>>()
    );
    assert_eq!(
        expanded.dropped(),
        &BTreeSet::from([Dropped {
            member: Utf8PathBuf::from("._pkg"),
            prefix: Utf8PathBuf::new(),
            strip: 1,
            origin: Origin::Archive {
                path: fixture.path("pkg.tar"),
                via: None,
            },
        }])
    );
}

// The boundary: `strip` that leaves exactly one component leaves a member
// whole, and nothing is dropped.
#[test]
fn strip_leaving_one_component_drops_nothing() {
    let members = vec![Member::dir("pkg/"), Member::file("pkg/README", "read me\n")];
    let (fixture, expanded) = expand_at("pkg.tar", &tar(&members), 1);
    assert_eq!(
        expanded.unwrap(),
        from_archive(
            &fixture,
            "pkg.tar",
            Tree::new().file("README", "read me\n").entries()
        )
    );
}

// A symlink `strip` erases is dropped on the same terms as a file.
#[test]
fn a_symlink_member_strip_erases_is_dropped() {
    let members = vec![
        Member::symlink("current", "pkg/README"),
        Member::file("pkg/README", "read me\n"),
    ];
    let expanded = expand_bytes("pkg.tar", &tar(&members), 1).unwrap();
    assert_eq!(dropped_members(&expanded), vec!["current"]);
}

// A zip goes through the same admission, so its members drop on the same
// terms.
#[test]
fn a_zip_member_strip_erases_is_dropped_and_the_rest_loads() {
    let members = vec![
        zip_file("._pkg", "AppleDouble\n"),
        ZipMember::Dir("pkg/".to_owned()),
        zip_file("pkg/README", "read me\n"),
    ];
    let (fixture, expanded) = expand_at("pkg.zip", &zip(&members), 1);
    let expanded = expanded.unwrap();
    assert_eq!(
        expanded.iter().collect::<BTreeMap<_, _>>(),
        Tree::new()
            .file("README", "read me\n")
            .entries()
            .iter()
            .collect::<BTreeMap<_, _>>()
    );
    assert_eq!(dropped_members(&expanded), vec!["._pkg"]);
    let _ = fixture;
}

// Dropping a member among survivors is the point; dropping every one of
// them means `strip` is deeper than the archive. That has to fail rather
// than expand to nothing: an empty desired tree plans a removal, so a
// mistyped `strip` would clear whatever the owner holds.
#[test]
fn an_archive_strip_erases_entirely_fails_the_load() {
    let members = vec![
        Member::file("._pkg", "AppleDouble\n"),
        Member::file("notes.txt", "notes\n"),
    ];
    assert!(matches!(
        expand_bytes("pkg.tar", &tar(&members), 1).unwrap_err(),
        Error::ArchiveFullyStripped { strip, dropped, .. } if strip == 1 && dropped == 2
    ));
}

// Two members of one archive may carry the same name — tar imposes no rule
// against it, and two raw names can normalize onto one path. Both are erased,
// and the two halves of the drop disagree on purpose: the report states one
// record, because "._pkg had no path left" is one fact however many members
// spelled it, while the diagnostic counts two, because two members are what
// the strip actually consumed. A survivor keeps the load alive, so the count
// is observed through a later drop rather than through the error.
#[test]
fn one_name_on_two_dropped_members_is_one_record_and_two_drops() {
    let members = vec![
        Member::file("._pkg", "first\n"),
        Member::file("._pkg", "second\n"),
        Member::file("pkg/README", "read me\n"),
    ];
    let expanded = expand_bytes("pkg.tar", &tar(&members), 1).unwrap();

    assert_eq!(dropped_members(&expanded), vec!["._pkg"]);

    // The same pair with no survivor: the error counts members, not names,
    // so it says two rather than the one the record set holds.
    let members = vec![
        Member::file("._pkg", "first\n"),
        Member::file("._pkg", "second\n"),
    ];
    assert!(matches!(
        expand_bytes("pkg.tar", &tar(&members), 1).unwrap_err(),
        Error::ArchiveFullyStripped { dropped, .. } if dropped == 2
    ));
}

// Two members that survive and claim one projected path are refused, so a
// repeated name only ever reaches the drop set — never the tree.
#[test]
fn one_name_on_two_surviving_members_is_refused_rather_than_recorded_once() {
    let members = vec![
        Member::file("pkg/README", "first\n"),
        Member::file("pkg/README", "second\n"),
    ];
    assert!(matches!(
        expand_bytes("pkg.tar", &tar(&members), 1).unwrap_err(),
        Error::ArchiveMemberDuplicate { member, .. } if member == "README"
    ));
}

// The rule reads the drops, not the emptiness: an archive that carried
// nothing to begin with expands to an empty tree as it always has, and only
// an archive `strip` emptied is refused.
#[test]
fn an_archive_that_was_always_empty_still_expands_to_nothing() {
    let expanded = expand_bytes("empty.tar", &tar(&[]), 1).unwrap();

    assert!(expanded.is_empty());
    assert!(expanded.dropped().is_empty());
}

// Directories carry no entry, so an archive of nothing but directories
// projects nothing whatever `strip` does to it — the tour says so — and it
// drops nothing either, which is what keeps it outside the rule.
#[test]
fn an_archive_of_directories_alone_projects_nothing_without_failing() {
    let members = vec![Member::dir("pkg/"), Member::dir("pkg/sub/")];
    let expanded = expand_bytes("pkg.tar", &tar(&members), 1).unwrap();

    assert!(expanded.is_empty());
    assert!(expanded.dropped().is_empty());
}

// A directory surviving the strip is not something projected: it carries no
// entry, so the expansion still has nothing to show for itself while a real
// file was erased. That is the wrong-strip case the rule is for, and it
// refuses — the surviving directory does not excuse the dropped file.
#[test]
fn a_surviving_directory_does_not_save_an_expansion_that_projects_nothing() {
    let members = vec![
        Member::file("._pkg", "AppleDouble\n"),
        Member::dir("pkg/sub/"),
    ];
    assert!(matches!(
        expand_bytes("pkg.tar", &tar(&members), 1).unwrap_err(),
        Error::ArchiveFullyStripped { strip, dropped, .. } if strip == 1 && dropped == 1
    ));
}

// A dropped member is still a member: it costs a place against the cap on
// how many one archive may carry, so an archive cannot buy headroom by
// filling itself with members `strip` erases.
#[test]
fn members_strip_erases_still_count_against_the_member_cap() {
    let fixture = Tree::new().materialize();
    let path = fixture.path("many.tar.gz");
    {
        let file = fs::File::create(&path).expect("create the archive");
        let mut encoder =
            flate2::write::GzEncoder::new(BufWriter::new(file), flate2::Compression::fast());
        for index in 0..=MAX_MEMBERS {
            write_header(
                &mut encoder,
                format!("._m{index}").as_bytes(),
                REGULAR,
                0o644,
                "",
                0,
            );
        }
        write_end(&mut encoder);
        encoder.finish().expect("finish the archive");
    }
    assert!(matches!(
        load_archive(&path, 1, crate::Limits::default()).unwrap_err(),
        Error::ArchiveTooManyMembers { limit, .. } if limit == MAX_MEMBERS
    ));
}

// A mode contributes the executable bit and nothing else: setuid, group,
// and other bits are dropped, and a member with no execute bit is not
// executable however permissive the rest is.
#[test]
fn a_member_mode_contributes_only_the_executable_bit() {
    let members = vec![
        Member::file("plain", "a").mode(0o666),
        Member::file("setuid", "b").mode(0o4755),
        Member::file("group-only", "c").mode(0o010),
    ];
    let (fixture, expanded) = expand_at("modes.tar", &tar(&members), 0);
    assert_eq!(
        expanded.unwrap(),
        from_archive(
            &fixture,
            "modes.tar",
            Tree::new()
                .file("plain", "a")
                .executable("setuid", "b")
                .file("group-only", "c")
                .entries()
        )
    );
}

// ---------------------------------------------------------------------------
// The extension picks the decoder
// ---------------------------------------------------------------------------

#[test]
fn the_extension_picks_the_decoder_and_the_bytes_are_never_sniffed() {
    // Real zip bytes under a name that says gzipped tar: the decoder the
    // name picked is the decoder that runs, and it fails.
    let bytes = zip(&zip_members());
    assert!(matches!(
        expand_bytes("vendor.tar.gz", &bytes, 0).unwrap_err(),
        Error::ArchiveDecode { format, .. } if format == ArchiveFormat::TarGz
    ));

    // And the other way round.
    let bytes = gzip(&tar(&tar_members()));
    assert!(matches!(
        expand_bytes("vendor.zip", &bytes, 0).unwrap_err(),
        Error::ArchiveDecode { format, .. } if format == ArchiveFormat::Zip
    ));
}

#[test]
fn a_name_outside_the_supported_extensions_names_no_decoder() {
    for name in [
        "vendor.rar",
        "vendor.gz",
        "vendor",
        "vendor.tar.bz2",
        ".tar",
    ] {
        assert_eq!(ArchiveFormat::for_path(Utf8Path::new(name)), None, "{name}");
    }
    assert!(matches!(
        expand_bytes("vendor.rar", &tar(&tar_members()), 0).unwrap_err(),
        Error::ArchiveFormatUnknown { path } if path.file_name() == Some("vendor.rar")
    ));
}

#[test]
fn the_extension_match_ignores_ascii_case() {
    assert_eq!(
        ArchiveFormat::for_path(Utf8Path::new("/a/SKELETON.TAR.GZ")),
        Some(ArchiveFormat::TarGz)
    );
    let (fixture, expanded) = expand_at("SKELETON.TAR.GZ", &gzip(&tar(&tar_members())), 0);
    assert_eq!(
        expanded.unwrap(),
        from_archive(&fixture, "SKELETON.TAR.GZ", declared_tree().entries())
    );
}

#[test]
fn a_relative_archive_path_resolves_against_the_current_directory() {
    let absent = MissingName::with_suffix(".tar");

    assert!(matches!(
        load_archive(absent.relative(), 0, crate::Limits::default()).unwrap_err(),
        Error::Io {
            role: IoRole::Archive,
            path,
            ..
        } if path == absent.absolute()
    ));
}

#[test]
fn a_truncated_stream_reports_the_decoders_own_error() {
    let bytes = gzip(&tar(&tar_members()));
    let truncated = &bytes[..bytes.len() / 2];
    assert!(matches!(
        expand_bytes("skeleton.tar.gz", truncated, 0).unwrap_err(),
        Error::ArchiveDecode { .. }
    ));
}

// gzip streams concatenate, and one tar stream may be written through
// several gzip members — `gzip -d` and `tar tzf` read that as one archive.
// A decoder that stopped at the first member would expand a prefix of the
// archive and report success, projecting fewer files than the archive
// carries and saying nothing about the rest.
#[test]
fn a_tar_split_across_gzip_members_expands_whole() {
    let whole = tar(&[
        Member::file("a.txt", "first\n"),
        Member::file("b.txt", "second\n"),
    ]);
    let (head, tail) = whole.split_at(1024);
    let mut bytes = gzip(head);
    bytes.extend_from_slice(&gzip(tail));
    let tree = expand_bytes("both.tar.gz", &bytes, 0).expect("expand both gzip members");
    assert_eq!(
        tree.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
        ["a.txt", "b.txt"]
    );
}

// A zstd frame header names the window the decoder must hold, and that
// buffer is allocated inside the decoder, where no budget can meter it. A
// few bytes of header can ask for more than the whole load may spend, so
// the window is capped at the byte bound and a frame asking for more
// fails to decode.
#[test]
fn a_zstd_frame_asking_for_too_large_a_window_fails_to_decode() {
    // A frame header and nothing else: magic, a descriptor claiming no
    // content size and no dictionary, and a window descriptor one exponent
    // past what the cap allows.
    let exponent = window_log_max(Limits::DEFAULT_MAX_SOURCE_BYTES) - 10 + 1;
    let mut bytes = vec![0x28, 0xB5, 0x2F, 0xFD, 0x00];
    bytes.push(u8::try_from(exponent << 3).unwrap());
    let error = expand_bytes("wide.tar.zst", &bytes, 0).unwrap_err();
    assert!(
        matches!(&error, Error::ArchiveDecode { format, .. } if *format == ArchiveFormat::TarZst),
        "{error}"
    );
    // Named so the test cannot pass on some other decode failure: the
    // decoder refuses the *window*, not the truncated frame behind it.
    assert!(error.to_string().contains("too much memory"), "{error}");
}

// zstd takes its cap as an exponent, and a byte bound is not a power of two.
// Rounding the exponent down would cap a 500 MiB bound at 2^28 — 256 MiB —
// and refuse frames whose window fits the bound with room to spare, so the
// exponent is the one that covers the bound rather than the one under it.
#[test]
fn the_window_cap_covers_the_byte_bound_rather_than_falling_under_it() {
    assert_eq!(window_log_max(Limits::DEFAULT_MAX_SOURCE_BYTES), 29);
    assert!((1u64 << window_log_max(Limits::DEFAULT_MAX_SOURCE_BYTES)) >= 500 << 20);
    // A bound that is already a power of two names its own exponent: there
    // is nothing to round up to.
    assert_eq!(window_log_max(1 << 28), 28);
    // The format's own range, either side of it.
    assert_eq!(window_log_max(0), 10);
    assert_eq!(window_log_max(1), 10);
    assert_eq!(window_log_max(u64::MAX), 31);
}

// The frame the rounding buys back: a window of 288 MiB, which is over 2^28
// and under the 500 MiB bound. Its header is all there is, so the decode
// still fails — on the truncated frame behind the window, which is the point.
#[test]
fn a_zstd_frame_whose_window_fits_the_bound_is_not_refused_for_its_window() {
    // A window descriptor is an exponent and a three-bit mantissa:
    // 2^(10 + 18) + (2^28 / 8) * 1 = 288 MiB.
    let mut bytes = vec![0x28, 0xB5, 0x2F, 0xFD, 0x00];
    bytes.push((18 << 3) | 1);
    let error = expand_bytes("wide.tar.zst", &bytes, 0).unwrap_err();
    assert!(
        matches!(&error, Error::ArchiveDecode { format, .. } if *format == ArchiveFormat::TarZst),
        "{error}"
    );
    assert!(!error.to_string().contains("too much memory"), "{error}");
}

// The cap is taken off what the load has left, not off the bound it opened
// at. A mapping expands its archives against one budget, so a last archive
// asking for a window the earlier sources already spent would allocate the
// bound a second time — the decoder holds that buffer whatever the reader
// beyond it goes on to meter.
#[test]
fn a_spent_budget_narrows_the_window_a_later_archive_may_ask_for() {
    // A frame header asking for a 512 KiB window: 2^(10 + 9), no mantissa.
    let mut bytes = vec![0x28, 0xB5, 0x2F, 0xFD, 0x00];
    bytes.push(9 << 3);
    let fixture = Tree::new().file("late.tar.zst", bytes).materialize();
    let path = fixture.path("late.tar.zst");
    let limits = Limits::default().with_max_source_bytes(1 << 20);
    let expand_with = |budget: &Rc<Budget>| {
        expand(&path, 0, Utf8Path::new(""), None, budget)
            .unwrap_err()
            .to_string()
    };

    // Against the whole 1 MiB bound the window is inside what the load may
    // hold, and the frame fails on the nothing behind its header instead.
    let fresh = Rc::new(Budget::new(limits));
    assert!(!expand_with(&fresh).contains("too much memory"));

    // The same frame reached with 256 KiB left is refused at the window.
    let spent = Rc::new(Budget::new(limits));
    assert!(spent.spend((1 << 20) - (1 << 18)));
    assert!(expand_with(&spent).contains("too much memory"));
}

// The zstd counterpart: frames concatenate the same way.
#[test]
fn a_tar_split_across_zstd_frames_expands_whole() {
    let whole = tar(&[
        Member::file("a.txt", "first\n"),
        Member::file("b.txt", "second\n"),
    ]);
    let (head, tail) = whole.split_at(1024);
    let mut bytes = zstd_compress(head);
    bytes.extend_from_slice(&zstd_compress(tail));
    let tree = expand_bytes("both.tar.zst", &bytes, 0).expect("expand both zstd frames");
    assert_eq!(
        tree.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
        ["a.txt", "b.txt"]
    );
}

// ---------------------------------------------------------------------------
// The malicious corpus
// ---------------------------------------------------------------------------

// Every member the containment gateway refuses comes back named exactly as
// the archive spells it, and the whole archive is reported at once.
#[test]
fn hostile_member_names_are_refused_and_named() {
    let members = vec![
        Member::file("/etc/passwd", "root::0:0\n"),
        Member::file("../../etc/shadow", "root::\n"),
        Member::file("a/../../escape", "out\n"),
        Member::file("dir\\file", "windows\n"),
        Member::file("stream:name", "ads\n"),
        Member::file("ok", "kept\n"),
    ];
    let fixture = Tree::new().file("hostile.tar", tar(&members)).materialize();
    let error =
        load_archive(&fixture.path("hostile.tar"), 0, crate::Limits::default()).unwrap_err();
    let refused = match &error {
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment => {
            origins_of(refused)
        }
        other => panic!("expected a containment refusal, got {other}"),
    };
    assert!(refused.values().all(|origin| *origin
        == Origin::Archive {
            path: fixture.path("hostile.tar"),
            via: None,
        }));
    let named: Vec<&str> = refused.keys().map(|path| path.as_str()).collect();
    assert_eq!(
        named,
        vec![
            "/etc/passwd",
            "../../etc/shadow",
            "a/../../escape",
            "dir\\file",
            "stream:name",
        ]
    );
}

// A zip built on Windows may store `dir\file`, though the specification
// requires `/`. It is refused, never translated.
#[test]
fn a_zip_with_windows_separators_is_refused_by_name() {
    let members = vec![
        zip_file("dir\\file", "windows\n"),
        zip_file("..\\..\\escape", "out\n"),
    ];
    let refused = match expand_bytes("windows.zip", &zip(&members), 0).unwrap_err() {
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment => {
            origins_of(&refused)
        }
        other => panic!("expected a containment refusal, got {other}"),
    };
    let named: Vec<&str> = refused.keys().map(|path| path.as_str()).collect();
    assert_eq!(named, vec!["..\\..\\escape", "dir\\file"]);
}

// A zip spells a member's kind twice — the trailing `/` the specification
// asks a directory to carry, and the file-type bits of a Unix mode — and
// the two need not agree. A member described both ways is refused as the
// disagreement it is, named as the archive spells it.
#[test]
fn a_zip_member_whose_name_and_mode_disagree_is_refused() {
    let members = vec![zip_symlink("evil/", "/etc")];
    assert!(matches!(
        expand_bytes("disagree.zip", &zip(&members), 0).unwrap_err(),
        Error::ArchiveMemberKindDisagrees { member, .. } if member == "evil/"
    ));
}

// The disagreement is judged before the name reaches `strip`, which can
// erase it: a symlink named `wrapper/` under `--strip 1` strips to nothing,
// and a member that strips to nothing and calls itself a directory is
// dropped on purpose. Judged after, the symlink would vanish with no error
// at all.
#[test]
fn a_zip_member_strip_would_erase_is_still_judged_for_its_kind() {
    let members = vec![zip_symlink("wrapper/", "/etc")];
    assert!(matches!(
        expand_bytes("disagree.zip", &zip(&members), 1).unwrap_err(),
        Error::ArchiveMemberKindDisagrees { member, .. } if member == "wrapper/"
    ));
}

// A zip member name may carry a NUL, which no host accepts in a pathname.
// The gateway refuses it, so the refusal arrives at plan time rather than
// from the OS partway through an apply that had already called the path
// writable.
#[test]
fn a_member_name_carrying_a_nul_is_refused() {
    let members = vec![zip_file("a\u{0}b", "x\n"), zip_file("ok", "kept\n")];
    let refused = match expand_bytes("nul.zip", &zip(&members), 0).unwrap_err() {
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment => {
            origins_of(&refused)
        }
        other => panic!("expected a containment refusal, got {other}"),
    };
    assert_eq!(
        refused.keys().map(|path| path.as_str()).collect::<Vec<_>>(),
        vec!["a\u{0}b"]
    );
}

#[test]
fn a_hardlink_member_is_refused_by_name() {
    let mut hardlink = Member::new("lib/alias", HARDLINK);
    hardlink.link = "lib/real".to_owned();
    let members = vec![Member::file("lib/real", "real\n"), hardlink];
    assert!(matches!(
        expand_bytes("links.tar", &tar(&members), 0).unwrap_err(),
        Error::ArchiveMemberKind { member, .. } if member == "lib/alias"
    ));
}

// A kind is judged before `strip` can erase the name carrying it, on the
// tar path as on the zip one. Judged after, a fifo the caller stripped down
// to nothing would come back as "nothing left after strip" — true, and not
// the problem.
#[test]
fn a_member_strip_would_erase_is_still_refused_for_its_kind() {
    assert!(matches!(
        expand_bytes("dev.tar", &tar(&[Member::new("pkg/pipe", FIFO)]), 2).unwrap_err(),
        Error::ArchiveMemberKind { member, .. } if member == "pkg/pipe"
    ));
}

#[test]
fn a_fifo_member_is_refused_by_name() {
    assert!(matches!(
        expand_bytes("dev.tar", &tar(&[Member::new("pipe", FIFO)]), 0).unwrap_err(),
        Error::ArchiveMemberKind { member, .. } if member == "pipe"
    ));
}

#[test]
fn a_device_member_is_refused_by_name() {
    assert!(matches!(
        expand_bytes("dev.tar", &tar(&[Member::new("tty", CHARDEV)]), 0).unwrap_err(),
        Error::ArchiveMemberKind { member, .. } if member == "tty"
    ));
}

// The zip-slip shape: a symlink member pointing out of the destination,
// followed by a member whose path resolves through it. The expansion places
// both — a symlink member's target is carried verbatim and graded by
// `decide`, which refuses the pair as a tree conflict naming both members,
// and would refuse the same pair however the tree was built.
#[test]
fn a_symlink_member_and_a_member_written_through_it_are_refused_together() {
    let members = vec![
        Member::symlink("logs", "/etc"),
        Member::file("logs/passwd", "root::0:0\n"),
    ];
    let desired = expand_bytes("slip.tar", &tar(&members), 0).unwrap();
    assert_eq!(
        desired
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        vec!["logs", "logs/passwd"]
    );

    let dest = Tree::new().materialize();
    let dest_dir = dir_at(dest.root());
    let manifest = Manifest::new();
    let observations =
        observe(&dest_dir, &manifest, &block_markers(&desired)).expect("observe destination");
    let plan = decide(
        "archive",
        &desired,
        &manifest,
        &observations,
        None,
        PlanOptions::default(),
    )
    .expect("decide");

    for (path, other) in [("logs", "logs/passwd"), ("logs/passwd", "logs")] {
        let action = plan.actions.get(Utf8Path::new(path)).expect("an action");
        assert!(
            matches!(
                action,
                crate::Action::Refuse { refusal: Refusal::TreeConflict { paths }, .. }
                    if paths.iter().any(|p| p == other)
            ),
            "{path} was not refused naming {other}: {action:?}"
        );
    }
    assert!(dest.root().read_dir_utf8().unwrap().next().is_none());
}

// Zip permits duplicate names outright and tar permits them by convention.
// Neither first-wins nor last-wins is a rule the invoker can see, so a
// duplicate is refused and named.
#[test]
fn duplicate_members_are_refused_by_name() {
    let members = vec![
        Member::file("lib/tool", "the one you scanned\n"),
        Member::file("lib/tool", "the one you extracted\n"),
    ];
    assert!(matches!(
        expand_bytes("dupe.tar", &tar(&members), 0).unwrap_err(),
        Error::ArchiveMemberDuplicate { member, .. } if member == "lib/tool"
    ));

    // Two names that normalize to one path are the same duplicate.
    let members = vec![
        Member::file("lib/tool", "one\n"),
        Member::file("lib/sub/../tool", "two\n"),
    ];
    assert!(matches!(
        expand_bytes("dupe.tar", &tar(&members), 0).unwrap_err(),
        Error::ArchiveMemberDuplicate { member, .. } if member == "lib/tool"
    ));
}

// The one duplicate shape a zip hides from the expansion: two members whose
// names are **byte-identical**. `ZipArchive` keys its members by name and
// keeps the last, so the second has already replaced the first by the time
// `read_zip` sees an index — the expansion is handed one member and has
// nothing to compare. This test pins that, so a dependency that stops
// collapsing them is noticed rather than quietly changing what a zip means.
//
// The collapse is the one every extractor performs — `unzip` writes both in
// order and the last one stands — so the archive projects what extracting it
// would produce.
#[test]
fn a_zip_hides_byte_identical_duplicate_names_from_the_expansion() {
    let members = vec![
        zip_file("a/tool", "the one you scanned\n"),
        zip_file("b/tool", "the one you extracted\n"),
    ];
    let mut bytes = zip(&members);
    // The writer refuses to produce a duplicate name, so the second member
    // is renamed in the bytes afterwards — same length, and no header
    // checksum covers a name.
    let mut at = 0;
    while let Some(found) = bytes[at..]
        .windows(6)
        .position(|window| window == b"b/tool")
    {
        let start = at + found;
        bytes[start..start + 6].copy_from_slice(b"a/tool");
        at = start + 6;
    }

    let tree = expand_bytes("dupe.zip", &bytes, 0).expect("expand");
    assert_eq!(
        tree.get(Utf8Path::new("a/tool")),
        Some(&Entry::File {
            contents: b"the one you extracted\n".to_vec(),
            executable: false,
        })
    );
    assert_eq!(tree.len(), 1);
}

// `strip` can collapse two distinct members onto one path, which is the
// same double claim.
#[test]
fn members_strip_collapses_onto_one_path_are_refused() {
    let members = vec![
        Member::file("a/tool", "one\n"),
        Member::file("b/tool", "two\n"),
    ];
    assert!(matches!(
        expand_bytes("collapse.tar", &tar(&members), 1).unwrap_err(),
        Error::ArchiveMemberDuplicate { member, .. } if member == "tool"
    ));
}

#[test]
fn a_member_name_that_is_not_utf8_fails_the_load() {
    let member = Member::new(b"lib/tool\xff", REGULAR);
    assert!(matches!(
        expand_bytes("bytes.tar", &tar(&[member]), 0).unwrap_err(),
        Error::ArchiveMemberNameNotUtf8 { name, .. } if name == "lib/tool\u{fffd}"
    ));
}

#[test]
fn a_symlink_member_target_that_is_not_utf8_fails_the_load() {
    // The link name is bytes on the tape, so it is spelled into the header
    // directly and the checksum recomputed over the edit.
    let mut bytes = Vec::new();
    write_header(&mut bytes, b"current", SYMLINK, 0o777, "", 0);
    bytes[157..164].copy_from_slice(b"rel/\xff\xfeX");
    fix_checksum(&mut bytes);
    write_end(&mut bytes);

    assert!(matches!(
        expand_bytes("target.tar", &bytes, 0).unwrap_err(),
        Error::ArchiveMemberTargetNotUtf8 { member, target, .. }
            if member == "current" && target.contains('\u{fffd}')
    ));
}

// The archive's depth bound is the source-tree walk's, so a tarball of a
// directory tree expands to the tree that directory would have loaded as.
#[test]
fn a_member_nesting_past_the_depth_limit_fails_the_load() {
    let deepest = format!("{}f", "d/".repeat(MAX_MEMBER_DEPTH));
    assert!(
        expand_bytes("deep.tar", &tar(&[Member::file(&deepest, "ok\n")]), 0)
            .unwrap()
            .contains_key(Utf8Path::new(&deepest))
    );

    let past = format!("{}f", "d/".repeat(MAX_MEMBER_DEPTH + 1));
    assert!(matches!(
        expand_bytes("deep.tar", &tar(&[Member::file(&past, "ok\n")]), 0).unwrap_err(),
        Error::ArchiveMemberTooDeep { limit, .. } if limit == MAX_MEMBER_DEPTH
    ));
}

// A decompression bomb: a few kilobytes of gzip expanding past what one
// load may hold. The bound stops it while it is still being read, so the
// memory it wanted is never taken. Every bomb here is built and judged
// against BOMB_LIMIT rather than the 500 MiB default, so the tests also say
// that a caller's own limit is the one enforced.
#[test]
fn an_archive_expanding_past_the_byte_bound_fails_the_load() {
    let bomb = bomb_at("bomb.tar.gz", b"big", REGULAR, "");
    assert!(matches!(
        load_archive(&bomb.path("bomb.tar.gz"), 0, BOMB_LIMIT).unwrap_err(),
        Error::ArchiveTooLarge { limit, .. } if limit == BOMB_LIMIT.max_source_bytes
    ));
}

// A member the gateway refuses still has to be read past on a stream, and
// on a compressed stream that means decompressing it. Those bytes spend the
// same budget, so an archive cannot buy unbounded decompression with members
// it knows will be declined.
#[test]
fn a_declined_members_bytes_spend_the_budget_too() {
    let bomb = bomb_at("declined.tar.gz", b"/etc/passwd", REGULAR, "");
    assert!(matches!(
        load_archive(&bomb.path("declined.tar.gz"), 0, BOMB_LIMIT).unwrap_err(),
        Error::ArchiveTooLarge { limit, .. } if limit == BOMB_LIMIT.max_source_bytes
    ));
}

// The same holds for a member the expansion *keeps* without reading: a
// symlink header claiming a body makes the reader skip that many bytes to
// reach the next member.
#[test]
fn a_symlink_header_claiming_a_body_spends_the_budget_too() {
    let bomb = bomb_at("claimed.tar.gz", b"current", SYMLINK, "releases/1.2.3");
    assert!(matches!(
        load_archive(&bomb.path("claimed.tar.gz"), 0, BOMB_LIMIT).unwrap_err(),
        Error::ArchiveTooLarge { limit, .. } if limit == BOMB_LIMIT.max_source_bytes
    ));
}

// `tar` resolves a GNU long-name header — and a GNU long-link or a pax
// record — into memory while producing the entry that carries it, so those
// bytes are spent before the expansion is handed a member at all. Charging
// the stream rather than the member is what catches them: a long-name
// header claiming sixty-five megabytes is a few hundred kilobytes of gzip.
#[test]
fn a_long_name_header_cannot_outspend_the_budget() {
    const GNU_LONGNAME: u8 = b'L';
    let bomb = bomb_at("longname.tar.gz", b"././@LongLink", GNU_LONGNAME, "");
    assert!(matches!(
        load_archive(&bomb.path("longname.tar.gz"), 0, BOMB_LIMIT).unwrap_err(),
        Error::ArchiveTooLarge { limit, .. } if limit == BOMB_LIMIT.max_source_bytes
    ));
}

// A header may declare more than the stream holds — the bytes it promises
// are what the parser goes looking for, and running out of them is the
// decoder's own error rather than a budget the archive never actually
// spent.
#[test]
fn a_header_claiming_more_than_the_stream_holds_fails_to_decode() {
    let member = Member::new("short", REGULAR).declaring(Limits::DEFAULT_MAX_SOURCE_BYTES + 1);
    let mut bytes = Vec::new();
    write_member(&mut bytes, &member);
    write_end(&mut bytes);
    assert!(matches!(
        expand_bytes("truncated.tar", &bytes, 0).unwrap_err(),
        Error::ArchiveDecode { .. }
    ));
}

// The bound the bombs below are built and loaded against: small enough that
// building one is cheap, and nothing about the bound depends on its size.
const BOMB_LIMIT: Limits = Limits {
    max_source_bytes: 1 << 20,
};

// Writes a gzipped tar into a fresh fixture under `file`: one member of
// kind `kind` named `name`, whose body really carries
// `BOMB_LIMIT.max_source_bytes + 1` bytes. The bytes are zeros, so what lands
// on disk is a few kilobytes — which is the whole point of the bound.
fn bomb_at(file: &str, name: &[u8], kind: u8, link: &str) -> Fixture {
    let fixture = Tree::new().materialize();
    let path = fixture.path(file);
    let size = BOMB_LIMIT.max_source_bytes + 1;
    let file = fs::File::create(&path).expect("create the bomb");
    let mut encoder =
        flate2::write::GzEncoder::new(BufWriter::new(file), flate2::Compression::fast());
    write_header(&mut encoder, name, kind, 0o644, link, size);
    let chunk = vec![0u8; 1 << 20];
    let mut written = 0u64;
    while written < size {
        let take = chunk.len().min((size - written) as usize);
        encoder.write_all(&chunk[..take]).expect("write the bomb");
        written += take as u64;
    }
    let padding = (512 - size % 512) % 512;
    encoder
        .write_all(&vec![0u8; padding as usize])
        .expect("pad the bomb");
    write_end(&mut encoder);
    encoder.finish().expect("finish the bomb");
    fixture
}

// The byte bound does not see a million empty members: they expand to no
// bytes and still cost a map entry each.
#[test]
fn an_archive_carrying_too_many_members_fails_the_load() {
    let fixture = Tree::new().materialize();
    let path = fixture.path("many.tar.gz");
    {
        let file = fs::File::create(&path).expect("create the archive");
        let mut encoder =
            flate2::write::GzEncoder::new(BufWriter::new(file), flate2::Compression::fast());
        for index in 0..=MAX_MEMBERS {
            write_header(
                &mut encoder,
                format!("m{index}").as_bytes(),
                REGULAR,
                0o644,
                "",
                0,
            );
        }
        write_end(&mut encoder);
        encoder.finish().expect("finish the archive");
    }
    assert!(matches!(
        load_archive(&path, 0, crate::Limits::default()).unwrap_err(),
        Error::ArchiveTooManyMembers { limit, .. } if limit == MAX_MEMBERS
    ));
}

// A zip's central directory is read by `ZipArchive::new`, not through the
// budgeted reader that meters every other byte, so a zip is weighed by the
// size of its file before the parser is handed it.
//
// The archive here has had its end-of-central-directory record cut off: no
// parser can read it, so the second load fails to decode. That is what makes
// the first load's refusal mean something — under a bound smaller than the
// file, the size check answers before the parser ever runs.
#[test]
fn a_zip_larger_than_the_bound_is_refused_before_its_directory_is_parsed() {
    let mut bytes = zip(&zip_members());
    bytes.truncate(bytes.len() - 4);
    let fixture = Tree::new().file("wide.zip", bytes.clone()).materialize();
    let path = fixture.path("wide.zip");

    let tight = Limits::default().with_max_source_bytes(bytes.len() as u64 - 1);
    let error = load_archive(&path, 0, tight).unwrap_err();
    assert!(
        matches!(
            &error,
            Error::ArchiveFileTooLarge { size, remaining, limit, .. }
                if *size == bytes.len() as u64
                    && *remaining == tight.max_source_bytes
                    && *limit == tight.max_source_bytes
        ),
        "{error}"
    );
    // The one refusal weighing a file rather than what it expands to says so,
    // and names both numbers: the operator can see that raising the bound
    // past the file is what answers it.
    let message = error.to_string();
    assert!(
        message.contains(&format!("is {} bytes on disk", bytes.len())),
        "{message}"
    );
    assert!(
        message.contains(&format!(
            "{} bytes one load may hold",
            tight.max_source_bytes
        )),
        "{message}"
    );
    assert!(matches!(
        load_archive(&path, 0, Limits::default()).unwrap_err(),
        Error::ArchiveDecode { format, .. } if format == ArchiveFormat::Zip
    ));
}

// ---------------------------------------------------------------------------
// The prefix form
// ---------------------------------------------------------------------------

// `expand` under a prefix places every member below it — the
// `[archives."prefix/"]` shape — and the member paths are otherwise the
// same ones `load_archive` produces.
#[test]
fn a_prefix_places_every_member_beneath_it() {
    let fixture = Tree::new()
        .file("vendor.tar", tar(&tar_members()))
        .materialize();
    let tree = expand(
        &fixture.path("vendor.tar"),
        0,
        Utf8Path::new("vendor/lib"),
        None,
        &Rc::new(Budget::new(Limits::default())),
    )
    .unwrap()
    .tree;
    assert_eq!(
        tree.keys().map(|path| path.as_str()).collect::<Vec<_>>(),
        vec![
            "vendor/lib/bin/tool",
            "vendor/lib/config/settings.toml",
            "vendor/lib/current",
            "vendor/lib/releases/1.2.3/marker",
        ]
    );
}

// A member is judged before the prefix is joined, so a prefix confines a
// member rather than absorbing it: joining first would turn `../escape`
// under `vendor/` into `escape`, a contained path outside the prefix the
// mapping wrote, refused by nothing.
#[test]
fn a_prefix_never_absorbs_a_climbing_member() {
    let fixture = Tree::new()
        .file("vendor.tar", tar(&[Member::file("../escape", "out\n")]))
        .materialize();
    let refused = match expand(
        &fixture.path("vendor.tar"),
        0,
        Utf8Path::new("vendor"),
        None,
        &Rc::new(Budget::new(Limits::default())),
    )
    .unwrap_err()
    {
        Error::Refused(refused) if refused.kind() == RefusalKind::Containment => {
            origins_of(&refused)
        }
        other => panic!("expected a containment refusal, got {other}"),
    };
    assert_eq!(
        refused.keys().map(|path| path.as_str()).collect::<Vec<_>>(),
        vec!["../escape"]
    );
}

// A relative target is carried verbatim under a prefix; the link's parent
// moved, so where it resolves is `decide`'s to grade, and nothing here
// rewrites the string to compensate.
#[test]
fn a_prefix_leaves_a_symlink_target_verbatim() {
    let fixture = Tree::new()
        .file("vendor.tar", tar(&[Member::symlink("current", "../top")]))
        .materialize();
    let tree = expand(
        &fixture.path("vendor.tar"),
        0,
        Utf8Path::new("vendor"),
        None,
        &Rc::new(Budget::new(Limits::default())),
    )
    .unwrap()
    .tree;
    assert_eq!(
        tree.get(Utf8Path::new("vendor/current")),
        Some(&Entry::Symlink {
            target: "../top".to_owned()
        })
    );
}

// Recomputes a header's checksum after its bytes were edited by hand.
fn fix_checksum(header: &mut [u8]) {
    header[148..156].copy_from_slice(b"        ");
    let sum: u32 = header[..512].iter().map(|&byte| u32::from(byte)).sum();
    put_octal(&mut header[148..156], u64::from(sum), 6);
    header[154] = 0;
    header[155] = b' ';
}
