use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek};
use std::rc::Rc;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

use crate::{Entry, Error, Result};

/// How many bytes one load may expand archives to, summed over every member
/// it keeps — over every `[archives]` table in one mapping, not one table
/// at a time ([`new_budget`]).
///
/// A desired tree holds each member's contents as a `Vec<u8>`
/// ([`Entry::File`]), so expansion allocates what the archive decompresses
/// to — and a compressed archive chooses that number, not its own size. A
/// few hundred kilobytes of gzipped zeros expand to gigabytes; unbounded,
/// that is an out-of-memory abort at plan time, from an input
/// `docs/security.lex` section 1 calls hostile.
///
/// Unlike [`load_tree`](crate::load_tree)'s depth limit, this bound is not
/// measured against a fixed resource: there is no fixed amount of memory to
/// measure against. It is a ceiling on what an untrusted archive may make
/// the process allocate, set where no legitimate projection reaches. A
/// projection places managed files into a directory someone else owns —
/// configuration, scripts, small assets, a vendored release — which weigh
/// kilobytes to a few megabytes; 64 MiB is two orders of magnitude past
/// that and still an allocation any host can refuse cleanly rather than
/// die on.
///
/// Total expanded bytes is the bound, rather than a per-member size or a
/// compression ratio, because total bytes is what the desired tree costs.
/// A per-member cap misses ten thousand members just under it. A ratio cap
/// punishes exactly the content archives are good at — a tarball of text
/// compresses 10:1 honestly — while a bomb built from many small,
/// poorly-compressed members slips under it.
///
/// On a tar the bound is spent on every decompressed byte the parser
/// consumes, not only on the member bodies this module keeps: headers,
/// block padding, the bodies of members it skips past, and the GNU
/// long-name, GNU long-link, and pax records `tar` reads into memory
/// *before* it hands over the member they describe. Those last are the
/// reason the accounting sits in the reader rather than in the loop below
/// — a long-name header declaring eight gigabytes is a few hundred
/// kilobytes of gzip, and a budget checked per member would be consulted
/// only after the eight gigabytes had been read.
pub(crate) const MAX_EXPANDED_BYTES: u64 = 64 << 20;

/// How many members one archive may carry.
///
/// The byte bound above does not see this shape: a million empty members
/// expand to zero bytes and still cost a `BTreeMap` entry each, key
/// included. Real projections carry hundreds to a few thousand paths, so
/// 50 000 is far past any of them and caps the map's own overhead at a few
/// megabytes.
const MAX_MEMBERS: usize = 50_000;

/// How deep a member may nest once it is projected, counted in directories
/// above it — measured after `strip`, since that is the shape the
/// destination receives.
///
/// This is [`load_tree`](crate::load_tree)'s bound, and it is the same
/// constant rather than the same number so the two cannot drift apart.
/// `--tree` takes a directory or an archive (`docs/cli-tour.lex` section
/// 1), so the two spellings of one source should agree about what is
/// projectable: a tarball of a directory tree must expand to the tree that
/// directory would have loaded as. The walk's bound is a stack the
/// recursion would otherwise run off the end of; expansion is iterative and
/// has no such wall, so this limit buys the agreement rather than the
/// safety — and, incidentally, caps the length of a single key.
const MAX_MEMBER_DEPTH: usize = crate::tree::MAX_DEPTH;

/// The archive formats an archive source may be spelled in, each picked
/// from a filename extension by [`ArchiveFormat::for_path`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum ArchiveFormat {
    /// An uncompressed tar archive: `.tar`.
    Tar,
    /// A gzip-compressed tar archive: `.tar.gz` or `.tgz`.
    TarGz,
    /// A zstd-compressed tar archive: `.tar.zst`.
    TarZst,
    /// A zip archive: `.zip`.
    Zip,
}

