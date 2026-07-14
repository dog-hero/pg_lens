//! Macro Lens: server-wide vitals dashboard.
//!
//! The sparklines render from [`DbSnapshot::history`] — the ring owned and
//! grown incrementally by the poller. Per frame we only copy the (≤120)
//! samples into a display buffer; the series itself is never rebuilt here.
//!
//! [`DbSnapshot::history`]: pg_lens_core::DbSnapshot

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Gauge, Paragraph, Sparkline},
};

use crate::app::App;
use crate::ui::{format, style};

/// Bordered block with the panel title in the shared accent style.
fn titled_block(title: &'static str) -> Block<'static> {
    Block::bordered().title(Line::from(title).style(style::accent_style()))
}

pub fn draw(app: &App, frame: &mut Frame, area: Rect) {
    let vitals = &app.snapshot.vitals;
    let history = &app.snapshot.history;

    let [gauge_area, tps_area, active_area, vitals_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(5),
        Constraint::Length(5),
        Constraint::Min(0),
    ])
    .areas(area);
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
}
