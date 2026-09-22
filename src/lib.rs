//! Total up real on-disk usage of a directory tree.
//!
//! `std::fs::metadata().len()` reports apparent file size, which can be far
//! from what a filesystem actually spends on a file (sparse files, block
//! rounding). This crate walks a tree and sums the space the filesystem
//! reports as actually allocated.
//!
//! The other thing most `du`-alikes get wrong: they either abort on the
//! first permission error, or they silently skip unreadable entries and
//! print a total that looks complete but isn't. This crate defaults to the
//! former (fail loudly) and makes the latter an explicit, opt-in choice via
//! [`ScanOptions::lenient`].
//!
//! On Unix, files with more than one hard link are only counted once: two
//! names for the same inode share the same disk blocks, so charging both
//! would overstate the total.
//!
//! By default a scan follows every subdirectory it finds regardless of
//! which filesystem it's actually on, same as plain `du`. Set
//! [`ScanOptions::one_filesystem`] to stop at mount points instead, so a
//! scan of a disk doesn't wander onto a mounted network share or a bind
//! mount that's already counted elsewhere.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::collections::HashSet;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

/// (device, inode) pairs already counted, so a file with multiple hard
/// links only contributes its disk usage once per scan. Only meaningful
/// on Unix, where hard links exist; a no-op elsewhere.
#[cfg(unix)]
type Seen = HashSet<(u64, u64)>;
#[cfg(not(unix))]
type Seen = ();

/// The device a scan started on, when `ScanOptions::one_filesystem` is set.
/// `None` means the option is off and nothing should be compared. Only
/// meaningful on Unix, where `st_dev` identifies a filesystem; a no-op
/// elsewhere, since the standard library doesn't expose an equivalent on
/// other platforms.
#[cfg(unix)]
type Boundary = Option<u64>;
#[cfg(not(unix))]
type Boundary = ();

mod human;

pub use human::human_bytes;

/// Aggregate disk usage for a directory tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiskUsage {
    pub bytes: u64,
    pub files: u64,
    pub dirs: u64,
}

impl DiskUsage {
    fn add_file(&mut self, bytes: u64) {
        self.bytes += bytes;
        self.files += 1;
    }

    fn add_dir(&mut self) {
        self.dirs += 1;
    }

    /// `self.bytes` formatted the way `du -h` would print it, e.g. `"4.2 MiB"`.
    pub fn human_bytes(&self) -> String {
        human_bytes(self.bytes)
    }
}

impl std::ops::AddAssign for DiskUsage {
    fn add_assign(&mut self, other: DiskUsage) {
        self.bytes += other.bytes;
        self.files += other.files;
        self.dirs += other.dirs;
    }
}

/// One entry that could not be read during a scan, and why.
#[derive(Debug)]
pub struct SkippedEntry {
    pub path: PathBuf,
    pub error: io::Error,
}

/// Options controlling how a scan walks the tree and handles errors.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// If `false` (the default), the first unreadable entry aborts the
    /// scan and its error is returned to the caller. If `true`, unreadable
    /// entries are skipped and recorded in `ScanReport::skipped` instead,
    /// and the scan runs to completion. Turning this on trades a correct
    /// total for a best-effort one; use it when partial results are
    /// genuinely useful, not as a default way to silence errors.
    pub lenient: bool,
    /// If `false` (the default), symlinks are measured as themselves and
    /// never followed. Following symlinks can double-count space shared
    /// between two parts of a tree, or loop forever on a cyclic link.
    pub follow_symlinks: bool,
    /// If `false` (the default), the scan crosses mount points freely,
    /// same as plain `du`. If `true`, any directory whose device differs
    /// from the device `root` itself is on gets excluded: its bytes aren't
    /// counted and it isn't recursed into, the same way `du --one-file-system`
    /// behaves. Useful for totaling "everything actually stored on this
    /// disk" without wandering onto a mounted network share or a bind
    /// mount of something already counted elsewhere.
    ///
    /// Only meaningful on Unix, where `st_dev` identifies a filesystem;
    /// a no-op on other platforms.
    pub one_filesystem: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            lenient: false,
            follow_symlinks: false,
            one_filesystem: false,
        }
    }
}

