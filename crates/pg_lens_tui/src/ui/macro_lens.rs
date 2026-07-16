//! Macro Lens: server-wide vitals dashboard.
//!
//! The sparklines render from [`DbSnapshot::history`] — the ring owned and
//! grown incrementally by the poller. Per frame we only copy the (≤120)
//! samples into a display buffer; the series itself is never rebuilt here.
//!
//! [`DbSnapshot::history`]: pg_lens_core::DbSnapshot

use pg_lens_core::{CheckpointerStats, ReplicationInfo, ReplicationSlotRow, SchemaSnapshot};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Gauge, Paragraph, Sparkline},
};

use crate::app::App;
use crate::ui::replication::{Severity as Lag, receiver_line, sender_line, slot_line, slot_severity};
use crate::ui::{format, style, vacuum};

/// Bordered block with the panel title in the shared accent style.
fn titled_block(title: &'static str) -> Block<'static> {
    Block::bordered().title(Line::from(title).style(style::accent_style()))
}

/// Cap on WAL-sender rows in the panel: a CDC/fleet primary can have 10+
/// active senders, and before this cap they pushed every slot row out of the
/// height-limited panel — the exact rows the F2.5 feature exists to show.
const SENDERS_SHOWN: usize = 4;
/// Cap on slot rows (worst-severity first, so what clips is the calm tail).
const SLOTS_SHOWN: usize = 6;

/// The replication panel's lines, or `None` when there is nothing worth a
/// panel (a primary with no replicas and no slots). A standby's
/// sender/receiver section always shows a line; slot rows (F2.5) follow,
/// ranked worst-first, and contribute nothing when the slots list is empty
/// (no extra "no slots" section — silence is the calm state). Both sections
/// clip with an explicit dim `… +N more` line instead of silently dropping
/// rows.
fn replication_lines(
    repl: Option<&ReplicationInfo>,
    slots: Option<&[ReplicationSlotRow]>,
) -> Option<Vec<Line<'static>>> {
    let more_line = |n: usize, what: &str| {
        Line::from(Span::styled(format!("   \u{2026} +{n} more {what}"), style::label_style()))
    };
    let mut clipped = false;
    let mut lines: Vec<Line<'static>> = match repl {
        Some(ReplicationInfo::Primary { senders }) => {
            let mut lines: Vec<Line<'static>> =
                senders.iter().take(SENDERS_SHOWN).map(sender_line).collect();
            if senders.len() > SENDERS_SHOWN {
                lines.push(more_line(senders.len() - SENDERS_SHOWN, "replicas"));
                clipped = true;
            }
            lines
        }
        Some(ReplicationInfo::Standby { receiver: Some(r) }) => vec![receiver_line(r)],
        Some(ReplicationInfo::Standby { receiver: None }) => vec![Line::from(Span::styled(
            "standby · waiting for a WAL sender…",
            style::label_style(),
        ))],
        None => Vec::new(),
    };
    if let Some(slots) = slots
        && !slots.is_empty()
    {
        // Worst first (Bad > Warn > Ok), then by retained bytes descending
        // (the SQL's order), so clipping drops the healthiest slots.
        let mut ranked: Vec<&ReplicationSlotRow> = slots.iter().collect();
        ranked.sort_by_key(|s| match slot_severity(s) {
            Lag::Bad => 0u8,
            Lag::Warn => 1,
            Lag::Ok => 2,
        });
        lines.extend(ranked.iter().take(SLOTS_SHOWN).map(|s| slot_line(s)));
        if ranked.len() > SLOTS_SHOWN {
            lines.push(more_line(ranked.len() - SLOTS_SHOWN, "slots"));
            clipped = true;
        }
    }
    // U1: this panel stays capped/compact by design — the full,
    // never-clipped picture lives one Tab away.
    if clipped {
        lines.push(Line::from(Span::styled(
            "   Tab \u{2192} Replication for all",
            style::label_style(),
        )));
    }
    if lines.is_empty() { None } else { Some(lines) }
}

