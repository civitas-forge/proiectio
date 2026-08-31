use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek};
use std::rc::Rc;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

use crate::limits::Budget;
use crate::{Desired, Entry, Error, IoRole, Limits, Origin, Refusal, Refused, Result};

/// How many members one archive may carry.
const MAX_MEMBERS: usize = 50_000;

/// How deep a member may nest once projected, counted in directories above
/// it and measured after `strip`.
const MAX_MEMBER_DEPTH: usize = crate::MAX_WALK_DEPTH;

/// The archive formats an archive source may be spelled in.
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
    /// The format `path`'s extension names, matched ASCII-case-insensitively
    /// and never sniffed from the bytes, or `None` when it names none.
    pub fn for_path(path: &Utf8Path) -> Option<Self> {
        let name = path.file_name()?.to_ascii_lowercase();
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

/// Expands an archive into a desired tree, dropping `strip` leading path
/// components from each member and keeping only files, directories, and
/// symlinks. What the expansion may hold in memory is
/// [`Limits::max_source_bytes`].
pub fn load_archive(source: &Utf8Path, strip: u32, limits: Limits) -> Result<Desired> {
    let source = crate::absolutize(source)?;
    let source = source.as_path();
    let origin = Origin::Archive {
        path: source.to_owned(),
        via: None,
    };
    let budget = Rc::new(Budget::new(limits));
    let expanded = expand(source, strip, Utf8Path::new(""), None, &budget)?;
    let mut desired = Desired::from_source(expanded.tree, origin);
    for dropped in expanded.dropped {
        desired.record_dropped(dropped);
    }
    Ok(desired)
}

/// Each member passes the containment gateway before `prefix` is joined onto
/// it, so `../etc/passwd` under a `vendor/` prefix is refused; joined first
/// and normalized after, it would project as `etc/passwd`.
pub(crate) fn expand(
    source: &Utf8Path,
    strip: u32,
    prefix: &Utf8Path,
    via: Option<&Utf8Path>,
    budget: &Rc<Budget>,
) -> Result<Expanded> {
    let format = ArchiveFormat::for_path(source).ok_or_else(|| Error::ArchiveFormatUnknown {
        path: source.to_owned(),
    })?;
    let file = File::open(source).map_err(|e| Error::Io {
        role: IoRole::Archive,
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
        dropped: BTreeSet::new(),
        dropped_members: 0,
        members: 0,
        budget: Rc::clone(budget),
    };
    match format {
        ArchiveFormat::Tar => expansion.read_tar(Budgeted::new(reader, Rc::clone(budget)))?,
        // `MultiGzDecoder`, not `GzDecoder`: gzip streams concatenate, and one
        // tar stream written through several gzip members is a single archive
        // to `gzip -d` and to `tar tzf`.
        ArchiveFormat::TarGz => {
            let decoder = flate2::read::MultiGzDecoder::new(reader);
            expansion.read_tar(Budgeted::new(decoder, Rc::clone(budget)))?;
        }
        ArchiveFormat::TarZst => {
            let mut decoder = zstd::stream::read::Decoder::new(reader)
                .map_err(|e| decode_error(source, format, e))?;
            // A zstd frame header names a window buffer allocated inside the
            // decoder, which no `Budgeted` reader sees; the reference
            // library's default would let a small archive ask for 128 MiB.
            // The cap comes off what the load has left rather than the bound
            // it opened at: a mapping that has already retained most of its
            // budget would otherwise let a last archive allocate the whole
            // bound over again.
            decoder
                .window_log_max(window_log_max(budget.remaining()))
                .map_err(|e| decode_error(source, format, e))?;
            expansion.read_tar(Budgeted::new(decoder, Rc::clone(budget)))?;
        }
        ArchiveFormat::Zip => {
            let on_disk = reader
                .get_ref()
                .metadata()
                .map_err(|e| Error::Io {
                    role: IoRole::Archive,
                    path: source.to_owned(),
                    source: e,
                })?
                .len();
            expansion.read_zip(reader, on_disk)?;
        }
    }
    if !expansion.refused.is_empty() {
        let origin = Origin::Archive {
            path: source.to_owned(),
            via: via.map(Utf8Path::to_owned),
        };
        return Err(Refused::aggregate(
            expansion
                .refused
                .into_iter()
                .map(|path| (path, Refusal::Containment { through: None }, origin.clone())),
        )
        .expect("refused is not empty")
        .into());
    }
    // A drop is tolerable among members that survive; one that leaves the
    // expansion with nothing to project is a `strip` deeper than the archive.
    // Letting it through would project an empty tree, and an empty desired
    // tree plans a removal — so a mistyped `strip` would clear everything the
    // owner holds. An archive that drops nothing never reaches this: one
    // carrying only directories projects nothing on its own terms.
    if expansion.tree.is_empty() && expansion.dropped_members > 0 {
        return Err(Error::ArchiveFullyStripped {
            path: source.to_owned(),
            strip,
            dropped: expansion.dropped_members,
        });
    }
    Ok(Expanded {
        tree: expansion.tree,
        dropped: expansion
            .dropped
            .into_iter()
            .map(|member| Dropped {
                member,
                prefix: prefix.to_owned(),
                strip,
                origin: Origin::Archive {
                    path: source.to_owned(),
                    via: via.map(Utf8Path::to_owned),
                },
            })
            .collect(),
    })
}

#[derive(Debug)]
pub(crate) struct Expanded {
    pub(crate) tree: BTreeMap<Utf8PathBuf, Entry>,
    pub(crate) dropped: BTreeSet<Dropped>,
}

/// An archive member `strip` left with no path at all. One expansion is
/// identified by all four fields: the same archive expanded under two
/// `[archives]` prefixes, or at two `strip` counts, drops its members twice,
/// and each drop names the expansion that erased it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Dropped {
    /// The member's path as its archive spells it, normalized.
    pub member: Utf8PathBuf,
    /// Where in the destination the expansion places the archive: a mapping's
    /// `[archives]` key, and empty for an archive loaded on its own. The
    /// dropped member never reaches it.
    pub prefix: Utf8PathBuf,
    /// The number of leading components the expansion asked `strip` to drop,
    /// which is what left this member with no path.
    pub strip: u32,
    /// The archive that carried the member.
    pub origin: Origin,
}

/// The window cap `limit` names, spelled the way zstd takes it: the base-2
/// exponent of a window size, rounded **up** to the power of two that covers
/// `limit` and clamped to the exponents the format spells.
///
/// A frame's own window is not a power of two — its header carries an
/// exponent and a three-bit mantissa — so a 300 MiB body compressed in one
/// frame declares a window of 320 MiB. Rounding the exponent down would cap
/// the decoder at 2^28, or 256 MiB, and refuse that frame even though both
/// its window and its body fit the 500 MiB default. Rounding up pays for
/// that in the other direction: the window a decoder may hold reaches the
/// next power of two past `limit` rather than stopping at `limit` itself.
/// The clamp is the format's own range — a limit under 1 KiB would otherwise
/// name a window no decoder accepts, failing every zstd archive rather than
/// the oversized one.
fn window_log_max(limit: u64) -> u32 {
    let limit = limit.max(1);
    (limit.ilog2() + u32::from(!limit.is_power_of_two())).clamp(10, 31)
}

/// A tar's decompressed stream, spending the [`Budget`] on every byte the
/// parser takes from it, including the long-name and pax records `tar`
/// resolves before yielding the member they describe.
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
        let headroom = self.budget.remaining().saturating_add(1);
        let end = buf
            .len()
            .min(usize::try_from(headroom).unwrap_or(usize::MAX));
        let read = self.inner.read(&mut buf[..end])?;
        if !self.budget.spend(read as u64) {
            return Err(io::Error::other(
                "archive expands past the bytes one load may hold in memory",
            ));
        }
        Ok(read)
    }
}

