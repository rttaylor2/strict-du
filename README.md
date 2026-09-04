# strict-du

A Rust library that walks a directory tree and totals up real on-disk usage.

## The problem

Two things are easy to get wrong when you write your own `du`:

1. **Apparent size vs. real size.** `metadata.len()` tells you how big a
   file looks, not how much disk it costs. A 10 GB sparse file might occupy
   almost nothing on disk. `strict-du` sums the space the filesystem
   actually reports as allocated (`st_blocks * 512` on Unix), not the
   logical length.
2. **Errors during the walk.** Permission denied on a subdirectory, a file
   that vanishes mid-scan, a broken symlink — every tool has to decide what
   to do here. Most either die on the first one, or quietly skip it and
   hand back a total that looks complete but isn't. Silently wrong totals
   are worse than a crash, so `strict-du` fails by default: any I/O error
   aborts the scan and you get that error back. If a partial answer is
   actually useful to you, you ask for it explicitly.

## Usage

```rust
use strict_du::{scan, ScanOptions};
use std::path::Path;

// Strict by default: the first unreadable file or directory returns
// an error instead of a possibly-wrong total.
let report = scan(Path::new("/var/log"), &ScanOptions::default())?;
println!("{} bytes across {} files", report.usage.bytes, report.usage.files);
# Ok::<(), std::io::Error>(())
```

To get a best-effort total instead of an error, opt into lenient mode
explicitly:

```rust
use strict_du::{scan, ScanOptions};
use std::path::Path;

let options = ScanOptions {
    lenient: true,
    ..ScanOptions::default()
};

let report = scan(Path::new("/var/log"), &options)?;
println!("{} bytes counted", report.usage.bytes);
for skipped in &report.skipped {
    eprintln!("skipped {}: {}", skipped.path.display(), skipped.error);
}
# Ok::<(), std::io::Error>(())
```

`ScanOptions` also controls whether symlinks are followed
(`follow_symlinks`, off by default — following symlinks can double-count
space shared between two parts of a tree, or loop on a cycle).

## Status

Early skeleton. The scanning core works and is tested; see the roadmap in
the repo history for what's planned next (human-readable formatting,
per-top-level-entry breakdowns, hardlink dedup).

## License

MIT, see [LICENSE](LICENSE).