impl ArchiveFormat {
    /// The format `path`'s extension names, or `None` when it names none of
    /// them.
    ///
    /// The extension picks the decoder and nothing else does: the bytes are
    /// never sniffed. A file whose name says `.zip` and whose bytes are a
    /// gzipped tar fails to decode ([`Error::ArchiveDecode`]) rather than
    /// being decoded as what it turned out to be — a projection that
    /// silently followed the content would let whoever wrote the file
    /// choose the decoder, and the invoker who typed the name would be
    /// reading a different archive than the one that expanded.
    ///
    /// Matching is ASCII-case-insensitive, so `SKELETON.TAR.GZ` names the
    /// same decoder as `skeleton.tar.gz`; case is not a second spelling of
    /// the format. A name that is *only* an extension (`.tar`) names no
    /// format: it is a hidden file called `tar`.
    pub fn for_path(path: &Utf8Path) -> Option<Self> {
        let name = path.file_name()?.to_ascii_lowercase();
        // Longest first so `.tar.gz` is never read as a bare `.gz` — which
        // names no format here — nor as `.tar`.
        [
            (".tar.gz", Self::TarGz),
            (".tar.zst", Self::TarZst),
            (".tgz", Self::TarGz),
            (".tar", Self::Tar),
            (".zip", Self::Zip),
        ]
        .into_iter()
        .find(|(suffix, _)| name.len() > suffix.len() && name.ends_with(suffix))
        .map(|(_, format)| format)
    }
}

impl fmt::Display for ArchiveFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Tar => "a tar archive",
            Self::TarGz => "a gzip-compressed tar archive",
            Self::TarZst => "a zstd-compressed tar archive",
            Self::Zip => "a zip archive",
        };
        f.write_str(name)
    }
}

/// The extensions [`ArchiveFormat::for_path`] recognizes, for the error a
/// name outside them produces.
pub(crate) const ARCHIVE_EXTENSIONS: &str = ".tar, .tar.gz, .tgz, .tar.zst, .zip";