/// F2's Macro Lens warning: one line, loud only when the cluster's XID
/// wraparound distance has crossed yellow/red — `None` (no banner) while
/// healthy or before the first slow collection, mirroring the replication
/// panel's "nothing worth showing → nothing rendered" rule.
fn vacuum_banner_line(schema: Option<&SchemaSnapshot>) -> Option<Line<'static>> {
    let age = schema?.vacuum_cluster_age.as_ref()?;
    let sev = vacuum::age_severity(age.max_age_xids);
    if sev == vacuum::Severity::Ok {
        return None;
    }
    Some(Line::from(vec![
        Span::styled(format!("{} ", sev.marker()), Style::new().fg(sev.color())),
        Span::styled(
            format!(
                "XID wraparound: {} xids old (worst db: {}) \u{2014} VACUUM attention needed",
                format::human_count(age.max_age_xids),
                age.worst_database,
            ),
            Style::new().fg(sev.color()).bold(),
        ),
    ]))
}

/// F4's checkpoint-pressure severity: a high requested share (over the
/// poller session window, not per-tick — checkpoints are rare) means
/// `max_wal_size` is likely too small. Yellow only — this is a tuning
/// signal, not an incident, so it never escalates to red. `None` (no
/// checkpoint yet this session) renders calm.
fn checkpoint_pressure_severity(ratio: Option<f64>) -> Lag {
    match ratio {
        Some(r) if r > 0.5 => Lag::Warn,
        _ => Lag::Ok,
    }
}

/// `12.3/s`, or `--` while no delta window exists yet (first poll of a
/// session — same rule as TPS).
fn rate_per_sec(v: Option<f64>) -> String {
    match v {
        Some(x) => format!("{x:.1}/s"),
        None => "--".to_string(),
    }
}

/// Backend-issued buffer writes: absent for a real reason on PG 17+ (moved
/// to `pg_stat_io`, out of this cheap single-row query's scope) — the empty
/// state says so instead of a bare dash.
fn backend_rate_text(cp: &CheckpointerStats) -> String {
    if cp.buffers_backend.is_none() {
        "n/a (17+)".to_string()
    } else {
        rate_per_sec(cp.buffers_backend_per_sec)
    }
}

/// The Checkpoints/writer panel's lines (F4). Absent counters (first poll of
/// a session) render as `--`, never a fault — mirrors the vitals panel's
/// pre-first-snapshot treatment of TPS.
fn checkpointer_lines(cp: Option<&CheckpointerStats>) -> Vec<Line<'static>> {
    let Some(cp) = cp else {
        return vec![Line::from(Span::styled(
            "collecting checkpointer stats\u{2026}",
            style::label_style(),
        ))];
    };
    let sev = checkpoint_pressure_severity(cp.requested_ratio_session);
    let per_min = match (cp.checkpoints_per_min_timed, cp.checkpoints_per_min_req) {
        (Some(t), Some(r)) => format!("{t:.2} timed / {r:.2} req /min"),
        _ => "-- timed / -- req /min".to_string(),
    };
    let pressure = match cp.requested_ratio_session {
        Some(r) => format!("{:.0}% requested (session)", r * 100.0),
        None => "-- (no checkpoint yet this session)".to_string(),
    };
    let avg_write = cp
        .avg_checkpoint_write_ms
        .map(format::human_ms)
        .unwrap_or_else(|| "--".to_string());
    let avg_sync = cp
        .avg_checkpoint_sync_ms
        .map(format::human_ms)
        .unwrap_or_else(|| "--".to_string());

    vec![
        Line::from(vec![
            Span::styled(format!("{} ", sev.marker()), Style::new().fg(sev.color())),
            Span::styled("checkpoints: ", style::label_style()),
            Span::styled(per_min, Style::new().fg(sev.color())),
        ]),
        Line::from(vec![
            Span::styled("  pressure   : ", style::label_style()),
            Span::styled(pressure, Style::new().fg(sev.color())),
        ]),
        style::kv(
            "  buffers/s  : ",
            format!(
                "chkpt {} \u{b7} bgwriter {} \u{b7} backend {}",
                rate_per_sec(cp.buffers_checkpoint_per_sec),
                rate_per_sec(cp.buffers_clean_per_sec),
                backend_rate_text(cp),
            ),
        ),
        style::kv(
            "  avg write/sync: ",
            format!("{avg_write} / {avg_sync}"),
        ),
    ]
}

