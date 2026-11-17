use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::time::Duration;

pub struct Measurement {
    pub label: &'static str,
    pub elapsed: Duration,
    pub failed: bool,
}
impl Measurement {
    pub fn text(&self) -> String {
        let time = if self.elapsed < Duration::from_millis(1) {
            "<1ms".into()
        } else {
            format!("{}ms", self.elapsed.as_millis())
        };
        format!(
            "{}{} {time}",
            self.label,
            if self.failed { "!" } else { "" }
        )
    }
}

#[derive(Clone, Copy)]
pub enum FooterMode {
    Browse,
    Search { editing: bool },
    Help,
}

fn key_spans(key: &'static str, label: &'static str) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            key,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {label}"), Style::default().fg(Color::Gray)),
    ]
}
fn shortcuts(
    width: usize,
    groups: &[(&'static str, &'static str)],
    tail: &[(&'static str, &'static str)],
) -> Line<'static> {
    let mut right = Line::default();
    for &(key, label) in tail {
        if !right.spans.is_empty() {
            right.spans.push(Span::raw("  "));
        }
        right.spans.extend(key_spans(key, label));
    }
    let available =
        width.saturating_sub(right.width() + if right.spans.is_empty() { 0 } else { 2 });
    let mut left = Line::default();
    for &(key, label) in groups {
        let item = key_spans(key, label);
        let needed = Line::from(item.clone()).width() + if left.spans.is_empty() { 0 } else { 2 };
        if left.width() + needed > available {
            continue;
        }
        if !left.spans.is_empty() {
            left.spans.push(Span::raw("  "));
        }
        left.spans.extend(item);
    }
    if !right.spans.is_empty() {
        left.spans.push(Span::raw(
            " ".repeat(width.saturating_sub(left.width() + right.width())),
        ));
        left.spans.extend(right.spans);
    }
    left
}

pub fn footer(width: u16, height: u16, mode: FooterMode) -> Vec<Line<'static>> {
    let width = usize::from(width);
    match mode {
        FooterMode::Help => vec![shortcuts(width, &[("Any key", "Close help")], &[])],
        FooterMode::Search { editing } => {
            let mut rows = vec![shortcuts(
                width,
                &[
                    ("Enter", if editing { "Search" } else { "Open" }),
                    ("↑↓", "Select"),
                ],
                &[("Esc", "Back")],
            )];
            if height > 1 {
                rows.push(shortcuts(
                    width,
                    &[("Type", "Edit query"), ("Backspace", "Delete")],
                    &[],
                ));
            }
            rows
        }
        FooterMode::Browse if height < 2 => vec![shortcuts(
            width,
            &[
                ("Enter", "Open"),
                ("d", "Diff"),
                ("p", "Files"),
                ("/", "Find"),
            ],
            &[("?", "Help"), ("q", "Quit")],
        )],
        FooterMode::Browse => vec![
            shortcuts(
                width,
                &[
                    ("Enter", "Open"),
                    ("Tab", "Focus"),
                    ("↑↓", "Move"),
                    ("←→", "Tree/scroll"),
                    ("PgUp/Dn", "Page"),
                    ("Home/End", "Ends"),
                ],
                &[("?", "Help"), ("q", "Quit")],
            ),
            shortcuts(
                width,
                &[
                    ("d", "Diff"),
                    ("g", "Changes"),
                    ("p", "Files"),
                    ("/", "Find"),
                    ("^f", "Grep"),
                    ("i", "Ignored"),
                    ("r", "Refresh"),
                ],
                &[],
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }
    #[test]
    fn shortcuts_fit_narrow_panes_and_keep_help_and_exit_visible() {
        for width in [35, 40, 60, 80, 120] {
            for height in [1, 2] {
                for mode in [
                    FooterMode::Browse,
                    FooterMode::Search { editing: true },
                    FooterMode::Search { editing: false },
                    FooterMode::Help,
                ] {
                    let rows = footer(width, height, mode);
                    assert!(rows.len() <= height as usize);
                    assert!(rows.iter().all(|r| r.width() <= width as usize));
                    if matches!(mode, FooterMode::Browse) {
                        assert!(text(&rows[0]).contains("? Help"));
                        assert!(text(&rows[0]).ends_with("q Quit"));
                    }
                }
            }
        }
        let rows = footer(120, 2, FooterMode::Browse);
        assert!(text(&rows[0]).contains("Home/End"));
        assert!(text(&rows[1]).contains("r Refresh"));
    }
    #[test]
    fn measurement_distinguishes_fast_work_and_errors() {
        let mut sample = Measurement {
            label: "open",
            elapsed: Duration::from_micros(42),
            failed: false,
        };
        assert_eq!(sample.text(), "open <1ms");
        sample.elapsed = Duration::from_millis(123);
        sample.failed = true;
        assert_eq!(sample.text(), "open! 123ms");
    }
}
