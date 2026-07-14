//! Shared visual palette + key/value styling helpers for the view layer.
//!
//! One place defines the three roles every "label: value" surface uses —
//! Macro Lens vitals, detail panels, the statusbar and the splash screen —
//! so the whole TUI reads consistently:
//! - LABEL: dim gray — the static text ("Active", "size:", key hints);
//! - VALUE: bold default fg — the data the user actually scans for;
//! - ACCENT: cyan — emphasis (wordmark, block titles, keybinding letters).
//!
//! Severity colors (yellow/red for waiting/blocked/bloat) are deliberately
//! NOT defined here: they stay with the lenses that own their semantics.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Emphasis color: wordmark, panel titles, keybinding letters.
pub const ACCENT: Color = Color::Cyan;

/// Style of static labels/keys ("Active", "size:", statusbar descriptions).
pub fn label_style() -> Style {
    Style::new().fg(Color::DarkGray)
}

/// Style of the values next to those labels.
pub fn value_style() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

/// Accent style (bold cyan) for titles and keybinding letters.
pub fn accent_style() -> Style {
    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
}

/// A `label value` line: dim label span + bold value span. The label carries
/// its own separator/padding (e.g. `"Active : "`) so callers keep full
/// control of alignment; the rendered text is exactly `label + value`.
pub fn kv(label: impl Into<String>, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(label.into(), label_style()),
        Span::styled(value.into(), value_style()),
    ])
}

/// A statusbar keybinding hint: the key letters in accent, the description
/// dim — `hint("q/Esc", ": quit")` renders as `q/Esc: quit`.
pub fn hint(key: impl Into<String>, desc: impl Into<String>) -> [Span<'static>; 2] {
    [
        Span::styled(key.into(), Style::new().fg(ACCENT)),
        Span::styled(desc.into(), label_style()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_splits_label_and_value_into_two_styled_spans() {
        let line = kv("Active : ", "42");
        assert_eq!(line.spans.len(), 2);
        assert_eq!(line.spans[0].content, "Active : ");
        assert_eq!(line.spans[0].style, label_style());
        assert_eq!(line.spans[1].content, "42");
        assert_eq!(line.spans[1].style, value_style());
        // The rendered text is exactly label + value.
        assert_eq!(
            line.spans.iter().map(|s| s.content.as_ref()).collect::<String>(),
            "Active : 42"
        );
    }

    #[test]
    fn hint_accents_the_key_and_dims_the_description() {
        let [key, desc] = hint("q/Esc", ": quit");
        assert_eq!(key.content, "q/Esc");
        assert_eq!(key.style.fg, Some(ACCENT));
        assert_eq!(desc.content, ": quit");
        assert_eq!(desc.style, label_style());
    }

    #[test]
    fn label_value_and_accent_are_visually_distinct() {
        assert_ne!(label_style(), value_style());
        assert_ne!(label_style(), accent_style());
        assert_ne!(value_style(), accent_style());
    }
}