/// Result of a scan: the totals, plus anything skipped along the way.
///
/// `skipped` is only ever non-empty when the scan ran with
/// `ScanOptions::lenient` set; in strict mode the first error short-circuits
/// the scan and is returned as an `Err` instead of being collected here.
#[derive(Debug, Default)]
pub struct ScanReport {
    pub usage: DiskUsage,
    pub skipped: Vec<SkippedEntry>,
}

/// Walk `root` and total up real disk usage.
///
/// `root` may be a file or a directory. See [`ScanOptions`] for how errors
/// and symlinks are handled.
pub fn scan(root: &Path, options: &ScanOptions) -> io::Result<ScanReport> {
    let mut report = ScanReport::default();
    let mut seen = Seen::default();

    let metadata = read_metadata(root, options);
    match metadata {
        Ok(metadata) if metadata.is_dir() => {
            let boundary = root_device(&metadata, options);
            walk(root, options, &mut report, &mut seen, &boundary)?;
        }
        Ok(metadata) => {
            report.usage.add_file(disk_bytes(&metadata, &mut seen));
        }
        Err(err) => {
            handle_error(root, err, options, &mut report)?;
        }
    }

    Ok(report)
}

/// One immediate child of the scanned root, with its own subtotal.
#[derive(Debug)]
pub struct TopLevelEntry {
    pub name: OsString,
    pub report: ScanReport,
}

/// Result of [`scan_top_level`]: a `du -d1`-style breakdown.
///
/// `total.usage` is the same figure `scan` would have produced for the same
/// root. `total.skipped` only holds errors that couldn't be attributed to a
/// specific child (failing to list `root` itself, for instance) — errors
/// encountered while measuring a particular child land in that child's own
/// `TopLevelEntry::report.skipped` instead, so a caller can tell which entry
/// they came from.
#[derive(Debug, Default)]
pub struct TopLevelBreakdown {
    pub entries: Vec<TopLevelEntry>,
    pub total: ScanReport,
}

/// Walk `root` one level at a time, like `du -d1`: a subtotal per immediate
/// child, plus the same grand total [`scan`] would produce.
///
/// If `root` is a file rather than a directory, the breakdown has a single
/// entry for `root` itself. See [`ScanOptions`] for how errors and symlinks
/// are handled; in strict mode, an error anywhere below `root` still aborts
/// the whole scan rather than just the entry it occurred in.
///
/// Hard link deduplication (see [`scan`]) applies across the whole
/// breakdown, not per entry: if the same inode turns up under two
/// different top-level children, only the first one encountered gets its
/// disk usage counted, same as a single `scan` of `root` would.
///
/// With `ScanOptions::one_filesystem` set, a top-level child that's a mount
/// point for a different filesystem than `root` still gets a
/// `TopLevelEntry`, but with an empty `report`: it's listed so callers know
/// it was there, without its contents being walked or counted.
pub fn scan_top_level(root: &Path, options: &ScanOptions) -> io::Result<TopLevelBreakdown> {
    let mut breakdown = TopLevelBreakdown::default();
    let mut seen = Seen::default();

    let metadata = match read_metadata(root, options) {
        Ok(metadata) => metadata,
        Err(err) => {
            handle_error(root, err, options, &mut breakdown.total)?;
            return Ok(breakdown);
        }
    };

    if !metadata.is_dir() {
        let mut report = ScanReport::default();
        report.usage.add_file(disk_bytes(&metadata, &mut seen));
        breakdown.total.usage += report.usage;
        breakdown.entries.push(TopLevelEntry {
            name: root.file_name().map(OsString::from).unwrap_or_default(),
            report,
        });
        return Ok(breakdown);
    }

    let boundary = root_device(&metadata, options);

    let dir_entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) => {
            handle_error(root, err, options, &mut breakdown.total)?;
            return Ok(breakdown);
        }
    };

    for entry in dir_entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                handle_error(root, err, options, &mut breakdown.total)?;
                continue;
            }
        };

        let path = entry.path();
        let mut child_report = ScanReport::default();

        match read_metadata(&path, options) {
            Ok(metadata) if metadata.is_dir() && crosses_boundary(&boundary, &metadata) => {}
            Ok(metadata) if metadata.is_dir() => {
                child_report.usage.add_dir();
                walk(&path, options, &mut child_report, &mut seen, &boundary)?;
            }
            Ok(metadata) => {
                child_report.usage.add_file(disk_bytes(&metadata, &mut seen));
            }
            Err(err) => {
                handle_error(&path, err, options, &mut child_report)?;
            }
        }

        breakdown.total.usage += child_report.usage;
        breakdown.entries.push(TopLevelEntry {
            name: entry.file_name(),
            report: child_report,
        });
    }

    Ok(breakdown)
}

