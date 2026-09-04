use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use strict_du::{scan, ScanOptions};

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