/// Expands an archive into the desired tree [`decide`](crate::decide) takes
/// — the third desired-tree source beside
/// [`load_mapping`](crate::load_mapping) and
/// [`load_tree`](crate::load_tree), and the one behind `--tree` pointed at
/// an archive (`docs/cli-tour.lex` section 1,
/// <https://github.com/civitas-forge/proiectio/blob/main/docs/cli-tour.lex>).
///
/// An archive is a tree constructor, not a node type. Every member it keeps
/// becomes an **ordinary** entry — [`Entry::File`] carrying the member's
/// bytes and executable bit, [`Entry::Symlink`] carrying its target string
/// verbatim — keyed by the member's own path. Nothing downstream is
/// archive-aware: the manifest records each member as its own path, status
/// reports drift per member, and removal removes per member
/// (`docs/security.lex` section 4). An archive met *inside* a source tree is
/// a file like any other and is copied byte-for-byte; expansion happens only
/// where it is asked for.
///
/// `strip` drops that many leading components from each member's path, for
/// release tarballs wrapped in a top-level directory: `--strip 1` over
/// `skeleton-1.2/bin/tool` projects `bin/tool`. The wrapper directory itself
/// strips to nothing and contributes nothing — a directory carries no entry
/// in the first place. A *file* or *symlink* member that strips to nothing
/// is [`Error::ArchiveMemberStripped`]: it is content the caller asked to
/// project, and there is no path left to project it to, so dropping it
/// silently would lose it.
///
/// Directories carry no entry of their own, exactly as in
/// [`load_tree`](crate::load_tree): [`Entry`] has no directory variant and a
/// desired tree implies its directories from its files' parent components.
/// A directory member is still judged — containment, depth — because the
/// paths beneath it carry its name.
///
/// The executable bit is the only thing a member's mode contributes
/// (`docs/security.lex` section 4). Everything else in it — setuid, the
/// group and other bits, ownership — is dropped. A member with no mode at
/// all (a zip written by a tool that records none) is not executable.
///
/// # Trust
///
/// The trust split of `docs/security.lex` section 1: `source` is the
/// invoker's and is trusted — it may point anywhere the invoker can read and
/// passes no containment check — while every member name, mode, and symlink
/// target inside it is hostile input, chosen by whoever built the archive.
/// Archive members are the canonical hostile tree, and they get the
/// treatment every other tree path gets.
///
/// Nothing is extracted to disk here. Expansion produces a desired tree in
/// memory; the writing, and with it the apply-time refusal to write through
/// a symlinked ancestor, is [`apply`](crate::apply)'s as for any other tree.
///
/// # Refusals and errors
///
/// Every member path passes the containment gateway —
/// [`contained_join`](crate::contained_join)'s lexical contract, the same
/// one every mapping key and every walked source-tree path passes — and
/// the refused ones are aggregated so a hostile archive is reported whole,
/// in one [`Error::Containment`] naming each member as the archive spells
/// it. That is where an absolute member (`/etc/passwd`), a member climbing
/// out (`../../etc/passwd`), and a member carrying a backslash
/// (`dir\file`) are refused.
///
/// The backslash case is the one an archive raises that a filesystem does
/// not: a zip built on Windows may store `dir\file`, though the zip
/// specification requires `/`. It is **refused**, not translated. `\` is a
/// separator on one host and an ordinary filename character on another, so
/// rewriting it to `/` would guess which the archive meant — guess one way
/// and a legitimate Unix file named `a\b` becomes two directories, guess
/// the other and `..\..\x` becomes the traversal the gateway exists to
/// refuse. The gateway already refuses a backslash in any tree path on
/// every host (`docs/security.lex` section 2), and an archive gets the same
/// verdict as a mapping key.
///
/// Symlink members carry their target string into [`Entry::Symlink`]
/// verbatim and unjudged, as [`load_tree`](crate::load_tree) carries a
/// walked link's. Grading a target in-dest or external needs the
/// destination and belongs to [`decide`](crate::decide)
/// (`docs/security.lex` section 3). Under a prefix the link's *parent*
/// moves, so a relative target is resolved from where the link lands; the
/// target string itself is never rewritten to compensate, because what
/// reaches disk is the target verbatim.
///
/// One shape this function deliberately does not judge: a member nesting
/// beneath another member that is not a directory — the symlink-member
/// followed by a member writing through it, and the same shape with a file
/// underneath. [`decide`](crate::decide) refuses it for *any* desired tree
/// as [`Refusal::TreeConflict`](crate::Refusal::TreeConflict), naming both
/// paths, and it is the only stage that can: the member writing through an
/// archive's symlink may come from a `[files]` entry in the same mapping,
/// which no single archive's expansion sees. Checking it here as well would
/// give one tree two different verdicts depending on which loader built it.
///
/// The rest are errors, not refusals — an archive carrying something the
/// projection cannot express fails the load rather than declining a
/// destination path, the same split [`load_tree`](crate::load_tree) draws:
///
/// - a name with no decoder — [`Error::ArchiveFormatUnknown`];
/// - bytes that do not decode as the extension's format, or a stream that
///   ends early — [`Error::ArchiveDecode`], carrying the decoder's own
///   error;
/// - a member name that is not UTF-8 — [`Error::ArchiveMemberNameNotUtf8`],
///   since a desired-tree key is a [`Utf8PathBuf`] by construction;
/// - a symlink member whose target is not UTF-8 —
///   [`Error::ArchiveMemberTargetNotUtf8`], since [`Entry::Symlink`] carries
///   a `String`;
/// - a member of a kind the projection never writes — a hardlink, a device
///   node, a fifo, a socket, a GNU sparse member —
///   [`Error::ArchiveMemberKind`] naming it. Kinds are restricted to files,
///   directories, and symlinks (`docs/security.lex` section 4);
/// - two members claiming one projected path —
///   [`Error::ArchiveMemberDuplicate`]. Zip permits duplicate names outright
///   and tar permits them by convention; `strip` can also collapse two
///   distinct members onto one path. A desired tree holds one entry per
///   path, so a duplicate has to resolve to one member, and first-wins and
///   last-wins both let the archive's author decide which — the split
///   between what a reader sees and what an extractor writes. The
///   projection refuses instead, as it refuses two mapping keys claiming
///   one location ([`Error::MappingDuplicate`]) and two desired keys
///   claiming one ([`Refusal::TreeConflict`](crate::Refusal::TreeConflict))
///   for the same reason: there is no deterministic entry to prefer.
///   One zip shape escapes it: `ZipArchive` keys its members by the name it
///   decodes and keeps the last, so two members that decode to one name
///   have already become one before this expansion sees an index, and it is
///   handed a single member with nothing to compare. Byte-identical names
///   are the common case, and two spellings of one name — the same
///   characters stored once as flagged UTF-8 and once in a legacy encoding
///   — collapse the same way. That collapse is the one every extractor
///   performs, `unzip` writing both in order and the last standing, so a
///   zip projects what extracting it would produce; every duplicate whose
///   names survive as two is refused;
/// - a file or symlink member `strip` erases —
///   [`Error::ArchiveMemberStripped`];
/// - a member that, after `strip`, still nests more than 64 directories
///   deep —
///   [`Error::ArchiveMemberTooDeep`];
/// - an archive expanding past the memory bounds above —
///   [`Error::ArchiveTooLarge`] and [`Error::ArchiveTooManyMembers`];
/// - the archive file itself failing to open — [`Error::Io`].
///
/// # Panics
///
/// Panics if `source` is relative: the crate never consults the current
/// directory, so a relative path here has no meaning it could honor.
pub fn load_archive(source: &Utf8Path, strip: u32) -> Result<BTreeMap<Utf8PathBuf, Entry>> {
    expand(source, strip, Utf8Path::new(""), &new_budget())
}

