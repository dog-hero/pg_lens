//! Visual replay timeline / scrubber bar component.
//!
//! Renders a prominent visual timeline bar during in-app incident replay,
//! displaying a graphical track (`[████░░░] 42%`), frame counters, elapsed time,
//! playback speed, pause/play state, loop indicator, and keyboard scrubbing hints.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::App;
use crate::ui::style;

/// Draws the replay timeline and scrubber bar across `area`.
pub fn draw(app: &App, frame: &mut Frame, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let Some(ref replay) = app.replay_state else {
        return;
    };

    let line = build_scrubber_line(replay, area.width);
    frame.render_widget(Paragraph::new(line), area);
}

/// Builds the formatted line for the scrubber bar.
pub fn build_scrubber_line(replay: &crate::app::ReplayState, width: u16) -> Line<'static> {
    let total = replay.frames.len();
    let current = replay.current_idx;
    let pct = if total > 1 {
        (current as f64 / (total - 1) as f64 * 100.0).clamp(0.0, 100.0)
    } else {
        100.0
    };

    // Elapsed delta from frame 0
    let elapsed_str = if let (Some(first), Some(curr)) = (replay.frames.first(), replay.frames.get(current)) {
        let delta_secs = curr.vitals.uptime_secs.saturating_sub(first.vitals.uptime_secs);
        let mm = delta_secs / 60;
        let ss = delta_secs % 60;
        if mm > 0 {
            format!("+{mm:02}:{ss:02}")
        } else {
            format!("+{ss}s")
        }
    } else {
        "+0s".to_string()
    };

    let state_badge = if replay.is_paused {
        Span::styled(" PAUSED ", Style::new().fg(Color::Black).bg(Color::Yellow).bold())
    } else {
        Span::styled(" PLAY ", Style::new().fg(Color::Black).bg(Color::Green).bold())
    };

    let speed_badge = Span::styled(
        format!("{:.2}x", replay.speed),
        Style::new().fg(Color::Cyan).bold(),
    );

    let sep = Span::styled(" │ ", style::label_style());

    // Dynamically adjust progress track width
    let bar_width = if width >= 120 {
        24
    } else if width >= 90 {
        16
    } else {
        8
    };

    let filled = ((pct / 100.0) * bar_width as f64).round() as usize;
    let filled = filled.min(bar_width);
    let empty = bar_width.saturating_sub(filled);
    let progress_bar = format!(
        "[{}{}] {:>3.0}%",
        "█".repeat(filled),
        "░".repeat(empty),
        pct
    );

    let mut spans = vec![
        Span::raw(" "),
        Span::styled("REPLAY", Style::new().fg(Color::Black).bg(Color::Cyan).bold()),
        Span::raw(" "),
        state_badge,
        Span::raw(" "),
        speed_badge,
        Span::raw(" "),
    ];

    if replay.loop_playback {
        spans.push(Span::styled("LOOP ", Style::new().fg(Color::Magenta).bold()));
    }

    spans.push(Span::styled(progress_bar, Style::new().fg(Color::White).bold()));
    spans.push(sep.clone());
    spans.push(Span::styled(
        format!("Frame {}/{} ({})", current + 1, total, elapsed_str),
        Style::new().fg(Color::Yellow).bold(),
    ));

    // Only show quick hints if width permits
    if width >= 100 {
        spans.push(sep.clone());
        let [sk, sd] = style::hint("Space", ": play");
        spans.push(sk);
        spans.push(sd);
        spans.push(Span::raw(" "));
        let [arr_k, arr_d] = style::hint("←/→", ": scrub");
        spans.push(arr_k);
        spans.push(arr_d);
    }
    if width >= 120 {
        spans.push(Span::raw(" "));
        let [sp_k, sp_d] = style::hint("[/]", ": speed");
        spans.push(sp_k);
        spans.push(sp_d);
        spans.push(Span::raw(" "));
        let [l_k, l_d] = style::hint("l", ": loop");
        spans.push(l_k);
        spans.push(l_d);
    }

    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pg_lens_core::DbSnapshot;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Instant;

    #[test]
    fn scrubber_builds_line_with_correct_progress_and_badges() {
        let mut snap1 = DbSnapshot::mock();
        snap1.vitals.uptime_secs = 1000;
        let mut snap2 = DbSnapshot::mock();
        snap2.vitals.uptime_secs = 1015;
        let mut snap3 = DbSnapshot::mock();
        snap3.vitals.uptime_secs = 1030;

        let replay = crate::app::ReplayState {
            frames: vec![Arc::new(snap1), Arc::new(snap2), Arc::new(snap3)],
            current_idx: 1,
            is_paused: true,
            speed: 1.0,
            loop_playback: true,
            source_path: PathBuf::from("test.jsonl"),
            last_frame_time: Instant::now(),
        };

        let line = build_scrubber_line(&replay, 140);
        let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();

        assert!(rendered.contains("REPLAY"));
        assert!(rendered.contains("PAUSED"));
        assert!(rendered.contains("1.00x"));
        assert!(rendered.contains("LOOP"));
        assert!(rendered.contains("50%"));
        assert!(rendered.contains("Frame 2/3 (+15s)"));
        assert!(rendered.contains("Space: play"));
        assert!(rendered.contains("←/→: scrub"));
        assert!(rendered.contains("[/]: speed"));
        assert!(rendered.contains("l: loop"));
    }

    #[test]
    fn scrubber_adapts_to_narrow_terminal() {
        let snap = Arc::new(DbSnapshot::mock());
        let replay = crate::app::ReplayState {
            frames: vec![snap],
            current_idx: 0,
            is_paused: false,
            speed: 2.0,
            loop_playback: false,
            source_path: PathBuf::from("test.jsonl"),
            last_frame_time: Instant::now(),
        };

        let line = build_scrubber_line(&replay, 80);
        let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();

        assert!(rendered.contains("PLAY"));
        assert!(rendered.contains("2.00x"));
        assert!(rendered.contains("100%"));
        assert!(rendered.contains("Frame 1/1 (+0s)"));
        // Narrow width omits extra hints
        assert!(!rendered.contains("Space: play"));
    }
}

