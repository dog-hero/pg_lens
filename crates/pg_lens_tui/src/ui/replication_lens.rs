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
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Row, Table},
};

use crate::app::{App, Tab};
use crate::ui::{format, style};
use crate::ui::replication::{
    publication_line, receiver_line, sender_line, slot_severity, subscription_line,
    wal_generation_line,
};

/// Fixed widths of columns on wide terminals (>= 135 cols) - all 12 columns:
/// Type, Plugin, DB, Active, Retained, Lag, WAL Status, Safe Size, xmin Age, Invalidated.
const SEVERITY_WIDTH: u16 = 2;
const FIXED_WIDTHS_WIDE: [u16; 10] = [8, 8, 8, 8, 9, 9, 10, 9, 10, 11];

/// Fixed widths of columns on medium terminals (>= 100 cols, e.g. standard 120 cols) - 10 columns:
/// Type, Plugin, DB, Active, Retained, Lag, WAL Status, Safe Size.
const FIXED_WIDTHS_MED: [u16; 8] = [8, 8, 8, 8, 9, 9, 10, 9];

/// Fixed widths of columns on narrow terminals (< 100 cols, e.g. 80 cols) - 7 columns:
/// Type, Active, Retained, Lag, WAL Status.
const FIXED_WIDTHS_NARROW: [u16; 5] = [8, 8, 9, 9, 10];

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
    let role_height = (lines.len() as u16 + 2).min(area.height.saturating_sub(6).max(3));
    let wal_height = u16::from(app.snapshot.wal.is_some()) * 3;
    let conflicts_height = u16::from(app.snapshot.conflicts.is_some()) * 3;

    // Separate panels for Publications and Subscriptions
    let pub_count = app.snapshot.publications.as_deref().map_or(0, <[_]>::len);
    let sub_count = app.snapshot.subscriptions.as_deref().map_or(0, <[_]>::len);
    let pub_height = if app.snapshot.publications.is_some() {
        if pub_count == 0 { 3 } else { (pub_count as u16 + 2).clamp(3, 5) }
    } else {
        0
    };
    let sub_height = if app.snapshot.subscriptions.is_some() {
        if sub_count == 0 { 3 } else { (sub_count as u16 + 2).clamp(3, 5) }
    } else {
        0
    };

    let [wal_area, conflicts_area, role_area, pub_area, sub_area, table_area, footer_area] = Layout::vertical([
        Constraint::Length(wal_height),
        Constraint::Length(conflicts_height),
        Constraint::Length(role_height),
        Constraint::Length(pub_height),
        Constraint::Length(sub_height),
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

    let role_panel =
        Paragraph::new(lines).block(Block::bordered().title("Physical Replication (Role)"));
    frame.render_widget(role_panel, role_area);

    if pub_height > 0 {
        draw_publications(app, frame, pub_area);
    }
    if sub_height > 0 {
        draw_subscriptions(app, frame, sub_area);
    }

    draw_slots(app, frame, table_area);
    draw_footer(app, frame, footer_area);

    if app.detail_open && app.active_tab == Tab::ReplicationLens {
        draw_slot_detail(app, frame, area);
    }
}

fn draw_publications(app: &App, frame: &mut Frame, area: Rect) {
    if area.height == 0 {
        return;
    }
    let mut lines = Vec::new();
    if let Some(pubs) = app.snapshot.publications.as_deref() {
        for p in pubs {
            lines.push(publication_line(p));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from("  no publications configured in this database").dim());
    }
    let panel = Paragraph::new(lines).block(Block::bordered().title("Publications (pg_publication)"));
    frame.render_widget(panel, area);
}

fn draw_subscriptions(app: &App, frame: &mut Frame, area: Rect) {
    if area.height == 0 {
        return;
    }
    let mut lines = Vec::new();
    if let Some(subs) = app.snapshot.subscriptions.as_deref() {
        for s in subs {
            lines.push(subscription_line(s));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from("  no subscriptions configured in this database").dim());
    }
    let panel = Paragraph::new(lines).block(Block::bordered().title("Subscriptions (pg_subscription)"));
    frame.render_widget(panel, area);
}

fn slot_table_row(
    slot: &pg_lens_core::ReplicationSlotRow,
    slot_width: usize,
    width_tier: u8,
) -> Row<'static> {
    let sev = slot_severity(slot);
    let active = if slot.active { "active" } else { "inactive" };
    let retained = slot
        .retained_wal_bytes
        .map_or_else(|| "\u{2014}".to_string(), format::human_bytes);
    let lag = slot
        .consumer_lag_bytes
        .map_or_else(|| "\u{2014}".to_string(), format::human_bytes);
    let safe = slot
        .safe_wal_size
        .map_or_else(|| "\u{2014}".to_string(), format::human_bytes);

    if width_tier == 0 {
        let xmin = slot
            .xmin_age
            .or(slot.catalog_xmin_age)
            .map_or_else(|| "\u{2014}".to_string(), format::human_count);
        let inval = slot.invalidated.as_deref().unwrap_or("\u{2014}");
        Row::new([
            sev.marker().to_string(),
            format::truncate_with_ellipsis(&slot.slot_name, slot_width),
            slot.slot_type.clone(),
            slot.plugin.clone().unwrap_or_else(|| "\u{2014}".to_string()),
            slot.database.clone().unwrap_or_else(|| "\u{2014}".to_string()),
            active.to_string(),
            retained,
            lag,
            slot.wal_status.clone().unwrap_or_else(|| "\u{2014}".to_string()),
            safe,
            xmin,
            inval.to_string(),
        ])
        .style(Style::new().fg(sev.color()))
    } else if width_tier == 1 {
        Row::new([
            sev.marker().to_string(),
            format::truncate_with_ellipsis(&slot.slot_name, slot_width),
            slot.slot_type.clone(),
            slot.plugin.clone().unwrap_or_else(|| "\u{2014}".to_string()),
            slot.database.clone().unwrap_or_else(|| "\u{2014}".to_string()),
            active.to_string(),
            retained,
            lag,
            slot.wal_status.clone().unwrap_or_else(|| "\u{2014}".to_string()),
            safe,
        ])
        .style(Style::new().fg(sev.color()))
    } else {
        Row::new([
            sev.marker().to_string(),
            format::truncate_with_ellipsis(&slot.slot_name, slot_width),
            slot.slot_type.clone(),
            active.to_string(),
            retained,
            lag,
            slot.wal_status.clone().unwrap_or_else(|| "\u{2014}".to_string()),
        ])
        .style(Style::new().fg(sev.color()))
    }
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

    let slot_width = slot_column_width(area.width);

    let (header, widths, tier): (Row<'static>, Vec<Constraint>, u8) = if area.width >= 135 {
        let header = Row::new([
            "!",
            "Slot",
            "Type",
            "Plugin",
            "DB",
            "Active",
            "Retained",
            "Lag",
            "WAL Status",
            "Safe Size",
            "xmin Age",
            "Invalidated",
        ])
        .style(Style::new().bold());

        let widths = vec![
            Constraint::Length(SEVERITY_WIDTH),
            Constraint::Min(16),
            Constraint::Length(FIXED_WIDTHS_WIDE[0]),
            Constraint::Length(FIXED_WIDTHS_WIDE[1]),
            Constraint::Length(FIXED_WIDTHS_WIDE[2]),
            Constraint::Length(FIXED_WIDTHS_WIDE[3]),
            Constraint::Length(FIXED_WIDTHS_WIDE[4]),
            Constraint::Length(FIXED_WIDTHS_WIDE[5]),
            Constraint::Length(FIXED_WIDTHS_WIDE[6]),
            Constraint::Length(FIXED_WIDTHS_WIDE[7]),
            Constraint::Length(FIXED_WIDTHS_WIDE[8]),
            Constraint::Length(FIXED_WIDTHS_WIDE[9]),
        ];
        (header, widths, 0)
    } else if area.width >= 100 {
        let header = Row::new([
            "!",
            "Slot",
            "Type",
            "Plugin",
            "DB",
            "Active",
            "Retained",
            "Lag",
            "WAL Status",
            "Safe Size",
        ])
        .style(Style::new().bold());

        let widths = vec![
            Constraint::Length(SEVERITY_WIDTH),
            Constraint::Min(16),
            Constraint::Length(FIXED_WIDTHS_MED[0]),
            Constraint::Length(FIXED_WIDTHS_MED[1]),
            Constraint::Length(FIXED_WIDTHS_MED[2]),
            Constraint::Length(FIXED_WIDTHS_MED[3]),
            Constraint::Length(FIXED_WIDTHS_MED[4]),
            Constraint::Length(FIXED_WIDTHS_MED[5]),
            Constraint::Length(FIXED_WIDTHS_MED[6]),
            Constraint::Length(FIXED_WIDTHS_MED[7]),
        ];
        (header, widths, 1)
    } else {
        let header = Row::new([
            "!",
            "Slot",
            "Type",
            "Active",
            "Retained",
            "Lag",
            "WAL Status",
        ])
        .style(Style::new().bold());

        let widths = vec![
            Constraint::Length(SEVERITY_WIDTH),
            Constraint::Min(16),
            Constraint::Length(FIXED_WIDTHS_NARROW[0]),
            Constraint::Length(FIXED_WIDTHS_NARROW[1]),
            Constraint::Length(FIXED_WIDTHS_NARROW[2]),
            Constraint::Length(FIXED_WIDTHS_NARROW[3]),
            Constraint::Length(FIXED_WIDTHS_NARROW[4]),
        ];
        (header, widths, 2)
    };

    let rows = app
        .replication_row_order
        .iter()
        .filter_map(|&i| slots.get(i))
        .map(|slot| slot_table_row(slot, slot_width, tier));

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::bordered().title("Replication Slots"))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("\u{25b6} ");

    frame.render_stateful_widget(table, area, &mut app.replication_table_state);
}

