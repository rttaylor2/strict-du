//! Human-readable formatting for the byte counts `scan` produces.

const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

/// Format a byte count the way `du -h` does: binary units (1024, not 1000),
/// one decimal place once we're past whole bytes.
///
/// `du -h` is the reference point here because this crate measures the same
/// thing `du` does (blocks actually allocated), so the units should agree
/// with what people already read off that tool.
pub fn human_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{} B", bytes);
    }

    // u64::MAX is a bit under 16 EiB, so the unit table never runs out
    // before the loop's divisions bring `size` under 1024.
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }

    format!("{:.1} {}", size, UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_counts_have_no_decimal() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1), "1 B");
        assert_eq!(human_bytes(1023), "1023 B");
    }

    #[test]
    fn crosses_unit_boundaries() {
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1024 + 512), "1.5 KiB");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(human_bytes(1024 * 1024 * 1024), "1.0 GiB");
    }

    #[test]
    fn caps_at_exbibytes_instead_of_overflowing_the_unit_table() {
        assert_eq!(human_bytes(u64::MAX), "16.0 EiB");
    }
}
