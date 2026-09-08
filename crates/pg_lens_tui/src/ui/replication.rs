//! Shared replication rendering helpers (U1). Both the Macro Lens's compact,
//! capped panel and the full Replication Lens build their rows from the same
//! severity math and the same one-line formatters, so the two views can
//! never disagree about what counts as a warning.
//!
//! Slot severity ranking itself lives in `crate::app` (pure core logic, no
//! ratatui) so it can double as the Replication Lens's sort key — this
//! module only adds the marker/color mapping on top.

use pg_lens_core::{ReplicationSlotRow, WalReceiverRow, WalSenderRow, WalStats};
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
    let sev = lag_severity(s.total_lag_bytes.or(s.replay_lag_bytes), s.replay_lag_secs);
    let sync_desc = if s.sync_priority > 0 {
        format!("{}/{} (prio {})", s.state, s.sync_state, s.sync_priority)
    } else {
        format!("{}/{}", s.state, s.sync_state)
    };

    let mut spans = vec![
        Span::styled(format!("{} ", sev.marker()), Style::new().fg(sev.color())),
        Span::styled(
            format!("{}/{}", s.application_name, s.client),
            style::accent_style(),
        ),
        Span::styled(format!("  {sync_desc}  "), style::label_style()),
        Span::styled("lag: ", style::label_style()),
        Span::styled(
            lag_text(s.replay_lag_bytes, s.replay_lag_secs),
            Style::new().fg(sev.color()),
        ),
    ];

    if s.sent_lag_bytes.is_some() || s.write_lag_bytes.is_some() || s.flush_lag_bytes.is_some() {
        let sent_b = s.sent_lag_bytes.map(format::human_bytes).unwrap_or_else(|| "0 B".to_string());
        let write_b = s.write_lag_bytes.map(format::human_bytes).unwrap_or_else(|| "0 B".to_string());
        let flush_b = s.flush_lag_bytes.map(format::human_bytes).unwrap_or_else(|| "0 B".to_string());
        spans.push(Span::styled(
            format!("  [sent: {sent_b} \u{b7} write: {write_b} \u{b7} flush: {flush_b}]"),
            style::label_style(),
        ));
    }
    if let Some(total) = s.total_lag_bytes {
        spans.push(Span::styled(
            format!("  total: {}", format::human_bytes(total)),
            style::value_style(),
        ));
    }

    Line::from(spans)
}

pub(crate) fn receiver_line(r: &WalReceiverRow) -> Line<'static> {
    let sev = lag_severity(r.replay_lag_bytes, r.replay_lag_secs);
    let upstream = match (&r.sender_host, r.sender_port) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.clone(),
        _ => "upstream".to_string(),
    };
    let mut spans = vec![
        Span::styled(format!("{} ", sev.marker()), Style::new().fg(sev.color())),
        Span::styled("standby", style::accent_style()),
        Span::styled(format!("  {}  ", r.status), style::label_style()),
        Span::styled(format!("from {upstream}  "), style::value_style()),
        Span::styled("replay lag: ", style::label_style()),
        Span::styled(
            lag_text(r.replay_lag_bytes, r.replay_lag_secs),
            Style::new().fg(sev.color()),
        ),
    ];
    if r.is_paused {
        let state = r.pause_state.as_deref().unwrap_or("paused");
        spans.push(Span::styled(
            format!("  [{state}]"),
            Style::new().fg(Color::Red).bold(),
        ));
    }
    Line::from(spans)
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
    let mut spans = vec![
        Span::styled(format!("{} ", sev.marker()), Style::new().fg(sev.color())),
        Span::styled(
            format!("slot {}/{}", slot.slot_name, slot.slot_type),
            style::accent_style(),
        ),
        Span::styled(format!("  {active_text}  "), style::label_style()),
        Span::styled("retained: ", style::label_style()),
        Span::styled(retained, Style::new().fg(sev.color())),
        Span::styled(format!("  ({status})"), style::label_style()),
    ];
    if let Some(reason) = &slot.invalidated {
        spans.push(Span::styled(
            format!("  [invalidated: {reason}]"),
            Style::new().fg(Color::Red).bold(),
        ));
    }
    if let Some(xmin_age) = slot.xmin_age {
        if xmin_age > 10_000_000 {
            spans.push(Span::styled(
                format!("  [xmin age: {}]", format::human_count(xmin_age)),
                Style::new().fg(if xmin_age > 50_000_000 { Color::Red } else { Color::Yellow }).bold(),
            ));
        }
    }
    Line::from(spans)
}

