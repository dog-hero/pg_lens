//! Macro Lens: server-wide vitals dashboard.
//!
//! The sparklines render from [`DbSnapshot::history`] — the ring owned and
//! grown incrementally by the poller. Per frame we only copy the (≤120)
//! samples into a display buffer; the series itself is never rebuilt here.
//!
//! [`DbSnapshot::history`]: pg_lens_core::DbSnapshot

use pg_lens_core::{ReplicationInfo, SchemaSnapshot, WalReceiverRow, WalSenderRow};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Gauge, Paragraph, Sparkline},
};

use crate::app::App;
use crate::ui::{format, style, vacuum};

/// Bordered block with the panel title in the shared accent style.
fn titled_block(title: &'static str) -> Block<'static> {
    Block::bordered().title(Line::from(title).style(style::accent_style()))
}

/// Lag severity for replication rows. Thresholds (either dimension trips the
/// tier): yellow > 10 MB or > 10 s, red > 100 MB or > 60 s.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lag {
    Ok,
    Warn,
    Bad,
}

fn lag_severity(bytes: Option<i64>, secs: Option<f64>) -> Lag {
    // 0 bytes outstanding = definitively caught up. The seconds measure on the
    // standby side is `now() - pg_last_xact_replay_timestamp()`, which grows
    // unboundedly on an idle primary even when the standby is perfectly in
    // sync — so it must never raise an alarm on its own.
    if bytes == Some(0) {
        return Lag::Ok;
    }
    let b = bytes.unwrap_or(0);
    let s = secs.unwrap_or(0.0);
    if b > 100 * 1024 * 1024 || s > 60.0 {
        Lag::Bad
    } else if b > 10 * 1024 * 1024 || s > 10.0 {
        Lag::Warn
    } else {
        Lag::Ok
    }
}

impl Lag {
    /// 1-char textual marker (like the Micro Lens B/W markers) so severity is
    /// provable in VT captures without relying on color.
    fn marker(self) -> &'static str {
        match self {
            Lag::Ok => "  ",
            Lag::Warn => "! ",
            Lag::Bad => "!!",
        }
    }
    fn color(self) -> Color {
        match self {
            Lag::Ok => Color::Green,
            Lag::Warn => Color::Yellow,
            Lag::Bad => Color::Red,
        }
    }
}

/// Formats the two lag measures as `12 MB · 1.2s`, `—` when both absent.
fn lag_text(bytes: Option<i64>, secs: Option<f64>) -> String {
    match (bytes, secs) {
        (Some(b), Some(s)) => format!("{} · {}", format::human_bytes(b), format::human_duration(s)),
        (Some(b), None) => format::human_bytes(b),
        (None, Some(s)) => format::human_duration(s),
        (None, None) => "—".to_string(),
    }
}

fn sender_line(s: &WalSenderRow) -> Line<'static> {
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

fn receiver_line(r: &WalReceiverRow) -> Line<'static> {
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

/// The replication panel's lines, or `None` when there is nothing worth a
/// panel (a primary with no replicas). A standby always shows a line.
fn replication_lines(repl: Option<&ReplicationInfo>) -> Option<Vec<Line<'static>>> {
    match repl? {
        ReplicationInfo::Primary { senders } if senders.is_empty() => None,
        ReplicationInfo::Primary { senders } => Some(senders.iter().map(sender_line).collect()),
        ReplicationInfo::Standby { receiver: Some(r) } => Some(vec![receiver_line(r)]),
        ReplicationInfo::Standby { receiver: None } => Some(vec![Line::from(Span::styled(
            "standby · waiting for a WAL sender…",
            style::label_style(),
        ))]),
    }
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

pub fn draw(app: &App, frame: &mut Frame, area: Rect) {
    let vitals = &app.snapshot.vitals;
    let history = &app.snapshot.history;

    let vacuum_banner = vacuum_banner_line(app.snapshot.schema.as_deref());
    let vacuum_banner_height = u16::from(vacuum_banner.is_some());

    let repl_lines = replication_lines(app.snapshot.replication.as_ref());
    // Reserve a bordered panel only when there is replication to show;
    // otherwise the vitals panel keeps the whole bottom area (layout
    // unchanged for non-replicated servers).
    let repl_height = repl_lines
        .as_ref()
        .map(|l| (l.len() as u16 + 2).min(8))
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

    if let Some(lines) = repl_lines {
        let panel = Paragraph::new(lines).block(titled_block("Replication"));
        frame.render_widget(panel, repl_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_tiers_by_bytes_and_secs() {
        assert!(matches!(lag_severity(Some(0), Some(0.0)), Lag::Ok));
        assert!(matches!(lag_severity(Some(20 * 1024 * 1024), None), Lag::Warn));
        assert!(matches!(lag_severity(None, Some(12.0)), Lag::Warn));
        assert!(matches!(
            lag_severity(Some(200 * 1024 * 1024), None),
            Lag::Bad
        ));
        assert!(matches!(lag_severity(None, Some(90.0)), Lag::Bad));
    }

    #[test]
    fn zero_bytes_is_caught_up_regardless_of_the_stale_time_measure() {
        // Idle-primary case: 0 bytes outstanding but the last-replay age is
        // minutes old — must stay OK, not flag red.
        assert!(matches!(lag_severity(Some(0), Some(240.0)), Lag::Ok));
    }

    #[test]
    fn primary_without_replicas_hides_the_panel() {
        let repl = ReplicationInfo::Primary { senders: vec![] };
        assert!(replication_lines(Some(&repl)).is_none());
        assert!(replication_lines(None).is_none());
    }

    #[test]
    fn standby_always_shows_a_line() {
        let repl = ReplicationInfo::Standby { receiver: None };
        assert_eq!(replication_lines(Some(&repl)).unwrap().len(), 1);
    }

    #[test]
    fn lag_text_handles_missing_measures() {
        assert_eq!(lag_text(None, None), "—");
        assert!(lag_text(Some(1024 * 1024), Some(1.5)).contains('·'));
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
}