/// The byte budget one load spends, for a caller expanding several archives
/// into a single desired tree.
///
/// [`load_mapping`](crate::load_mapping) takes one of these and hands it to
/// every `[archives]` table, so a mapping's archives share
/// [`MAX_EXPANDED_BYTES`] instead of each getting its own. The bound is on
/// what one untrusted input may make the process allocate, and a mapping is
/// one input: fifty tables naming one small bomb would otherwise buy fifty
/// times the memory, all of it live at once, since the expanded trees are
/// merged rather than expanded and discarded.
pub(crate) fn new_budget() -> Rc<Budget> {
    Budget::new(MAX_EXPANDED_BYTES)
}

/// [`load_archive`] with every projected key placed under `prefix` — the
/// `[archives."prefix/"]` mapping entry of `docs/cli-tour.lex` section 5,
/// whose members expand under the table's key. `prefix` is empty for
/// [`load_archive`] itself, and otherwise a key
/// [`load_mapping`](crate::load_mapping) has already admitted through the
/// containment gateway.
///
/// **A member is judged before the prefix is joined**, and the order is the
/// point. The gateway is what makes a hostile relative path safe, and a
/// prefix must confine a member rather than absorb it: joining first and
/// normalizing after would turn `../etc/passwd` under `vendor/` into
/// `etc/passwd` — a contained path, projected, outside the prefix the
/// mapping wrote, and refused by nothing. Judging the member alone refuses
/// it, and gives an archive the same verdict through both entry points, so
/// one corpus of hostile archives means the same thing for `--tree` and for
/// `[archives]`. Joining a normalized prefix onto a normalized member yields
/// a contained path by construction — both are sequences of ordinary
/// components — so the join needs no second verdict.
///
/// `budget` is what the whole load may still allocate — [`new_budget`] says
/// why it is the caller's rather than this function's.
pub(crate) fn expand(
    source: &Utf8Path,
    strip: u32,
    prefix: &Utf8Path,
    budget: &Rc<Budget>,
) -> Result<BTreeMap<Utf8PathBuf, Entry>> {
    assert!(
        source.is_absolute(),
        "archive source path must be absolute, got {source}"
    );
    let format = ArchiveFormat::for_path(source).ok_or_else(|| Error::ArchiveFormatUnknown {
        path: source.to_owned(),
    })?;
    let file = File::open(source).map_err(|e| Error::Io {
        path: source.to_owned(),
        source: e,
    })?;
    let reader = BufReader::new(file);

    let mut expansion = Expansion {
        source,
        format,
        strip: strip as usize,
        prefix,
        tree: BTreeMap::new(),
        refused: BTreeSet::new(),
        members: 0,
        budget: Rc::clone(budget),
    };
    match format {
        ArchiveFormat::Tar => expansion.read_tar(Budgeted::new(reader, Rc::clone(budget)))?,
        // `MultiGzDecoder`, not `GzDecoder`: gzip streams concatenate, and
        // one tar stream written through several gzip members is a single
        // archive to `gzip -d` and to `tar tzf`. A decoder that stopped at
        // the first member would expand a prefix of the archive and report
        // success — the projection would place fewer files than the archive
        // carries, silently, which is the divergence between what a reader
        // shows and what an extractor writes that this module refuses
        // everywhere else.
        ArchiveFormat::TarGz => {
            let decoder = flate2::read::MultiGzDecoder::new(reader);
            expansion.read_tar(Budgeted::new(decoder, Rc::clone(budget)))?;
        }
        ArchiveFormat::TarZst => {
            let mut decoder = zstd::stream::read::Decoder::new(reader)
                .map_err(|e| decode_error(source, format, e))?;
            // A zstd frame header names the window the decoder must hold,
            // and that buffer is allocated inside the decoder — bytes no
            // `Budgeted` reader ever sees, from a header a few dozen bytes
            // long. The reference library's own default would let a small
            // archive ask for 128 MiB, twice what the whole load may spend,
            // so the window is capped at the byte bound instead. A frame
            // asking for more is `ArchiveDecode`: it could not have fit in
            // the budget anyway.
            decoder
                .window_log_max(MAX_EXPANDED_BYTES.ilog2())
                .map_err(|e| decode_error(source, format, e))?;
            expansion.read_tar(Budgeted::new(decoder, Rc::clone(budget)))?;
        }
        ArchiveFormat::Zip => expansion.read_zip(reader)?,
    }
    if !expansion.refused.is_empty() {
        return Err(Error::Containment {
            paths: expansion.refused,
        });
    }
    Ok(expansion.tree)
}

