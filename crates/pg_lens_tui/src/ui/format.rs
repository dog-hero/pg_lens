//! Pure formatting helpers for the view layer (Fase 4).
//!
//! No I/O, no state — every function here is a total `value → String`
//! mapping, unit-tested below. Used by the header, the Macro Lens vitals and
//! the Micro Lens table.

/// Compact human duration for query/session ages: `980ms`, `12s`, `4m32s`,
/// `1h04m`. Negative inputs (clock skew between `now()` and `query_start`)
/// clamp to `0s`.
pub fn human_duration(secs: f64) -> String {
    if !secs.is_finite() || secs <= 0.0 {
        return "0s".to_string();
    }
    if secs < 1.0 {
        return format!("{:.0}ms", secs * 1_000.0);
    }
    let total = secs as u64;
    if total < 60 {
        format!("{total}s")
    } else if total < 3_600 {
        format!("{}m{:02}s", total / 60, total % 60)
    } else if total < 86_400 {
        format!("{}h{:02}m", total / 3_600, (total % 3_600) / 60)
    } else {
        format!("{}d{:02}h", total / 86_400, (total % 86_400) / 3_600)
    }
}

/// Server uptime for the header: `3d 4h`, `4h 27m`, `27m`, `42s`.
pub fn human_uptime(secs: u64) -> String {
    let (days, hours, mins) = (secs / 86_400, (secs % 86_400) / 3_600, (secs % 3_600) / 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m")
    } else {
        format!("{secs}s")
    }
}

/// Human byte size: `512 B`, `3.4 MB`, `1.2 GB` (1024-based). Negative
/// inputs (defensive: the counters are `i64`) clamp to `0 B`.
pub fn human_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes.max(0) as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", value as u64)
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Truncates `text` to at most `width` characters, spending the last one on
/// an ellipsis when something was cut. `width == 0` yields an empty string.
pub fn truncate_with_ellipsis(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(width - 1).collect();
    out.push('\u{2026}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_covers_all_magnitudes() {
        assert_eq!(human_duration(0.98), "980ms");
        assert_eq!(human_duration(12.7), "12s");
        assert_eq!(human_duration(4.0 * 60.0 + 32.0), "4m32s");
        assert_eq!(human_duration(3_600.0 + 4.0 * 60.0), "1h04m");
        assert_eq!(human_duration(2.0 * 86_400.0 + 3.0 * 3_600.0), "2d03h");
    }

    #[test]
    fn duration_clamps_negatives_and_nan_to_zero() {
        assert_eq!(human_duration(-0.002), "0s");
        assert_eq!(human_duration(-500.0), "0s");
        assert_eq!(human_duration(f64::NAN), "0s");
        assert_eq!(human_duration(0.0), "0s");
    }

    #[test]
    fn uptime_is_human() {
        assert_eq!(human_uptime(42), "42s");
        assert_eq!(human_uptime(27 * 60), "27m");
        assert_eq!(human_uptime(4 * 3_600 + 27 * 60), "4h 27m");
        assert_eq!(human_uptime(3 * 86_400 + 4 * 3_600), "3d 4h");
    }

    #[test]
    fn bytes_pick_the_right_unit() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(3 * 1024 * 1024 + 400 * 1024), "3.4 MB");
        assert_eq!(human_bytes((1.2 * 1024.0 * 1024.0 * 1024.0) as i64), "1.2 GB");
        assert_eq!(human_bytes(-7), "0 B");
    }

    #[test]
    fn truncation_is_explicit_and_char_safe() {
        assert_eq!(truncate_with_ellipsis("SELECT 1", 20), "SELECT 1");
        assert_eq!(truncate_with_ellipsis("SELECT pg_sleep(60)", 10), "SELECT pg\u{2026}");
        assert_eq!(truncate_with_ellipsis("exact", 5), "exact");
        assert_eq!(truncate_with_ellipsis("caf\u{e9} au lait", 5), "caf\u{e9}\u{2026}");
        assert_eq!(truncate_with_ellipsis("anything", 0), "");
        assert_eq!(truncate_with_ellipsis("anything", 1), "\u{2026}");
    }
}