struct Expansion<'a> {
    source: &'a Utf8Path,
    format: ArchiveFormat,
    strip: usize,
    prefix: &'a Utf8Path,
    tree: BTreeMap<Utf8PathBuf, Entry>,
    refused: BTreeSet<Utf8PathBuf>,
    /// One record per dropped name, which is what a report states: two
    /// members of one archive carrying the same name are one fact.
    dropped: BTreeSet<Utf8PathBuf>,
    /// One count per dropped member, which is what a diagnostic states: a
    /// name repeated across two members erased two of them.
    dropped_members: usize,
    members: usize,
    budget: Rc<Budget>,
}

impl Expansion<'_> {
    fn read_tar(&mut self, reader: Budgeted<impl Read>) -> Result<()> {
        let mut archive = tar::Archive::new(reader);
        let entries = archive.entries().map_err(|e| self.decode(e))?;
        for entry in entries {
            let mut entry = entry.map_err(|e| self.decode(e))?;
            let entry_type = entry.header().entry_type();
            // A pax global header describes the archive, not a member.
            if entry_type == tar::EntryType::XGlobalHeader {
                continue;
            }
            self.count_member()?;
            let raw = entry.path_bytes().into_owned();
            let name = self.utf8_name(&raw)?;
            let is_dir = entry_type.is_dir();
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

    /// `on_disk` is the archive file's own byte size, which is checked
    /// against the budget before the parser runs. Unlike a tar, whose every
    /// byte reaches the expansion through a [`Budgeted`] reader,
    /// `ZipArchive::new` reads the central directory itself and keeps every
    /// member's name, extra field, and comment before a single body is read —
    /// so a zip of nothing but maximum-length names would allocate freely
    /// under a bound it never spent. That directory cannot outgrow the file
    /// carrying it, so a file already larger than the budget's remainder is
    /// refused here instead, and what the parser retains stays proportional
    /// to the bound.
    ///
    /// This is the one place a source's compressed size is weighed at all,
    /// so [`Limits::max_source_bytes`] names it as the exception it is, and
    /// [`Error::ArchiveFileTooLarge`] names the file's size beside the bound
    /// rather than reporting the expansion budget as spent.
    fn read_zip(&mut self, reader: impl Read + Seek, on_disk: u64) -> Result<()> {
        if on_disk > self.budget.remaining() {
            return Err(Error::ArchiveFileTooLarge {
                path: self.source.to_owned(),
                size: on_disk,
                remaining: self.budget.remaining(),
                limit: self.budget.limit(),
            });
        }
        let mut archive = zip::ZipArchive::new(reader).map_err(|e| self.decode(e.into()))?;
        if archive.len() > MAX_MEMBERS {
            return Err(Error::ArchiveTooManyMembers {
                path: self.source.to_owned(),
                limit: MAX_MEMBERS,
            });
        }
        for index in 0..archive.len() {
            self.count_member()?;
            let mut file = archive.by_index(index).map_err(|e| self.decode(e.into()))?;
            // `name_raw`, never `name`: the latter decodes a name that is not
            // flagged UTF-8 as CP437.
            let raw = file.name_raw().to_vec();
            let name = self.utf8_name(&raw)?;
            let is_dir = file.is_dir();
            let mode = file.unix_mode();
            // A zip spells a member's kind twice and the two need not agree:
            // the trailing `/`, which is all `ZipFile::is_dir` reads, and the
            // file-type bits of a Unix mode.
            let kind = mode.map(|mode| mode & S_IFMT);
            let agrees = match kind {
                None | Some(0) => true,
                Some(S_IFDIR) => is_dir,
                Some(S_IFREG | S_IFLNK) => !is_dir,
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

    fn utf8_name<'a>(&self, raw: &'a [u8]) -> Result<&'a str> {
        std::str::from_utf8(raw).map_err(|_| Error::ArchiveMemberNameNotUtf8 {
            path: self.source.to_owned(),
            name: String::from_utf8_lossy(raw).into_owned(),
        })
    }

    /// Judges one member name and returns the path it projects to, relative
    /// to the prefix; `None` means the member contributes nothing.
    fn admit(&mut self, name: &str, is_dir: bool) -> Result<Option<Utf8PathBuf>> {
        // A directory member conventionally ends in `/`, which the gateway
        // would read as an empty trailing component.
        let spelled = if is_dir {
            name.trim_end_matches('/')
        } else {
            name
        };
        if is_dir && names_only_dot(spelled) {
            return Ok(None);
        }
        let spelled = without_dot_prefix(spelled);
        let Some(normalized) = crate::containment::contained_normalize(Utf8Path::new(spelled))
        else {
            self.refused.insert(Utf8PathBuf::from(name));
            return Ok(None);
        };

        let components: Vec<&str> = normalized.as_str().split('/').collect();
        let kept = components.get(self.strip..).unwrap_or(&[]);
        if kept.is_empty() {
            if !is_dir {
                self.dropped.insert(normalized);
                self.dropped_members += 1;
            }
            return Ok(None);
        }
        if kept.len() - 1 > MAX_MEMBER_DEPTH {
            return Err(Error::ArchiveMemberTooDeep {
                path: self.source.to_owned(),
                member: Utf8PathBuf::from(kept.join("/")),
                limit: MAX_MEMBER_DEPTH,
            });
        }
        Ok(Some(Utf8PathBuf::from(kept.join("/"))))
    }

    fn insert(&mut self, member: Utf8PathBuf, entry: Entry) -> Result<()> {
        // Joined with `/` explicitly: a projected key is byte-identical on
        // every host, and `Utf8Path::join` would separate with the platform's
        // own.
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

    /// Reads one zip member's body against the budget; a declared size never
    /// sizes the buffer.
    fn read_bounded(&mut self, reader: &mut impl Read) -> Result<Vec<u8>> {
        match self
            .budget
            .read_to_end(reader)
            .map_err(|e| self.decode(e))?
        {
            Some(contents) => Ok(contents),
            None => Err(self.too_large()),
        }
    }

    fn too_large(&self) -> Error {
        Error::ArchiveTooLarge {
            path: self.source.to_owned(),
            limit: self.budget.limit(),
        }
    }

    fn utf8_target(&self, member: &Utf8Path, raw: &[u8]) -> Result<String> {
        std::str::from_utf8(raw)
            .map(str::to_owned)
            .map_err(|_| Error::ArchiveMemberTargetNotUtf8 {
                path: self.source.to_owned(),
                member: member.to_owned(),
                target: String::from_utf8_lossy(raw).into_owned(),
            })
    }

    fn decode(&self, source: io::Error) -> Error {
        if self.budget.exhausted() {
            return self.too_large();
        }
        decode_error(self.source, self.format, source)
    }
}

fn decode_error(path: &Utf8Path, format: ArchiveFormat, source: io::Error) -> Error {
    Error::ArchiveDecode {
        path: path.to_owned(),
        format,
        source,
    }
}

/// The file-type bits of a Unix mode. Spelled here rather than taken from
/// `libc`, and read on every target, since the mode comes out of the archive
/// rather than off a filesystem.
const S_IFMT: u32 = 0o170_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFREG: u32 = 0o100_000;
const S_IFLNK: u32 = 0o120_000;

fn is_executable(mode: u32) -> bool {
    mode & 0o100 != 0
}

/// `name` without the leading `./`, however many times a writer spelled one.
fn without_dot_prefix(name: &str) -> &str {
    let mut rest = name;
    loop {
        match rest.strip_prefix("./") {
            Some(shorter) => rest = shorter,
            None => return if rest == "." { "" } else { rest },
        }
    }
}

fn names_only_dot(name: &str) -> bool {
    !name.is_empty() && without_dot_prefix(name).is_empty()
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