pub(crate) fn publication_line(p: &pg_lens_core::PublicationRow) -> Line<'static> {
    let mut ops = Vec::new();
    if p.pubinsert { ops.push("ins"); }
    if p.pubupdate { ops.push("upd"); }
    if p.pubdelete { ops.push("del"); }
    if p.pubtruncate { ops.push("trunc"); }
    let ops_str = if ops.is_empty() { "none".to_string() } else { ops.join(",") };

    let tables_str = if p.all_tables {
        "all tables".to_string()
    } else {
        format!("{} table{}", p.table_count, if p.table_count == 1 { "" } else { "s" })
    };

    let mut spans = vec![
        Span::styled("  pub ", style::label_style()),
        Span::styled(p.pubname.clone(), style::accent_style()),
        Span::styled(format!(" ({})", p.owner), style::label_style()),
        Span::styled("  tables: ", style::label_style()),
        Span::styled(tables_str, style::value_style()),
        Span::styled("  ops: ", style::label_style()),
        Span::styled(ops_str, style::value_style()),
    ];
    if p.pubviaroot {
        spans.push(Span::styled("  (via_root)", style::label_style()));
    }
    if !p.published_tables.is_empty() {
        let sample = p.published_tables.join(", ");
        let truncated = if sample.len() > 40 {
            format!("{}[…]", &sample[..37])
        } else {
            sample
        };
        spans.push(Span::styled(format!("  [{truncated}]"), style::label_style()));
    }
    Line::from(spans)
}

pub(crate) fn subscription_line(s: &pg_lens_core::SubscriptionRow) -> Line<'static> {
    let has_errors = s.apply_error_count.unwrap_or(0) > 0 || s.sync_error_count.unwrap_or(0) > 0;
    let sev = if has_errors {
        Severity::Bad
    } else if !s.enabled {
        Severity::Warn
    } else {
        Severity::Ok
    };

    let status_str = if s.enabled { "enabled" } else { "disabled" };
    let worker_str = match s.worker_pid {
        Some(pid) => format!("worker: pid {pid}"),
        None => "worker: idle".to_string(),
    };
    let lsn_str = s.received_lsn.as_deref().unwrap_or("—");
    let pubs_str = s.publications.join(",");

    let mut spans = vec![
        Span::styled(format!("{} ", sev.marker()), Style::new().fg(sev.color())),
        Span::styled(format!("sub {}", s.subname), style::accent_style()),
        Span::styled(format!(" ({})", s.owner), style::label_style()),
        Span::styled(
            format!("  {status_str}  "),
            if s.enabled { style::value_style() } else { Style::new().fg(Color::Yellow) },
        ),
    ];

    if let (Some(host), Some(db)) = (&s.publisher_host, &s.publisher_dbname) {
        spans.push(Span::styled(format!("from {host}/{db}  "), style::label_style()));
    }

    if let Some(sync_commit) = &s.sync_commit {
        spans.push(Span::styled(format!("sync_commit: {sync_commit}  "), style::label_style()));
    }

    spans.extend([
        Span::styled(format!("{worker_str}  "), style::label_style()),
        Span::styled(format!("pubs: [{pubs_str}]  "), style::label_style()),
        Span::styled(format!("recv: {lsn_str}  "), style::value_style()),
        Span::styled(format!("tables: {}/{} ready", s.ready_tables, s.total_tables), style::value_style()),
    ]);

    if s.sync_tables > 0 {
        spans.push(Span::styled(
            format!(" ({} syncing)", s.sync_tables),
            Style::new().fg(Color::Yellow),
        ));
    }

    if !s.syncing_table_names.is_empty() {
        spans.push(Span::styled(
            format!("  [{}]", s.syncing_table_names.join(", ")),
            Style::new().fg(Color::Yellow),
        ));
    }

    if let Some(errs) = s.apply_error_count {
        if errs > 0 {
            spans.push(Span::styled(
                format!("  [apply errors: {errs}]"),
                Style::new().fg(Color::Red).bold(),
            ));
        }
    }
    if let Some(errs) = s.sync_error_count {
        if errs > 0 {
            spans.push(Span::styled(
                format!("  [sync errors: {errs}]"),
                Style::new().fg(Color::Red).bold(),
            ));
        }
    }

    Line::from(spans)
}

