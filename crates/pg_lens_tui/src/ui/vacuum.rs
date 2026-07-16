//! Vacuum health / XID wraparound severity (F2) — shared by the Schema
//! Lens's "Vacuum / wraparound" section and the Macro Lens's one-line
//! banner, so both surfaces read from exactly one set of thresholds.
//!
//! Thresholds: yellow past `autovacuum_freeze_max_age`'s default (200M —
//! the point at which autovacuum starts forcing freezes specifically to
//! fight wraparound), red past 500M (a quarter of the way to the ~2.1
//! billion forced-shutdown point, and comfortably past "this needs
//! attention now").

use ratatui::style::Color;

/// Autovacuum starts forcing freeze scans past this age by default.
pub const WARN_AGE_XIDS: i64 = 200_000_000;
/// Well past the point where an operator should already be worried; a
/// quarter of the distance to the ~2.1 billion forced-shutdown ceiling.
pub const BAD_AGE_XIDS: i64 = 500_000_000;

/// Severity tier of one `age(datfrozenxid)`/`age(relfrozenxid)` reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Ok,
    Warn,
    Bad,
}

pub fn age_severity(age_xids: i64) -> Severity {
    if age_xids > BAD_AGE_XIDS {
        Severity::Bad
    } else if age_xids > WARN_AGE_XIDS {
        Severity::Warn
    } else {
        Severity::Ok
    }
}

impl Severity {
    /// 2-char textual marker (like the Schema Lens bloat/Macro Lens lag
    /// markers) so severity is provable in VT captures without color.
    pub fn marker(self) -> &'static str {
        match self {
            Severity::Ok => "  ",
            Severity::Warn => "! ",
            Severity::Bad => "!!",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Severity::Ok => Color::Green,
            Severity::Warn => Color::Yellow,
            Severity::Bad => Color::Red,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_tiers_match_the_spec_thresholds() {
        assert_eq!(age_severity(0), Severity::Ok);
        assert_eq!(age_severity(199_999_999), Severity::Ok);
        assert_eq!(age_severity(200_000_000), Severity::Ok, "boundary is not yet warn");
        assert_eq!(age_severity(200_000_001), Severity::Warn);
        assert_eq!(age_severity(499_999_999), Severity::Warn);
        assert_eq!(age_severity(500_000_000), Severity::Warn, "boundary is not yet bad");
        assert_eq!(age_severity(500_000_001), Severity::Bad);
        assert_eq!(age_severity(2_100_000_000), Severity::Bad);
    }

    #[test]
    fn markers_and_colors_are_distinct_per_tier() {
        for sev in [Severity::Ok, Severity::Warn, Severity::Bad] {
            assert_eq!(sev.marker().len(), 2);
        }
        assert_ne!(Severity::Ok.color(), Severity::Warn.color());
        assert_ne!(Severity::Warn.color(), Severity::Bad.color());
        assert_ne!(Severity::Ok.color(), Severity::Bad.color());
    }
}