/// Floating modal dialog rendering complete diagnostics for the selected slot.
fn draw_slot_detail(app: &App, frame: &mut Frame, area: Rect) {
    let Some(slot) = app.selected_replication_slot() else {
        return;
    };
    let width = 88.min(area.width.saturating_sub(4));
    let height = 21.min(area.height.saturating_sub(2));
    let [panel_area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [panel_area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(panel_area);

    frame.render_widget(Clear, panel_area);

    let sev = slot_severity(slot);
    let title = format!(" Replication Slot Details \u{2014} {} ", slot.slot_name);
    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(sev.color()));
    let inner = block.inner(panel_area);
    frame.render_widget(block, panel_area);

    let active_str = if slot.active { "active" } else { "inactive" };
    let status_str = slot.wal_status.as_deref().unwrap_or("\u{2014}");
    let safe_str = slot
        .safe_wal_size
        .map_or_else(|| "unlimited / n/a".to_string(), format::human_bytes);
    let retained_str = slot
        .retained_wal_bytes
        .map_or_else(|| "\u{2014}".to_string(), format::human_bytes);
    let lag_str = slot
        .consumer_lag_bytes
        .map_or_else(|| "\u{2014}".to_string(), format::human_bytes);

    let lines = vec![
        Line::from(vec![
            Span::styled("Type: ", style::label_style()),
            Span::styled(slot.slot_type.clone(), style::value_style()),
            Span::styled("  \u{2502}  Plugin: ", style::label_style()),
            Span::styled(slot.plugin.as_deref().unwrap_or("\u{2014}"), style::value_style()),
            Span::styled("  \u{2502}  Database: ", style::label_style()),
            Span::styled(slot.database.as_deref().unwrap_or("\u{2014}"), style::value_style()),
            Span::styled("  \u{2502}  Temporary: ", style::label_style()),
            Span::styled(if slot.temporary { "yes" } else { "no" }, style::value_style()),
        ]),
        Line::from(vec![
            Span::styled("State: ", style::label_style()),
            Span::styled(
                active_str,
                if slot.active {
                    Style::new().fg(Color::Green).bold()
                } else {
                    Style::new().fg(Color::Yellow).bold()
                },
            ),
            Span::styled("  \u{2502}  Active PID: ", style::label_style()),
            Span::styled(
                slot.active_pid.map_or_else(|| "\u{2014}".to_string(), |p| p.to_string()),
                style::value_style(),
            ),
            Span::styled("  \u{2502}  Client App: ", style::label_style()),
            Span::styled(slot.application_name.as_deref().unwrap_or("\u{2014}"), style::value_style()),
            Span::styled("  \u{2502}  Client IP: ", style::label_style()),
            Span::styled(slot.client_addr.as_deref().unwrap_or("\u{2014}"), style::value_style()),
        ]),
        Line::default(),
        Line::from(Span::styled("\u{2500}\u{2500} WAL & Replication Lag \u{2500}\u{2500}", style::label_style())),
        Line::from(vec![
            Span::styled("Restart LSN: ", style::label_style()),
            Span::styled(slot.restart_lsn.as_deref().unwrap_or("\u{2014}"), style::value_style()),
            Span::styled("  \u{2502}  Confirmed Flush LSN: ", style::label_style()),
            Span::styled(slot.confirmed_flush_lsn.as_deref().unwrap_or("\u{2014}"), style::value_style()),
        ]),
        Line::from(vec![
            Span::styled("Retained WAL on Disk: ", style::label_style()),
            Span::styled(retained_str, Style::new().fg(sev.color()).bold()),
            Span::styled("  \u{2502}  Consumer Lag: ", style::label_style()),
            Span::styled(lag_str, style::value_style()),
        ]),
        Line::from(vec![
            Span::styled("WAL Status: ", style::label_style()),
            Span::styled(status_str, style::value_style()),
            Span::styled("  \u{2502}  Safe WAL Headroom: ", style::label_style()),
            Span::styled(safe_str, style::value_style()),
        ]),
        Line::default(),
        Line::from(Span::styled("\u{2500}\u{2500} Transaction Horizons & Bloat \u{2500}\u{2500}", style::label_style())),
        Line::from(vec![
            Span::styled("xmin Age: ", style::label_style()),
            Span::styled(
                slot.xmin_age.map_or_else(|| "\u{2014}".to_string(), format::human_count),
                if slot.xmin_age.unwrap_or(0) > 10_000_000 {
                    Style::new().fg(Color::Yellow).bold()
                } else {
                    style::value_style()
                },
            ),
            Span::styled(" (holds back autovacuum)", style::label_style()),
            Span::styled("  \u{2502}  catalog_xmin Age: ", style::label_style()),
            Span::styled(
                slot.catalog_xmin_age.map_or_else(|| "\u{2014}".to_string(), format::human_count),
                style::value_style(),
            ),
            Span::styled(" (holds back catalog vacuum)", style::label_style()),
        ]),
        Line::default(),
        Line::from(Span::styled("\u{2500}\u{2500} Advanced Flags \u{2500}\u{2500}", style::label_style())),
        Line::from(vec![
            Span::styled("Two-Phase: ", style::label_style()),
            Span::styled(slot.two_phase.map_or("\u{2014}", |b| if b { "yes" } else { "no" }), style::value_style()),
            Span::styled("  \u{2502}  Standby Conflict: ", style::label_style()),
            Span::styled(
                slot.conflicting.map_or("\u{2014}", |b| if b { "YES (recovery conflict)" } else { "no" }),
                if slot.conflicting.unwrap_or(false) {
                    Style::new().fg(Color::Red).bold()
                } else {
                    style::value_style()
                },
            ),
            Span::styled("  \u{2502}  Invalidated: ", style::label_style()),
            Span::styled(
                slot.invalidated.as_deref().unwrap_or("no"),
                if slot.invalidated.is_some() {
                    Style::new().fg(Color::Red).bold()
                } else {
                    style::value_style()
                },
            ),
        ]),
        Line::default(),
        Line::from(Span::styled("Enter / Esc: close detail", style::label_style().italic())),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

/// How many characters the flexible Slot column can hold at this terminal
/// width (same arithmetic as the Schema/Index Lens's flexible columns).
fn slot_column_width(area_width: u16) -> usize {
    let (fixed, cols): (u16, u16) = if area_width >= 135 {
        (FIXED_WIDTHS_WIDE.iter().sum::<u16>() + SEVERITY_WIDTH, 11)
    } else if area_width >= 100 {
        (FIXED_WIDTHS_MED.iter().sum::<u16>() + SEVERITY_WIDTH, 9)
    } else {
        (FIXED_WIDTHS_NARROW.iter().sum::<u16>() + SEVERITY_WIDTH, 6)
    };
    let overhead = 2 /* block borders */ + HIGHLIGHT_WIDTH + fixed + cols * COLUMN_SPACING;
    usize::from(area_width.saturating_sub(overhead)).max(16)
}

fn draw_footer(app: &App, frame: &mut Frame, area: Rect) {
    let n = app
        .snapshot
        .replication_slots
        .as_deref()
        .map_or(0, <[_]>::len);
    let line = Line::from(format!(
        " {n} slot{} \u{b7} Enter: detail \u{b7} worst severity first",
        if n == 1 { "" } else { "s" }
    ))
    .dim();
    frame.render_widget(Paragraph::new(line), area);
}
