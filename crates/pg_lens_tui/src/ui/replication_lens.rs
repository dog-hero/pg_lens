//! Replication Lens (U1): the full replication picture — every WAL
//! sender/receiver and every slot, none of it clipped. The Macro Lens keeps
//! its own compact, capped summary (with a hint pointing here once it
//! clips); this lens is where a fleet primary with a dozen replicas and
//! slots finally has room to breathe.
//!
//! Row semantics mirror the Macro Lens panel exactly (same severity math,
//! shared via `ui/replication.rs`): senders/receiver render first, then
//! ALL slots as a scrollable (j/k) table, worst-severity-first then
//! retained-bytes descending — see [`crate::app::slot_severity_rank`].

use pg_lens_core::ReplicationInfo;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Paragraph, Row, Table},
};

use crate::app::App;
use crate::ui::{format, style};
use crate::ui::replication::{receiver_line, sender_line, slot_severity, wal_generation_line};

/// Fixed widths of every column except the flexible Slot one, in order:
/// severity, Type, Active, Retained, WAL Status, Safe Size.
const SEVERITY_WIDTH: u16 = 2;
const FIXED_WIDTHS: [u16; 5] = [9, 8, 10, 11, 10];
const HIGHLIGHT_WIDTH: u16 = 2;
const COLUMN_SPACING: u16 = 1;

/// Role summary lines: ALL senders on a primary (this view has room — no
/// `SENDERS_SHOWN` cap), the receiver on a standby, or a calm placeholder
/// while nothing has been collected yet / nothing is connected.
fn role_lines(repl: Option<&ReplicationInfo>) -> Vec<Line<'static>> {
    match repl {
        Some(ReplicationInfo::Primary { senders }) if senders.is_empty() => {
            vec![Line::from("  primary \u{b7} no replicas connected").dim()]
        }
        Some(ReplicationInfo::Primary { senders }) => senders.iter().map(sender_line).collect(),
        Some(ReplicationInfo::Standby { receiver: Some(r) }) => vec![receiver_line(r)],
        Some(ReplicationInfo::Standby { receiver: None }) => {
            vec![Line::from("  standby \u{b7} waiting for a WAL sender\u{2026}").dim()]
        }
        None => vec![Line::from("  collecting replication role\u{2026}").dim()],
    }
}

fn conflicts_line(conflicts: &pg_lens_core::DatabaseConflicts) -> Line<'static> {
    let mut spans = vec![
        Span::styled("total: ", style::label_style()),
        Span::styled(format::human_count(conflicts.confl_total), style::value_style()),
    ];
    if let Some(rate) = conflicts.conflicts_per_sec {
        if rate > 0.0 {
            spans.push(Span::styled(
                format!(" ({rate:.1}/s)"),
                Style::new().fg(Color::Red).bold(),
            ));
        } else {
            spans.push(Span::styled(format!(" ({rate:.1}/s)"), style::label_style()));
        }
    }
    spans.push(Span::styled(" \u{b7} lock: ", style::label_style()));
    spans.push(Span::styled(
        format::human_count(conflicts.confl_lock),
        style::value_style(),
    ));
    if let Some(r) = conflicts.lock_conflicts_per_sec
        && r > 0.0
    {
        spans.push(Span::styled(
            format!(" ({r:.1}/s)"),
            Style::new().fg(Color::Red),
        ));
    }
    spans.push(Span::styled(" \u{b7} snapshot: ", style::label_style()));
    spans.push(Span::styled(
        format::human_count(conflicts.confl_snapshot),
        style::value_style(),
    ));
    if let Some(r) = conflicts.snapshot_conflicts_per_sec
        && r > 0.0
    {
        spans.push(Span::styled(
            format!(" ({r:.1}/s)"),
            Style::new().fg(Color::Red),
        ));
    }
    spans.push(Span::styled(" \u{b7} deadlock: ", style::label_style()));
    spans.push(Span::styled(
        format::human_count(conflicts.confl_deadlock),
        style::value_style(),
    ));
    if let Some(r) = conflicts.deadlock_conflicts_per_sec
        && r > 0.0
    {
        spans.push(Span::styled(
            format!(" ({r:.1}/s)"),
            Style::new().fg(Color::Red).bold(),
        ));
    }
    spans.push(Span::styled(" \u{b7} pin: ", style::label_style()));
    spans.push(Span::styled(
        format::human_count(conflicts.confl_bufferpin),
        style::value_style(),
    ));
    spans.push(Span::styled(" \u{b7} tblspc: ", style::label_style()));
    spans.push(Span::styled(
        format::human_count(conflicts.confl_tablespace),
        style::value_style(),
    ));
    Line::from(spans)
}

