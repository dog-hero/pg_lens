//! Shared replication rendering helpers (U1). Both the Macro Lens's compact,
//! capped panel and the full Replication Lens build their rows from the same
//! severity math and the same one-line formatters, so the two views can
//! never disagree about what counts as a warning.
//!
//! Slot severity ranking itself lives in `crate::app` (pure core logic, no
//! ratatui) so it can double as the Replication Lens's sort key — this
//! module only adds the marker/color mapping on top.

use pg_lens_core::{ReplicationSlotRow, WalReceiverRow, WalSenderRow};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::app::slot_severity_rank;
use crate::ui::{format, style};

/// Lag/slot severity tier. Yellow > 10 MB or > 10 s (either dimension trips
/// it), red > 100 MB or > 60 s — used for sender/receiver lag; slots use
/// [`slot_severity`] instead (a different, WAL-retention-based rule).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Severity {
    Ok,
    Warn,
    Bad,
}

impl Severity {
    /// 1-char textual marker (like the Micro Lens B/W markers) so severity
    /// is provable in VT captures without relying on color.
    pub(crate) fn marker(self) -> &'static str {
        match self {
            Severity::Ok => "  ",
            Severity::Warn => "! ",
            Severity::Bad => "!!",
        }
    }
    pub(crate) fn color(self) -> Color {
        match self {
            Severity::Ok => Color::Green,
            Severity::Warn => Color::Yellow,
            Severity::Bad => Color::Red,
        }
    }
}

/// 0 bytes outstanding = definitively caught up. The seconds measure on the
/// standby side is `now() - pg_last_xact_replay_timestamp()`, which grows
/// unboundedly on an idle primary even when the standby is perfectly in
/// sync — so it must never raise an alarm on its own.
pub(crate) fn lag_severity(bytes: Option<i64>, secs: Option<f64>) -> Severity {
    if bytes == Some(0) {
        return Severity::Ok;
    }
    let b = bytes.unwrap_or(0);
    let s = secs.unwrap_or(0.0);
    if b > 100 * 1024 * 1024 || s > 60.0 {
        Severity::Bad
    } else if b > 10 * 1024 * 1024 || s > 10.0 {
        Severity::Warn
    } else {
        Severity::Ok
    }
}

/// Formats the two lag measures as `12 MB · 1.2s`, `—` when both absent.
pub(crate) fn lag_text(bytes: Option<i64>, secs: Option<f64>) -> String {
    match (bytes, secs) {
        (Some(b), Some(s)) => {
            format!("{} · {}", format::human_bytes(b), format::human_duration(s))
        }
        (Some(b), None) => format::human_bytes(b),
        (None, Some(s)) => format::human_duration(s),
        (None, None) => "—".to_string(),
    }
}

pub(crate) fn sender_line(s: &WalSenderRow) -> Line<'static> {
    let sev = lag_severity(s.replay_lag_bytes, s.replay_lag_secs);
    Line::from(vec![
        Span::styled(format!("{} ", sev.marker()), Style::new().fg(sev.color())),
        Span::styled(
            format!("{}/{}", s.application_name, s.client),
            style::accent_style(),
        ),
        Span::styled(
            format!("  {}/{}  ", s.state, s.sync_state),
            style::label_style(),
        ),
        Span::styled("lag: ", style::label_style()),
        Span::styled(
            lag_text(s.replay_lag_bytes, s.replay_lag_secs),
            Style::new().fg(sev.color()),
        ),
    ])
}

pub(crate) fn receiver_line(r: &WalReceiverRow) -> Line<'static> {
    let sev = lag_severity(r.replay_lag_bytes, r.replay_lag_secs);
    let upstream = match (&r.sender_host, r.sender_port) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.clone(),
        _ => "upstream".to_string(),
    };
    Line::from(vec![
        Span::styled(format!("{} ", sev.marker()), Style::new().fg(sev.color())),
        Span::styled("standby", style::accent_style()),
        Span::styled(format!("  {}  ", r.status), style::label_style()),
        Span::styled(format!("from {upstream}  "), style::value_style()),
        Span::styled("replay lag: ", style::label_style()),
        Span::styled(
            lag_text(r.replay_lag_bytes, r.replay_lag_secs),
            Style::new().fg(sev.color()),
        ),
    ])
}