/// v0.16's WAL generation-rate severity: yellow only when `wal_buffers_full`
/// is ACTIVELY CLIMBING this tick (a real `wal_buffers` sizing signal, not
/// just a nonzero cumulative count from history) — never escalates to red,
/// a tuning nudge rather than an incident, mirroring the Macro Lens's
/// checkpoint-pressure rule (`checkpoint_pressure_severity`).
pub(crate) fn wal_buffers_full_severity(wal: &WalStats) -> Severity {
    match wal.wal_buffers_full_delta {
        Some(d) if d > 0 => Severity::Warn,
        _ => Severity::Ok,
    }
}

/// One-line WAL generation summary: bytes/s, records/s, and the
/// `wal_buffers_full` pressure signal — dashes for the rates before the
/// first delta window this session (never a misleading `0/s`). Shared by
/// the Macro Lens's compact vitals and the full Replication Lens, so both
/// views agree on the exact same numbers and severity.
pub(crate) fn wal_generation_line(wal: &WalStats) -> Line<'static> {
    let sev = wal_buffers_full_severity(wal);
    let bytes_rate = wal
        .wal_bytes_per_sec
        .map(|v| format!("{}/s", format::human_bytes(v.max(0.0) as i64)))
        .unwrap_or_else(|| "--".to_string());
    let records_rate = wal
        .wal_records_per_sec
        .map(|v| format!("{v:.0} rec/s"))
        .unwrap_or_else(|| "-- rec/s".to_string());
    let buffers_full = match wal.wal_buffers_full_delta {
        Some(d) if d > 0 => format!("{} (+{d} this tick)", wal.wal_buffers_full),
        _ => wal.wal_buffers_full.to_string(),
    };
    Line::from(vec![
        Span::styled(format!("{} ", sev.marker()), Style::new().fg(sev.color())),
        Span::styled("WAL generation: ", style::label_style()),
        Span::styled(format!("{bytes_rate} \u{b7} {records_rate}  "), style::value_style()),
        Span::styled("buffers_full: ", style::label_style()),
        Span::styled(buffers_full, Style::new().fg(sev.color())),
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
            plugin: None,
            slot_type: "physical".to_string(),
            database: None,
            temporary: false,
            active,
            active_pid: None,
            application_name: None,
            client_addr: None,
            restart_lsn: None,
            confirmed_flush_lsn: None,
            retained_wal_bytes,
            consumer_lag_bytes: None,
            wal_status: wal_status.map(str::to_string),
            safe_wal_size: None,
            xmin_age: None,
            catalog_xmin_age: None,
            two_phase: None,
            conflicting: None,
            invalidated: None,
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

    fn wal(bytes_per_sec: Option<f64>, buffers_full: i64, delta: Option<i64>) -> WalStats {
        WalStats {
            wal_records: 1_000_000,
            wal_fpi: 10_000,
            wal_bytes: 500_000_000,
            wal_buffers_full: buffers_full,
            wal_write_time_ms: Some(1_000.0),
            wal_sync_time_ms: Some(100.0),
            wal_bytes_per_sec: bytes_per_sec,
            wal_records_per_sec: bytes_per_sec.map(|_| 400.0),
            wal_buffers_full_delta: delta,
        }
    }

    #[test]
    fn wal_generation_is_calm_when_buffers_full_is_not_climbing() {
        assert!(matches!(
            wal_buffers_full_severity(&wal(Some(2_000_000.0), 0, Some(0))),
            Severity::Ok
        ));
        // Nonzero from history, but not climbing THIS tick — still calm.
        assert!(matches!(
            wal_buffers_full_severity(&wal(Some(2_000_000.0), 40, Some(0))),
            Severity::Ok
        ));
        // No delta window yet (first poll) — calm, not a fault.
        assert!(matches!(
            wal_buffers_full_severity(&wal(None, 0, None)),
            Severity::Ok
        ));
    }

    #[test]
    fn wal_generation_warns_when_buffers_full_is_actively_climbing() {
        assert!(matches!(
            wal_buffers_full_severity(&wal(Some(2_000_000.0), 15, Some(3))),
            Severity::Warn
        ));
    }

    #[test]
    fn wal_generation_line_dashes_rates_before_the_first_delta_window() {
        let line = wal_generation_line(&wal(None, 0, None));
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("--"), "{text}");
        assert!(text.contains("buffers_full: 0"), "{text}");
    }

    #[test]
    fn wal_generation_line_shows_the_climbing_delta() {
        let line = wal_generation_line(&wal(Some(2_000_000.0), 15, Some(3)));
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("buffers_full: 15 (+3 this tick)"), "{text}");
    }

    #[test]
    fn receiver_line_shows_paused_badge() {
        let rec = WalReceiverRow {
            status: "streaming".to_string(),
            sender_host: Some("primary.internal".to_string()),
            sender_port: Some(5432),
            replay_lag_bytes: Some(0),
            replay_lag_secs: Some(0.0),
            is_paused: true,
            pause_state: Some("paused".to_string()),
        };
        let line = receiver_line(&rec);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("[paused]"), "{text}");
    }

    #[test]
    fn publication_line_renders_ops_and_tables() {
        let pub_row = pg_lens_core::PublicationRow {
            pubname: "test_pub".to_string(),
            owner: "admin".to_string(),
            all_tables: false,
            pubinsert: true,
            pubupdate: true,
            pubdelete: false,
            pubtruncate: false,
            pubviaroot: true,
            table_count: 3,
            published_tables: vec!["public.orders".to_string()],
        };
        let line = publication_line(&pub_row);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("pub test_pub"), "{text}");
        assert!(text.contains("3 tables"), "{text}");
        assert!(text.contains("ins,upd"), "{text}");
        assert!(text.contains("(via_root)"), "{text}");
        assert!(text.contains("[public.orders]"), "{text}");
    }

    #[test]
    fn subscription_line_renders_sync_and_errors() {
        let sub_row = pg_lens_core::SubscriptionRow {
            subname: "billing_sub".to_string(),
            owner: "app".to_string(),
            enabled: true,
            slot_name: Some("billing_slot".to_string()),
            publications: vec!["billing_pub".to_string()],
            sync_commit: Some("off".to_string()),
            publisher_host: Some("10.0.0.1".to_string()),
            publisher_port: Some("5432".to_string()),
            publisher_dbname: Some("prod".to_string()),
            streaming_mode: Some("parallel".to_string()),
            binary_mode: Some(true),
            two_phase: Some(false),
            worker_pid: Some(1234),
            received_lsn: Some("0/1A2B3C".to_string()),
            last_msg_send_secs: Some(0.1),
            last_msg_receipt_secs: Some(0.1),
            latest_end_lsn: Some("0/1A2B3C".to_string()),
            latest_end_secs: Some(0.1),
            sync_tables: 2,
            ready_tables: 8,
            total_tables: 10,
            syncing_table_names: vec!["public.users (copy)".to_string()],
            apply_error_count: Some(5),
            sync_error_count: Some(0),
        };
        let line = subscription_line(&sub_row);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("sub billing_sub"), "{text}");
        assert!(text.contains("from 10.0.0.1/prod"), "{text}");
        assert!(text.contains("sync_commit: off"), "{text}");
        assert!(text.contains("worker: pid 1234"), "{text}");
        assert!(text.contains("tables: 8/10 ready"), "{text}");
        assert!(text.contains("(2 syncing)"), "{text}");
        assert!(text.contains("[public.users (copy)]"), "{text}");
        assert!(text.contains("[apply errors: 5]"), "{text}");
    }
}