pub fn draw(app: &mut App, frame: &mut Frame, area: Rect) {
    let lines = role_lines(app.snapshot.replication.as_ref());
    // This view has room: no artificial cap on senders, but the role panel
    // still yields most of the screen to the slots table below it.
    let role_height = (lines.len() as u16 + 2).min(area.height.saturating_sub(6).max(3));
    // v0.16: WAL generation is what replicas must keep up with, so it gets
    // its own compact section here — a single bordered line, present only
    // once the fast tick has collected `pg_stat_wal` at least once this
    // session (absent, not an empty box, on PG < 14 or a restricted role).
    let wal_height = u16::from(app.snapshot.wal.is_some()) * 3;
    let conflicts_height = u16::from(app.snapshot.conflicts.is_some()) * 3;
    let [wal_area, conflicts_area, role_area, table_area, footer_area] = Layout::vertical([
        Constraint::Length(wal_height),
        Constraint::Length(conflicts_height),
        Constraint::Length(role_height),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    if let Some(wal) = app.snapshot.wal.as_ref() {
        let panel =
            Paragraph::new(wal_generation_line(wal)).block(Block::bordered().title("WAL Generation"));
        frame.render_widget(panel, wal_area);
    }

    if let Some(conflicts) = app.snapshot.conflicts.as_ref() {
        let panel = Paragraph::new(conflicts_line(conflicts)).block(
            Block::bordered()
                .title("Standby Recovery Conflicts (pg_stat_database_conflicts)"),
        );
        frame.render_widget(panel, conflicts_area);
    }

    let role_panel = Paragraph::new(lines).block(Block::bordered().title("Role"));
    frame.render_widget(role_panel, role_area);

    draw_slots(app, frame, table_area);
    draw_footer(app, frame, footer_area);
}

fn draw_slots(app: &mut App, frame: &mut Frame, area: Rect) {
    let Some(slots) = app.snapshot.replication_slots.as_deref() else {
        let placeholder = Paragraph::new(Line::from(" collecting replication slots\u{2026}").dim())
            .block(Block::bordered().title("Slots"));
        frame.render_widget(placeholder, area);
        return;
    };
    if slots.is_empty() {
        let placeholder =
            Paragraph::new(Line::from(" no replication slots on this server").dim())
                .block(Block::bordered().title("Slots"));
        frame.render_widget(placeholder, area);
        return;
    }

    let header = Row::new(["!", "Slot", "Type", "Active", "Retained", "WAL Status", "Safe Size"])
        .style(Style::new().bold());

    let slot_width = slot_column_width(area.width);

    let rows = app
        .replication_row_order
        .iter()
        .filter_map(|&i| slots.get(i))
        .map(|slot| {
            let sev = slot_severity(slot);
            let active = if slot.active { "active" } else { "inactive" };
            let retained = slot
                .retained_wal_bytes
                .map_or_else(|| "\u{2014}".to_string(), format::human_bytes);
            let safe = slot
                .safe_wal_size
                .map_or_else(|| "\u{2014}".to_string(), format::human_bytes);
            Row::new([
                sev.marker().to_string(),
                format::truncate_with_ellipsis(&slot.slot_name, slot_width),
                slot.slot_type.clone(),
                active.to_string(),
                retained,
                slot.wal_status.clone().unwrap_or_else(|| "\u{2014}".to_string()),
                safe,
            ])
            .style(Style::new().fg(sev.color()))
        });

    let widths = [
        Constraint::Length(SEVERITY_WIDTH),
        Constraint::Min(8),
        Constraint::Length(FIXED_WIDTHS[0]),
        Constraint::Length(FIXED_WIDTHS[1]),
        Constraint::Length(FIXED_WIDTHS[2]),
        Constraint::Length(FIXED_WIDTHS[3]),
        Constraint::Length(FIXED_WIDTHS[4]),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::bordered().title("Slots"))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("\u{25b6} ");

    frame.render_stateful_widget(table, area, &mut app.replication_table_state);
}

/// How many characters the flexible Slot column can hold at this terminal
/// width (same arithmetic as the Schema/Index Lens's flexible columns).
fn slot_column_width(area_width: u16) -> usize {
    let fixed: u16 = FIXED_WIDTHS.iter().sum::<u16>() + SEVERITY_WIDTH;
    let overhead = 2 /* block borders */ + HIGHLIGHT_WIDTH + fixed + 6 * COLUMN_SPACING;
    usize::from(area_width.saturating_sub(overhead))
}

fn draw_footer(app: &App, frame: &mut Frame, area: Rect) {
    let n = app
        .snapshot
        .replication_slots
        .as_deref()
        .map_or(0, <[_]>::len);
    let line = Line::from(format!(
        " {n} slot{} \u{b7} worst severity first",
        if n == 1 { "" } else { "s" }
    ))
    .dim();
    frame.render_widget(Paragraph::new(line), area);
}