/// Severity of one replication slot (F2.5) — a thin display-side wrapper
/// over [`crate::app::slot_severity_rank`], the single source of truth also
/// used to sort the Replication Lens's table. The point of the underlying
/// rule: an INACTIVE slot that keeps retaining WAL is the classic full-disk
/// incident — nothing is consuming it, so WAL piles up in `pg_wal` until the
/// disk fills.
pub(crate) fn slot_severity(slot: &ReplicationSlotRow) -> Severity {
    match slot_severity_rank(slot) {
        0 => Severity::Bad,
        1 => Severity::Warn,
        _ => Severity::Ok,
    }
}

pub(crate) fn slot_line(slot: &ReplicationSlotRow) -> Line<'static> {
    let sev = slot_severity(slot);
    let retained = match slot.retained_wal_bytes {
        Some(b) => format::human_bytes(b),
        None => "—".to_string(),
    };
    let active_text = if slot.active { "active" } else { "inactive" };
    let status = slot.wal_status.as_deref().unwrap_or("—");
    Line::from(vec![
        Span::styled(format!("{} ", sev.marker()), Style::new().fg(sev.color())),
        Span::styled(
            format!("slot {}/{}", slot.slot_name, slot.slot_type),
            style::accent_style(),
        ),
        Span::styled(format!("  {active_text}  "), style::label_style()),
        Span::styled("retained: ", style::label_style()),
        Span::styled(retained, Style::new().fg(sev.color())),
        Span::styled(format!("  ({status})"), style::label_style()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_tiers_by_bytes_and_secs() {
        assert!(matches!(lag_severity(Some(0), Some(0.0)), Severity::Ok));
        assert!(matches!(
            lag_severity(Some(20 * 1024 * 1024), None),
            Severity::Warn
        ));
        assert!(matches!(lag_severity(None, Some(12.0)), Severity::Warn));
        assert!(matches!(
            lag_severity(Some(200 * 1024 * 1024), None),
            Severity::Bad
        ));
        assert!(matches!(lag_severity(None, Some(90.0)), Severity::Bad));
    }

    #[test]
    fn zero_bytes_is_caught_up_regardless_of_the_stale_time_measure() {
        // Idle-primary case: 0 bytes outstanding but the last-replay age is
        // minutes old — must stay OK, not flag red.
        assert!(matches!(lag_severity(Some(0), Some(240.0)), Severity::Ok));
    }

    #[test]
    fn lag_text_handles_missing_measures() {
        assert_eq!(lag_text(None, None), "—");
        assert!(lag_text(Some(1024 * 1024), Some(1.5)).contains('·'));
    }

    fn slot(
        active: bool,
        wal_status: Option<&str>,
        retained_wal_bytes: Option<i64>,
    ) -> ReplicationSlotRow {
        ReplicationSlotRow {
            slot_name: "probe_slot".to_string(),
            slot_type: "physical".to_string(),
            active,
            retained_wal_bytes,
            wal_status: wal_status.map(str::to_string),
            safe_wal_size: None,
        }
    }

    #[test]
    fn active_reserved_slot_is_calm() {
        assert!(matches!(
            slot_severity(&slot(true, Some("reserved"), Some(0))),
            Severity::Ok
        ));
        // Active is calm even while retaining a lot — it's a live replica
        // consuming the WAL, not an abandoned one.
        assert!(matches!(
            slot_severity(&slot(true, Some("reserved"), Some(20 * 1024 * 1024 * 1024))),
            Severity::Ok
        ));
    }

    #[test]
    fn inactive_slot_retaining_wal_is_yellow_then_red() {
        assert!(
            matches!(
                slot_severity(&slot(false, Some("extended"), Some(0))),
                Severity::Ok
            ),
            "inactive but retaining nothing stays calm"
        );
        assert!(matches!(
            slot_severity(&slot(false, Some("extended"), Some(1024))),
            Severity::Warn
        ));
        assert!(matches!(
            slot_severity(&slot(
                false,
                Some("extended"),
                Some(11 * 1024 * 1024 * 1024)
            )),
            Severity::Bad
        ));
    }

    #[test]
    fn unreserved_or_lost_wal_status_is_always_red() {
        assert!(matches!(
            slot_severity(&slot(false, Some("unreserved"), Some(1024))),
            Severity::Bad
        ));
        assert!(matches!(
            slot_severity(&slot(false, Some("lost"), None)),
            Severity::Bad
        ));
        // Even an active slot: unreserved/lost is a red flag on its own.
        assert!(matches!(
            slot_severity(&slot(true, Some("unreserved"), Some(0))),
            Severity::Bad
        ));
    }
}
