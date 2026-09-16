use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use strict_du::{scan, scan_top_level, ScanOptions};

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("strict-du-test-{}-{}", label, nanos));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn counts_files_and_dirs() {
    let root = unique_temp_dir("counts");
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), b"hello").unwrap();
    fs::write(root.join("sub").join("b.txt"), b"world!!").unwrap();

    let report = scan(&root, &ScanOptions::default()).unwrap();

    assert_eq!(report.usage.files, 2);
    assert_eq!(report.usage.dirs, 1);
    assert!(report.skipped.is_empty());

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn single_file_root_is_measured_directly() {
    let root = unique_temp_dir("single-file-parent");
    let file = root.join("only.txt");
    fs::write(&file, b"contents").unwrap();

    let report = scan(&file, &ScanOptions::default()).unwrap();

    assert_eq!(report.usage.files, 1);
    assert_eq!(report.usage.dirs, 0);

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn strict_mode_fails_on_missing_root() {
    let root = std::env::temp_dir().join("strict-du-test-does-not-exist");
    let result = scan(&root, &ScanOptions::default());
    assert!(result.is_err());
}

#[test]
fn lenient_mode_records_missing_root_instead_of_failing() {
    let root = std::env::temp_dir().join("strict-du-test-does-not-exist-lenient");
    let options = ScanOptions {
        lenient: true,
        ..ScanOptions::default()
    };

    let report = scan(&root, &options).unwrap();

    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].path, root);
}

#[test]
fn top_level_breakdown_has_one_entry_per_child() {
    let root = unique_temp_dir("breakdown");
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("sub").join("b.txt"), b"world!!").unwrap();
    fs::write(root.join("a.txt"), b"hello").unwrap();

    let breakdown = scan_top_level(&root, &ScanOptions::default()).unwrap();

    assert_eq!(breakdown.entries.len(), 2);

    let sub = breakdown
        .entries
        .iter()
        .find(|e| e.name == "sub")
        .expect("sub entry present");
    assert_eq!(sub.report.usage.files, 1);
    assert_eq!(sub.report.usage.dirs, 1);

    let a = breakdown
        .entries
        .iter()
        .find(|e| e.name == "a.txt")
        .expect("a.txt entry present");
    assert_eq!(a.report.usage.files, 1);
    assert_eq!(a.report.usage.dirs, 0);

    let full_scan = scan(&root, &ScanOptions::default()).unwrap();
    assert_eq!(breakdown.total.usage, full_scan.usage);

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn top_level_breakdown_on_a_file_root_has_a_single_entry() {
    let root = unique_temp_dir("breakdown-single-file");
    let file = root.join("only.txt");
    fs::write(&file, b"contents").unwrap();

    let breakdown = scan_top_level(&file, &ScanOptions::default()).unwrap();

    assert_eq!(breakdown.entries.len(), 1);
    assert_eq!(breakdown.entries[0].name, "only.txt");
    assert_eq!(breakdown.entries[0].report.usage.files, 1);

    fs::remove_dir_all(&root).unwrap();
}

#[test]
#[cfg(unix)]
fn hardlinked_files_are_only_counted_once() {
    let root = unique_temp_dir("hardlinks");
    let original = root.join("a.txt");
    fs::write(&original, b"hello world").unwrap();
    fs::hard_link(&original, root.join("b.txt")).unwrap();

    let report = scan(&root, &ScanOptions::default()).unwrap();
    let single_file = scan(&original, &ScanOptions::default()).unwrap();

    // Two directory entries, but the shared inode's disk usage should only
    // be charged once, so the tree's total matches a scan of just one name.
    assert_eq!(report.usage.files, 2);
    assert_eq!(report.usage.bytes, single_file.usage.bytes);

    fs::remove_dir_all(&root).unwrap();
}

#[test]
#[cfg(unix)]
fn top_level_breakdown_dedupes_hardlinks_across_entries() {
    let root = unique_temp_dir("breakdown-hardlinks");
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), b"hello world").unwrap();
    fs::hard_link(root.join("a.txt"), root.join("sub").join("b.txt")).unwrap();

    let breakdown = scan_top_level(&root, &ScanOptions::default()).unwrap();
    let full_scan = scan(&root, &ScanOptions::default()).unwrap();

    // Deduplication has to share state across entries, not just within
    // one, or this total would double-count the linked file's blocks.
    assert_eq!(breakdown.total.usage, full_scan.usage);

    fs::remove_dir_all(&root).unwrap();
}

#[test]
#[cfg(unix)]
fn top_level_breakdown_attributes_skips_to_the_owning_entry() {
    // A broken symlink is a reliable way to make metadata() fail for one
    // specific, known child, without racing a delete against the scan.
    let root = unique_temp_dir("breakdown-lenient");
    fs::write(root.join("a.txt"), b"hello").unwrap();
    std::os::unix::fs::symlink(root.join("does-not-exist"), root.join("ghost")).unwrap();

    let options = ScanOptions {
        lenient: true,
        follow_symlinks: true,
        ..ScanOptions::default()
    };

    let breakdown = scan_top_level(&root, &options).unwrap();

    assert!(breakdown.total.skipped.is_empty());

    let ghost_entry = breakdown
        .entries
        .iter()
        .find(|e| e.name == "ghost")
        .expect("ghost entry present");
    assert_eq!(ghost_entry.report.skipped.len(), 1);
    assert_eq!(ghost_entry.report.usage, strict_du::DiskUsage::default());

    let a_entry = breakdown
        .entries
        .iter()
        .find(|e| e.name == "a.txt")
        .expect("a.txt entry present");
    assert!(a_entry.report.skipped.is_empty());

    fs::remove_dir_all(&root).unwrap();
}