pub fn draw(app: &App, frame: &mut Frame, area: Rect) {
    let vitals = &app.snapshot.vitals;
    let history = &app.snapshot.history;

    let vacuum_banner = vacuum_banner_line(app.snapshot.schema.as_deref());
    let vacuum_banner_height = u16::from(vacuum_banner.is_some());

    let repl_lines = replication_lines(
        app.snapshot.replication.as_ref(),
        app.snapshot.replication_slots.as_deref(),
    );
    // Reserve a bordered panel only when there is replication to show;
    // otherwise the vitals panel keeps the whole bottom area (layout
    // unchanged for non-replicated servers).
    // Content is already capped upstream (SENDERS_SHOWN/SLOTS_SHOWN + their
    // "+N more" lines + the U1 "Tab → Replication" hint when clipped = at
    // most 13 rows), so the +2 border fits in 15.
    let repl_height = repl_lines
        .as_ref()
        .map(|l| (l.len() as u16 + 2).min(15))
        .unwrap_or(0);

    let [banner_area, gauge_area, tps_area, active_area, bottom_area] = Layout::vertical([
        Constraint::Length(vacuum_banner_height),
        Constraint::Length(3),
        Constraint::Length(5),
        Constraint::Length(5),
        Constraint::Min(0),
    ])
    .areas(area);
    if let Some(line) = vacuum_banner {
        frame.render_widget(Paragraph::new(line), banner_area);
    }
    let [vitals_area, repl_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(repl_height)]).areas(bottom_area);
    // Vitals keeps the left 60%; the Checkpoints/writer panel (F4) takes the
    // right 40% — same split ratio as the two top gauges.
    let [vitals_area, checkpoint_area] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
            .areas(vitals_area);
    let [conn_gauge_area, cache_gauge_area] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .areas(gauge_area);

    let ratio = if vitals.max_connections > 0 {
        f64::from(vitals.connections_total) / f64::from(vitals.max_connections)
    } else {
        0.0
    };
    let gauge = Gauge::default()
        .block(titled_block("Connections"))
        .gauge_style(Style::new().fg(Color::Cyan))
        .ratio(ratio.clamp(0.0, 1.0))
        .label(format!(
            "{}/{}",
            vitals.connections_total, vitals.max_connections
        ));
    frame.render_widget(gauge, conn_gauge_area);

    let cache_gauge = Gauge::default()
        .block(titled_block("Cache hit"))
        .gauge_style(Style::new().fg(Color::Magenta))
        .ratio(vitals.cache_hit_ratio.clamp(0.0, 1.0))
        .label(format!("{:.1}%", vitals.cache_hit_ratio * 100.0));
    frame.render_widget(cache_gauge, cache_gauge_area);

    // Display buffers copied from the poller-owned ring (oldest → newest).
    let tps_series: Vec<u64> = history.iter().map(|p| p.tps.round() as u64).collect();
    let active_series: Vec<u64> = history
        .iter()
        .map(|p| u64::from(p.active_sessions))
        .collect();

    // Sparkline titles: name in accent, live value in the value style.
    let tps_title = Line::from(vec![
        ratatui::text::Span::styled("TPS ", style::accent_style()),
        ratatui::text::Span::styled(format!("(now: {:.0})", vitals.tps), style::value_style()),
    ]);
    let tps_sparkline = Sparkline::default()
        .block(Block::bordered().title(tps_title))
        .style(Style::new().fg(Color::Green))
        .data(&tps_series);
    frame.render_widget(tps_sparkline, tps_area);

    let active_title = Line::from(vec![
        ratatui::text::Span::styled("Active sessions ", style::accent_style()),
        ratatui::text::Span::styled(format!("(now: {})", vitals.active), style::value_style()),
    ]);
    let active_sparkline = Sparkline::default()
        .block(Block::bordered().title(active_title))
        .style(Style::new().fg(Color::Yellow))
        .data(&active_series);
    frame.render_widget(active_sparkline, active_area);

    // Key/value list: dim labels, bold values (style::kv) — the eye scans
    // straight down the value column.
    let lines = vec![
        style::kv("Active          : ", vitals.active.to_string()),
        style::kv("Idle            : ", vitals.idle.to_string()),
        style::kv("Idle in tx      : ", vitals.idle_in_transaction.to_string()),
        style::kv("Waiting         : ", vitals.waiting.to_string()),
        style::kv("Deadlocks       : ", vitals.deadlocks.to_string()),
        style::kv(
            "Temp files      : ",
            format!(
                "{} ({})",
                vitals.temp_files,
                format::human_bytes(vitals.temp_bytes)
            ),
        ),
    ];
    let paragraph = Paragraph::new(lines).block(titled_block("Vitals"));
    frame.render_widget(paragraph, vitals_area);

    let checkpoint_lines = checkpointer_lines(app.snapshot.checkpointer.as_ref());
    let checkpoint_panel = Paragraph::new(checkpoint_lines).block(titled_block("Checkpoints / writer"));
    frame.render_widget(checkpoint_panel, checkpoint_area);

    if let Some(lines) = repl_lines {
        let panel = Paragraph::new(lines).block(titled_block("Replication"));
        frame.render_widget(panel, repl_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pg_lens_core::WalSenderRow;

    // Lag/slot severity math itself is tested in `ui/replication.rs`, the
    // module that now owns it — these tests cover the compact panel's own
    // behavior (caps, ranking, the "+N more" / "Tab → Replication" hints).

    #[test]
    fn primary_without_replicas_hides_the_panel() {
        let repl = ReplicationInfo::Primary { senders: vec![] };
        assert!(replication_lines(Some(&repl), None).is_none());
        assert!(replication_lines(None, None).is_none());
        assert!(replication_lines(None, Some(&[])).is_none());
    }

    #[test]
    fn standby_always_shows_a_line() {
        let repl = ReplicationInfo::Standby { receiver: None };
        assert_eq!(replication_lines(Some(&repl), None).unwrap().len(), 1);
    }

    // --- F2.5: replication slots -----------------------------------------

    fn slot(active: bool, wal_status: Option<&str>, retained_wal_bytes: Option<i64>) -> ReplicationSlotRow {
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
    fn empty_slots_list_renders_no_extra_rows() {
        let repl = ReplicationInfo::Primary { senders: vec![] };
        assert!(replication_lines(Some(&repl), Some(&[])).is_none());
    }

    /// Regression (field report, v0.7.0): a CDC primary with many active
    /// senders pushed EVERY slot row out of the height-capped panel — the
    /// exact rows F2.5 exists to show. Senders now clip at SENDERS_SHOWN and
    /// slots always get their section, worst first.
    #[test]
    fn many_senders_do_not_push_slots_out_of_the_panel() {
        let sender = |name: &str| WalSenderRow {
            application_name: name.to_string(),
            client: "10.0.0.1".to_string(),
            state: "streaming".to_string(),
            sync_state: "async".to_string(),
            replay_lag_bytes: Some(0),
            replay_lag_secs: Some(0.0),
        };
        let senders: Vec<WalSenderRow> = (0..10).map(|i| sender(&format!("cdc_{i}"))).collect();
        let repl = ReplicationInfo::Primary { senders };
        let slots: Vec<ReplicationSlotRow> = (0..10)
            .map(|i| {
                let mut s = slot(false, Some("extended"), Some(1_000_000 * (i + 1)));
                s.slot_name = format!("slot_{i}");
                s
            })
            .collect();
        let lines = replication_lines(Some(&repl), Some(&slots)).expect("panel renders");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        // Senders clipped with an explicit indicator…
        assert!(text.contains("+6 more replicas"), "{text}");
        // …and slots are PRESENT (the bug was zero slot rows here).
        assert!(text.contains("slot_"), "slot rows must render: {text}");
        assert!(text.contains("+4 more slots"), "{text}");
        // U1: clipping shows the way to the full picture.
        assert!(text.contains("Tab \u{2192} Replication for all"), "{text}");
        // Total stays within the panel's height budget (13 content lines).
        assert!(lines.len() <= 13, "got {} lines", lines.len());
    }

    /// U1: an unclipped panel (nothing capped) carries no "Tab → Replication"
    /// hint — it would be pointless noise when the panel already shows
    /// everything.
    #[test]
    fn unclipped_panel_has_no_tab_hint() {
        let repl = ReplicationInfo::Primary { senders: vec![] };
        let slots = vec![slot(false, Some("extended"), Some(2_600_000_000))];
        let lines = replication_lines(Some(&repl), Some(&slots)).expect("panel renders");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(!text.contains("Tab \u{2192}"), "{text}");
    }

    /// Worst slots surface first, so clipping drops the calm tail.
    #[test]
    fn slots_rank_worst_severity_first() {
        let mut lost = slot(false, Some("lost"), Some(10));
        lost.slot_name = "lost_slot".to_string();
        let mut calm = slot(true, Some("reserved"), Some(999_999_999));
        calm.slot_name = "calm_slot".to_string();
        let mut warn = slot(false, Some("extended"), Some(5_000));
        warn.slot_name = "warn_slot".to_string();
        let slots = vec![calm, warn, lost];
        let lines = replication_lines(None, Some(&slots)).expect("panel renders");
        let first: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(first.contains("lost_slot"), "red first: {first}");
    }

    #[test]
    fn slots_render_under_the_senders_with_a_marker() {
        let repl = ReplicationInfo::Primary { senders: vec![] };
        let slots = vec![slot(false, Some("extended"), Some(2_600_000_000))];
        let lines = replication_lines(Some(&repl), Some(&slots)).expect("slot rows render");
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("probe_slot"));
        assert!(text.contains('!'), "warn marker must be visible: {text}");
    }

    // --- F2: vacuum wraparound banner -----------------------------------

    fn schema_with_age(max_age_xids: i64) -> SchemaSnapshot {
        let mut schema = SchemaSnapshot::mock();
        schema.vacuum_cluster_age = Some(pg_lens_core::VacuumClusterAge {
            max_age_xids,
            worst_database: "shop".to_string(),
        });
        schema
    }

    #[test]
    fn vacuum_banner_is_absent_below_the_yellow_threshold() {
        assert!(vacuum_banner_line(None).is_none(), "no schema yet");
        let mut schema = SchemaSnapshot::mock();
        schema.vacuum_cluster_age = None;
        assert!(
            vacuum_banner_line(Some(&schema)).is_none(),
            "no collection yet"
        );
        let schema = schema_with_age(150_000_000);
        assert!(vacuum_banner_line(Some(&schema)).is_none(), "healthy: no banner");
    }

    #[test]
    fn vacuum_banner_appears_past_yellow_and_red() {
        let yellow = schema_with_age(250_000_000);
        let line = vacuum_banner_line(Some(&yellow)).expect("yellow crosses the threshold");
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("XID wraparound"));
        assert!(text.contains("shop"));

        let red = schema_with_age(600_000_000);
        let line = vacuum_banner_line(Some(&red)).expect("red crosses the threshold");
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("XID wraparound"));
    }

    // --- F4: checkpointer / bgwriter panel -------------------------------

    fn checkpointer(requested_ratio_session: Option<f64>) -> CheckpointerStats {
        CheckpointerStats {
            checkpoints_timed: 100,
            checkpoints_req: 10,
            checkpoint_write_time_ms: 50_000.0,
            checkpoint_sync_time_ms: 4_000.0,
            buffers_checkpoint: 900_000,
            buffers_clean: 30_000,
            maxwritten_clean: 5,
            buffers_backend: Some(20_000),
            buffers_alloc: 1_000_000,
            checkpoints_per_min_timed: Some(0.5),
            checkpoints_per_min_req: Some(0.02),
            buffers_checkpoint_per_sec: Some(12.3),
            buffers_clean_per_sec: Some(4.5),
            buffers_backend_per_sec: Some(1.1),
            avg_checkpoint_write_ms: Some(4_200.0),
            avg_checkpoint_sync_ms: Some(310.0),
            requested_ratio_session,
        }
    }

    #[test]
    fn checkpoint_pressure_is_calm_below_and_at_the_fifty_percent_line() {
        assert!(matches!(
            checkpoint_pressure_severity(None),
            Lag::Ok
        ));
        assert!(matches!(
            checkpoint_pressure_severity(Some(0.0)),
            Lag::Ok
        ));
        assert!(matches!(
            checkpoint_pressure_severity(Some(0.5)),
            Lag::Ok
        ));
    }

    #[test]
    fn checkpoint_pressure_turns_yellow_once_requested_outweighs_timed() {
        assert!(matches!(
            checkpoint_pressure_severity(Some(0.51)),
            Lag::Warn
        ));
        assert!(matches!(
            checkpoint_pressure_severity(Some(1.0)),
            Lag::Warn
        ));
    }

    #[test]
    fn checkpointer_panel_shows_collecting_state_before_first_poll() {
        let lines = checkpointer_lines(None);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("collecting checkpointer stats"));
    }

    #[test]
    fn checkpointer_panel_renders_rates_and_a_calm_pressure_line() {
        let cp = checkpointer(Some(0.1));
        let lines = checkpointer_lines(Some(&cp));
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("0.50 timed"));
        assert!(text.contains("0.02 req"));
        assert!(text.contains("10% requested"));
        assert!(text.contains("chkpt 12.3/s"));
        assert!(text.contains("bgwriter 4.5/s"));
        assert!(text.contains("backend 1.1/s"));
        assert!(!text.contains('!'), "calm ratio carries no warn marker");
    }

    #[test]
    fn checkpointer_panel_flags_pressure_and_absent_first_tick_rates() {
        let cp = checkpointer(Some(0.9));
        let lines = checkpointer_lines(Some(&cp));
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains('!'), "warn marker visible under pressure");
        assert!(text.contains("90% requested"));

        // First-tick style stats: rates absent, ratio absent — "--" not 0.
        let mut cp0 = checkpointer(None);
        cp0.checkpoints_per_min_timed = None;
        cp0.checkpoints_per_min_req = None;
        cp0.buffers_checkpoint_per_sec = None;
        cp0.buffers_clean_per_sec = None;
        cp0.buffers_backend_per_sec = None;
        cp0.avg_checkpoint_write_ms = None;
        cp0.avg_checkpoint_sync_ms = None;
        let lines = checkpointer_lines(Some(&cp0));
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("-- timed / -- req"));
        assert!(text.contains("no checkpoint yet this session"));
    }

    #[test]
    fn checkpointer_panel_explains_the_pg17_backend_buffers_split() {
        let mut cp = checkpointer(Some(0.1));
        cp.buffers_backend = None;
        cp.buffers_backend_per_sec = None;
        let lines = checkpointer_lines(Some(&cp));
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("backend n/a (17+)"), "{text}");
    }

}