/// What is left of [`MAX_EXPANDED_BYTES`], and whether something ran it out.
///
/// Shared between the [`Expansion`] and the reader wrapped around a tar's
/// decompressed stream, because on a tar the two spend the same budget: the
/// parser reads bytes the expansion never sees, and a bound the expansion
/// alone held would be consulted after they had already been allocated.
pub(crate) struct Budget {
    remaining: Cell<u64>,
    /// Set the moment a spend does not fit, so the failure a reader can only
    /// report as an [`io::Error`] comes back as [`Error::ArchiveTooLarge`]
    /// rather than as a decode failure.
    exhausted: Cell<bool>,
}

impl Budget {
    fn new(bytes: u64) -> Rc<Self> {
        Rc::new(Self {
            remaining: Cell::new(bytes),
            exhausted: Cell::new(false),
        })
    }

    /// Spends `bytes`, or records the budget as exhausted and answers false.
    fn spend(&self, bytes: u64) -> bool {
        match self.remaining.get().checked_sub(bytes) {
            Some(left) => {
                self.remaining.set(left);
                true
            }
            None => {
                self.exhausted.set(true);
                false
            }
        }
    }
}

/// A tar's decompressed stream, spending the [`Budget`] on every byte the
/// parser takes from it and never handing over one past the budget.
///
/// This is where a tar's byte bound lives, rather than around each member
/// body, because `tar` reads a member's GNU long-name, GNU long-link, and
/// pax records into memory while producing the entry that carries them —
/// work that has finished by the time the expansion sees a member at all.
/// A header declaring a gigabyte of long name is a few hundred kilobytes of
/// gzip, and the loop below would meet the gigabyte already allocated.
struct Budgeted<R> {
    inner: R,
    budget: Rc<Budget>,
}

impl<R> Budgeted<R> {
    fn new(inner: R, budget: Rc<Budget>) -> Self {
        Self { inner, budget }
    }
}

impl<R: Read> Read for Budgeted<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // One byte past what remains, so an overrun is what the next read
        // reports rather than something a caller has to notice afterwards.
        let headroom = self.budget.remaining.get().saturating_add(1);
        let end = buf
            .len()
            .min(usize::try_from(headroom).unwrap_or(usize::MAX));
        let read = self.inner.read(&mut buf[..end])?;
        if !self.budget.spend(read as u64) {
            return Err(io::Error::other(
                "archive expands past the bytes an archive may allocate",
            ));
        }
        Ok(read)
    }
}

/// One [`expand`] run: the tree built so far, the member names containment
/// refused — which accumulate so an archive is reported whole rather than
/// one offending member at a time — and the two resource budgets the
/// archive spends.
struct Expansion<'a> {
    /// The absolute archive path, used to name the archive in every error.
    source: &'a Utf8Path,
    /// The format its extension picked, named in decode failures.
    format: ArchiveFormat,
    /// Leading path components dropped from every member.
    strip: usize,
    /// Where the expanded members land, empty for [`load_archive`].
    prefix: &'a Utf8Path,
    tree: BTreeMap<Utf8PathBuf, Entry>,
    refused: BTreeSet<Utf8PathBuf>,
    /// Members seen so far, against [`MAX_MEMBERS`].
    members: usize,
    /// Bytes the expansion may still allocate, from [`MAX_EXPANDED_BYTES`].
    /// On a tar this is the same budget [`Budgeted`] spends.
    budget: Rc<Budget>,
}

