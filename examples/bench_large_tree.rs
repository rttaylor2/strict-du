//! Builds a synthetic tree and times `scan` / `scan_top_level` against it.
//!
//! Run with:
//!
//! ```text
//! cargo run --release --example bench_large_tree -- [file_count] [fanout]
//! ```
//!
//! Defaults to 50,000 files with a branching factor of 20. `fanout` controls
//! both how many files land in each directory before a new one is opened and
//! how many subdirectories each directory gets, so depth stays logarithmic in
//! `file_count` instead of the walk recursing one stack frame per file.

use std::collections::VecDeque;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use strict_du::{scan, scan_top_level, ScanOptions};

fn main() {
    let mut args = env::args().skip(1);
    let file_count: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(50_000);
    let fanout: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(20);

    let root = unique_temp_dir();
    println!(
        "building {} files under {} (fanout {})...",
        file_count,
        root.display(),
        fanout
    );
    let build_start = Instant::now();
    build_tree(&root, file_count, fanout);
    println!("build took {:.2?}", build_start.elapsed());

    let options = ScanOptions::default();

    let scan_start = Instant::now();
    let report = scan(&root, &options).expect("scan failed");
    let scan_elapsed = scan_start.elapsed();
    println!(
        "scan: {} files, {} dirs, {} in {:.2?} ({:.0} files/sec)",
        report.usage.files,
        report.usage.dirs,
        report.usage.human_bytes(),
        scan_elapsed,
        report.usage.files as f64 / scan_elapsed.as_secs_f64(),
    );

    let top_level_start = Instant::now();
    let breakdown = scan_top_level(&root, &options).expect("scan_top_level failed");
    let top_level_elapsed = top_level_start.elapsed();
    println!(
        "scan_top_level: {} top-level entries in {:.2?}",
        breakdown.entries.len(),
        top_level_elapsed,
    );

    fs::remove_dir_all(&root).expect("cleanup failed");
}

/// Spreads `file_count` files breadth-first across directories with up to
/// `fanout` files and `fanout` subdirectories each, so the tree's depth grows
/// with log(file_count) rather than linearly. A flat directory with every
/// file dumped in it, or a directory-per-file chain, would both scan very
/// differently than the mixed trees `strict-du` actually gets pointed at.
fn build_tree(root: &Path, file_count: u64, fanout: u64) {
    fs::create_dir_all(root).expect("create root");

    let mut counter: u64 = 0;
    let mut queue = VecDeque::new();
    queue.push_back(root.to_path_buf());

    while counter < file_count {
        let dir = queue.pop_front().expect("ran out of directories before files");

        let files_here = fanout.min(file_count - counter);
        for _ in 0..files_here {
            let path = dir.join(format!("f{}.txt", counter));
            fs::write(&path, counter.to_le_bytes()).expect("write file");
            counter += 1;
        }

        if counter < file_count {
            for i in 0..fanout {
                let sub = dir.join(format!("d{}", i));
                fs::create_dir(&sub).expect("create subdir");
                queue.push_back(sub);
            }
        }
    }
}

fn unique_temp_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    env::temp_dir().join(format!("strict-du-bench-{}", nanos))
}