fn walk(
    dir: &Path,
    options: &ScanOptions,
    report: &mut ScanReport,
    seen: &mut Seen,
    boundary: &Boundary,
) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => return handle_error(dir, err, options, report),
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                handle_error(dir, err, options, report)?;
                continue;
            }
        };

        let path = entry.path();
        let metadata = match read_metadata(&path, options) {
            Ok(metadata) => metadata,
            Err(err) => {
                handle_error(&path, err, options, report)?;
                continue;
            }
        };

        if metadata.is_dir() {
            if crosses_boundary(boundary, &metadata) {
                continue;
            }
            report.usage.add_dir();
            walk(&path, options, report, seen, boundary)?;
        } else {
            report.usage.add_file(disk_bytes(&metadata, seen));
        }
    }

    Ok(())
}

/// The device `root`'s own metadata reports, if `ScanOptions::one_filesystem`
/// is set; `None` (or, on non-Unix, the unit no-op) otherwise, meaning
/// nothing should be excluded.
#[cfg(unix)]
fn root_device(metadata: &fs::Metadata, options: &ScanOptions) -> Boundary {
    if options.one_filesystem {
        Some(metadata.dev())
    } else {
        None
    }
}

#[cfg(not(unix))]
fn root_device(_metadata: &fs::Metadata, _options: &ScanOptions) -> Boundary {}

/// Whether `metadata` lives on a different device than the scan started on.
/// Always `false` when `boundary` is `None` (the option is off) or on
/// non-Unix platforms, where there's nothing to compare.
#[cfg(unix)]
fn crosses_boundary(boundary: &Boundary, metadata: &fs::Metadata) -> bool {
    matches!(boundary, Some(root_dev) if *root_dev != metadata.dev())
}

#[cfg(not(unix))]
fn crosses_boundary(_boundary: &Boundary, _metadata: &fs::Metadata) -> bool {
    false
}

fn read_metadata(path: &Path, options: &ScanOptions) -> io::Result<fs::Metadata> {
    if options.follow_symlinks {
        fs::metadata(path)
    } else {
        fs::symlink_metadata(path)
    }
}

/// Route an error either to the caller (strict) or into the report (lenient).
fn handle_error(
    path: &Path,
    error: io::Error,
    options: &ScanOptions,
    report: &mut ScanReport,
) -> io::Result<()> {
    if options.lenient {
        report.skipped.push(SkippedEntry {
            path: path.to_path_buf(),
            error,
        });
        Ok(())
    } else {
        Err(error)
    }
}

/// Bytes actually occupied on disk, not the apparent length. Sparse files
/// and filesystem block rounding mean these can differ a lot from
/// `metadata.len()`.
///
/// A file with more than one hard link shares its blocks with every other
/// name pointing at the same inode. The first name `disk_bytes` sees for a
/// given (device, inode) pair gets the real figure; every later name for
/// that same inode gets `0`, so a tree with hard-linked files doesn't have
/// their shared space counted once per name.
#[cfg(unix)]
fn disk_bytes(metadata: &fs::Metadata, seen: &mut Seen) -> u64 {
    let bytes = metadata.blocks() * 512;
    if metadata.nlink() > 1 && !seen.insert((metadata.dev(), metadata.ino())) {
        return 0;
    }
    bytes
}

#[cfg(not(unix))]
fn disk_bytes(metadata: &fs::Metadata, _seen: &mut Seen) -> u64 {
    metadata.len()
}
