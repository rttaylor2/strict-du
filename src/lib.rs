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

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

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
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            lenient: false,
            follow_symlinks: false,
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

    let metadata = read_metadata(root, options);
    match metadata {
        Ok(metadata) if metadata.is_dir() => {
            walk(root, options, &mut report)?;
        }
        Ok(metadata) => {
            report.usage.add_file(disk_bytes(&metadata));
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
pub fn scan_top_level(root: &Path, options: &ScanOptions) -> io::Result<TopLevelBreakdown> {
    let mut breakdown = TopLevelBreakdown::default();

    let metadata = match read_metadata(root, options) {
        Ok(metadata) => metadata,
        Err(err) => {
            handle_error(root, err, options, &mut breakdown.total)?;
            return Ok(breakdown);
        }
    };

    if !metadata.is_dir() {
        let mut report = ScanReport::default();
        report.usage.add_file(disk_bytes(&metadata));
        breakdown.total.usage += report.usage;
        breakdown.entries.push(TopLevelEntry {
            name: root.file_name().map(OsString::from).unwrap_or_default(),
            report,
        });
        return Ok(breakdown);
    }

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
            Ok(metadata) if metadata.is_dir() => {
                child_report.usage.add_dir();
                walk(&path, options, &mut child_report)?;
            }
            Ok(metadata) => {
                child_report.usage.add_file(disk_bytes(&metadata));
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

fn walk(dir: &Path, options: &ScanOptions, report: &mut ScanReport) -> io::Result<()> {
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
            report.usage.add_dir();
            walk(&path, options, report)?;
        } else {
            report.usage.add_file(disk_bytes(&metadata));
        }
    }

    Ok(())
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
#[cfg(unix)]
fn disk_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.blocks() * 512
}

#[cfg(not(unix))]
fn disk_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.len()
}