impl Expansion<'_> {
    /// Reads a tar stream — the same code for all three tar spellings, which
    /// differ only in the decompressor wrapped around the file.
    ///
    /// `reader` is always a [`Budgeted`], and every byte the parser takes
    /// from it is charged there: the headers, the block padding, the bodies
    /// of members this loop skips past (a tar is a stream, so skipping one
    /// still reads — and on a compressed stream decompresses — it), and the
    /// extension records `tar` resolves before yielding the member they
    /// describe. Nothing below charges the budget a second time.
    fn read_tar(&mut self, reader: Budgeted<impl Read>) -> Result<()> {
        let mut archive = tar::Archive::new(reader);
        let entries = archive.entries().map_err(|e| self.decode(e))?;
        for entry in entries {
            let mut entry = entry.map_err(|e| self.decode(e))?;
            let entry_type = entry.header().entry_type();
            // A pax *global* header describes the archive, not a member; the
            // tar reader consumes GNU long names and pax per-member headers
            // itself, so nothing else metadata-shaped reaches this loop.
            if entry_type == tar::EntryType::XGlobalHeader {
                continue;
            }
            self.count_member()?;
            let raw = entry.path_bytes().into_owned();
            let name = self.utf8_name(&raw)?;
            let is_dir = entry_type.is_dir();
            // The kind first, as on the zip path and for the same reason: a
            // name can vanish before it is judged — `strip` erases one, and
            // one the gateway refuses is only recorded — so a hardlink, a
            // device node, a fifo, or a GNU sparse member would come back
            // named for what became of its path rather than for the kind
            // that is the actual problem. Kinds `docs/security.lex` section
            // 4 restricts out, and a hardlink in particular is a second name
            // for a file the projection would otherwise have to resolve.
            if !matches!(
                entry_type,
                tar::EntryType::Regular
                    | tar::EntryType::Continuous
                    | tar::EntryType::Symlink
                    | tar::EntryType::Directory
            ) {
                return Err(Error::ArchiveMemberKind {
                    path: self.source.to_owned(),
                    member: Utf8PathBuf::from(name),
                });
            }
            let Some(member) = self.admit(name, is_dir)? else {
                continue;
            };
            if is_dir {
                continue;
            }
            if entry_type == tar::EntryType::Symlink {
                let raw_target = entry.link_name_bytes().unwrap_or_default().into_owned();
                let target = self.utf8_target(&member, &raw_target)?;
                self.insert(member, Entry::Symlink { target })?;
            } else {
                let mode = entry.header().mode().map_err(|e| self.decode(e))?;
                // Unbounded here only in form: the entry reads through the
                // `Budgeted` stream, which stops one byte past what the
                // budget has left.
                let mut contents = Vec::new();
                entry
                    .read_to_end(&mut contents)
                    .map_err(|e| self.decode(e))?;
                self.insert(
                    member,
                    Entry::File {
                        contents,
                        executable: is_executable(mode),
                    },
                )?;
            }
        }
        Ok(())
    }

    /// Reads a zip archive. Unlike a tar this is random access, so a member
    /// the gateway refuses costs nothing to skip and is charged nothing.
    fn read_zip(&mut self, reader: impl Read + Seek) -> Result<()> {
        let mut archive = zip::ZipArchive::new(reader).map_err(|e| self.decode(e.into()))?;
        // Answered before any member is read rather than on the fifty
        // thousand and first pass of the loop. It does not prevent the
        // reader's own allocation — `ZipArchive::new` has already parsed the
        // central directory — but that directory is stored uncompressed, so
        // it costs bytes proportional to the file the invoker already has on
        // disk, with none of the amplification a compressed stream carries.
        if archive.len() > MAX_MEMBERS {
            return Err(Error::ArchiveTooManyMembers {
                path: self.source.to_owned(),
                limit: MAX_MEMBERS,
            });
        }
        for index in 0..archive.len() {
            self.count_member()?;
            let mut file = archive.by_index(index).map_err(|e| self.decode(e.into()))?;
            // `name_raw`, never `name`: the latter decodes a name that is
            // not flagged UTF-8 as CP437, which invents a spelling the
            // archive never carried. A name this crate cannot spell in UTF-8
            // is an error, not a transliteration.
            let raw = file.name_raw().to_vec();
            let name = self.utf8_name(&raw)?;
            let is_dir = file.is_dir();
            let mode = file.unix_mode();
            // A zip spells a member's kind twice and the two need not agree:
            // the trailing `/` the specification asks for, which is all
            // `ZipFile::is_dir` reads, and the file-type bits of a Unix
            // mode. Judged here, before the name reaches the gateway or
            // `strip`, because both can make a member vanish: a symlink
            // named `wrapper/` under `--strip 1` strips to nothing, and a
            // directory that strips to nothing is dropped on purpose. So
            // the disagreement has to be caught while the member is still
            // whole, and it is named as the archive spells it, trailing
            // slash included.
            let kind = mode.map(|mode| mode & S_IFMT);
            let agrees = match kind {
                // No mode, or one carrying no file-type bits, says nothing
                // about the kind; the name is then the only spelling there
                // is.
                None | Some(0) => true,
                Some(S_IFDIR) => is_dir,
                Some(S_IFREG | S_IFLNK) => !is_dir,
                // Anything else is a kind the projection never writes — a
                // fifo, a socket, a device node.
                Some(_) => {
                    return Err(Error::ArchiveMemberKind {
                        path: self.source.to_owned(),
                        member: Utf8PathBuf::from(name),
                    });
                }
            };
            if !agrees {
                return Err(Error::ArchiveMemberKindDisagrees {
                    path: self.source.to_owned(),
                    member: Utf8PathBuf::from(name),
                });
            }
            let Some(member) = self.admit(name, is_dir)? else {
                continue;
            };
            if is_dir {
                continue;
            }
            if file.is_symlink() {
                // A zip symlink's body *is* its target string.
                let raw_target = self.read_bounded(&mut file)?;
                let target = self.utf8_target(&member, &raw_target)?;
                self.insert(member, Entry::Symlink { target })?;
            } else {
                let contents = self.read_bounded(&mut file)?;
                self.insert(
                    member,
                    Entry::File {
                        contents,
                        executable: mode.is_some_and(is_executable),
                    },
                )?;
            }
        }
        Ok(())
    }

    /// A member's name as a `str`, which is all a desired-tree key can be
    /// built from — the first thing asked of every member, since nothing
    /// else can name it in an error.
    fn utf8_name<'a>(&self, raw: &'a [u8]) -> Result<&'a str> {
        std::str::from_utf8(raw).map_err(|_| Error::ArchiveMemberNameNotUtf8 {
            path: self.source.to_owned(),
            name: String::from_utf8_lossy(raw).into_owned(),
        })
    }

    /// Judges one member name and returns the path it projects to, relative
    /// to the prefix: `None` means the member contributes nothing — either
    /// containment refused it (recorded for the aggregated
    /// [`Error::Containment`]) or `strip` erased a directory.
    fn admit(&mut self, name: &str, is_dir: bool) -> Result<Option<Utf8PathBuf>> {
        // A directory member conventionally ends in `/`, which the gateway
        // would read as an empty trailing component. The slash is how the
        // format spells "directory", not part of the name — the kind came
        // from the header, not from it.
        let spelled = if is_dir {
            name.trim_end_matches('/')
        } else {
            name
        };
        let Ok(normalized) = crate::containment::contained_normalize(Utf8Path::new(spelled)) else {
            // Named as the archive spells it, trailing slash included, so
            // the refusal points at something findable in the archive.
            self.refused.insert(Utf8PathBuf::from(name));
            return Ok(None);
        };

        let components: Vec<&str> = normalized.as_str().split('/').collect();
        let kept = components.get(self.strip..).unwrap_or(&[]);
        if kept.is_empty() {
            if is_dir {
                // The wrapper `strip` exists to drop. It carried no entry
                // anyway.
                return Ok(None);
            }
            return Err(Error::ArchiveMemberStripped {
                path: self.source.to_owned(),
                member: normalized,
                strip: self.strip as u32,
            });
        }
        // Directories above the member once it is projected — after
        // `strip`, since that is the shape the destination receives —
        // counted as `load_tree`'s walk counts them: the member's own name
        // is not a level.
        if kept.len() - 1 > MAX_MEMBER_DEPTH {
            return Err(Error::ArchiveMemberTooDeep {
                path: self.source.to_owned(),
                member: Utf8PathBuf::from(kept.join("/")),
                limit: MAX_MEMBER_DEPTH,
            });
        }
        Ok(Some(Utf8PathBuf::from(kept.join("/"))))
    }

    /// Places one expanded member in the tree, under the prefix, refusing a
    /// path a member already claimed.
    fn insert(&mut self, member: Utf8PathBuf, entry: Entry) -> Result<()> {
        // Joined with `/` explicitly, as `contained_normalize` rejoins its
        // components: a projected key is byte-identical on every host, and
        // `Utf8Path::join` would separate with the platform's own.
        let key = if self.prefix.as_str().is_empty() {
            member.clone()
        } else {
            Utf8PathBuf::from(format!("{}/{member}", self.prefix))
        };
        if self.tree.insert(key, entry).is_some() {
            return Err(Error::ArchiveMemberDuplicate {
                path: self.source.to_owned(),
                member,
            });
        }
        Ok(())
    }

    /// Counts one member against [`MAX_MEMBERS`], before any work is spent
    /// on it.
    fn count_member(&mut self) -> Result<()> {
        self.members += 1;
        if self.members > MAX_MEMBERS {
            return Err(Error::ArchiveTooManyMembers {
                path: self.source.to_owned(),
                limit: MAX_MEMBERS,
            });
        }
        Ok(())
    }

    /// Reads one zip member's body, spending the expansion budget as it
    /// goes. A tar's bodies are charged by [`Budgeted`] instead, which is
    /// wrapped around the whole stream.
    ///
    /// Bounded *while* reading rather than checked after: a bomb that has
    /// already been decompressed into a `Vec` has already cost the memory
    /// the bound exists to deny it. The reader is capped one byte past what
    /// remains, so overrunning the budget is what the extra byte reports —
    /// and a declared size is never trusted to size the buffer, since the
    /// header declaring it is the archive's to write.
    fn read_bounded(&mut self, reader: &mut impl Read) -> Result<Vec<u8>> {
        let mut contents = Vec::new();
        let read = reader
            .take(self.budget.remaining.get().saturating_add(1))
            .read_to_end(&mut contents)
            .map_err(|e| self.decode(e))?;
        self.charge(read as u64)?;
        Ok(contents)
    }

    /// Spends `bytes` of the expansion budget.
    fn charge(&mut self, bytes: u64) -> Result<()> {
        if self.budget.spend(bytes) {
            return Ok(());
        }
        Err(Error::ArchiveTooLarge {
            path: self.source.to_owned(),
            limit: MAX_EXPANDED_BYTES,
        })
    }

    /// A symlink member's target as a `String`, which is all
    /// [`Entry::Symlink`] can carry.
    fn utf8_target(&self, member: &Utf8Path, raw: &[u8]) -> Result<String> {
        std::str::from_utf8(raw)
            .map(str::to_owned)
            .map_err(|_| Error::ArchiveMemberTargetNotUtf8 {
                path: self.source.to_owned(),
                member: member.to_owned(),
                target: String::from_utf8_lossy(raw).into_owned(),
            })
    }

    /// Wraps a decoder or stream error as [`Error::ArchiveDecode`], naming
    /// the format the extension picked. The underlying error stays visible
    /// (`docs/implementation.lex` section 5).
    ///
    /// A [`Budgeted`] stream can only report an overrun as an
    /// [`io::Error`], and it reaches this function through the tar parser
    /// looking like any other stream failure. The budget records that it
    /// ran out, so the archive is named for what it did rather than for how
    /// the failure travelled.
    fn decode(&self, source: io::Error) -> Error {
        if self.budget.exhausted.get() {
            return Error::ArchiveTooLarge {
                path: self.source.to_owned(),
                limit: MAX_EXPANDED_BYTES,
            };
        }
        decode_error(self.source, self.format, source)
    }
}

/// [`Expansion::decode`] for the one decoder built before the expansion
/// exists.
fn decode_error(path: &Utf8Path, format: ArchiveFormat, source: io::Error) -> Error {
    Error::ArchiveDecode {
        path: path.to_owned(),
        format,
        source,
    }
}

/// The file-type bits of a Unix mode, and the two kinds a zip member may
/// carry. Spelled here rather than taken from `libc`, which this crate does
/// not depend on, and read on every target because the mode comes out of the
/// archive rather than off a filesystem.
const S_IFMT: u32 = 0o170_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFREG: u32 = 0o100_000;
const S_IFLNK: u32 = 0o120_000;

/// Whether a member's mode sets the owner-executable bit — the one thing a
/// mode contributes to a projected file (`docs/security.lex` section 4).
fn is_executable(mode: u32) -> bool {
    mode & 0o100 != 0
}

#[cfg(all(test, unix))]
#[path = "archive_tests.rs"]
mod tests;
