# strict-du

A Rust library that walks a directory tree and totals up real on-disk usage.

## The problem

Two things are easy to get wrong when you write your own `du`:

1. **Apparent size vs. real size.** `metadata.len()` tells you how big a
   file looks, not how much disk it costs. A 10 GB sparse file might occupy
   almost nothing on disk. `strict-du` sums the space the filesystem
   actually reports as allocated (`st_blocks * 512` on Unix), not the
   logical length. On Unix this also means hard-linked files are only
   charged once: two names for the same inode share the same blocks, and
   `strict-du` tracks which inodes it's already counted so the total
   doesn't overstate the tree's real disk footprint.
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
println!("{} across {} files", report.usage.human_bytes(), report.usage.files);
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
space shared between two parts of a tree, or loop on a cycle), and whether
the scan stays on the filesystem `root` started on (`one_filesystem`, off
by default). Turning `one_filesystem` on makes the scan skip any directory
whose device differs from `root`'s, the same way `du --one-file-system`
does — useful for totaling a single disk without wandering onto a mounted
network share.

Byte counts are raw `u64`s; `human_bytes` (also available as
`DiskUsage::human_bytes`) formats them the way `du -h` does, e.g. `"4.2 MiB"`,
using binary (1024-based) units.

For a `du -d1`-style breakdown — a subtotal per immediate child of `root`,
plus the same grand total `scan` would produce — use `scan_top_level`:

```rust
use strict_du::{scan_top_level, ScanOptions};
use std::path::Path;

let breakdown = scan_top_level(Path::new("/var/log"), &ScanOptions::default())?;
for entry in &breakdown.entries {
    println!("{:>10}  {}", entry.report.usage.human_bytes(), entry.name.to_string_lossy());
}
println!("{:>10}  total", breakdown.total.usage.human_bytes());
# Ok::<(), std::io::Error>(())
```

In lenient mode, an error while measuring one child is recorded on that
child's own `entry.report.skipped` rather than merged into
`breakdown.total.skipped`, so you can tell which entry it came from.

## Status

The scanning core and the top-level breakdown both work and are tested,
including hardlink deduplication and filesystem-boundary detection.

`examples/bench_large_tree.rs` builds a synthetic tree and times `scan` and
`scan_top_level` against it:

```sh
cargo run --release --example bench_large_tree -- 200000 30
```

The two arguments (file count, branching factor) are both optional and
default to 50,000 and 20. Deciding whether a streaming/incremental API is
worth the complexity, informed by these numbers, is next.

## License

MIT, see [LICENSE](LICENSE).
