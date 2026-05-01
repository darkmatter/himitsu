//! Top-level view modules for the ratatui TUI.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub mod command_palette;
pub mod envs;
pub mod envs_dsl_editor;
pub mod envs_text;
pub mod help;
pub mod init_wizard;
pub mod new_secret;
pub mod search;
pub mod secret_viewer;
pub mod store_picker;

fn standard_canvas(area: Rect) -> Rect {
    const MARGIN: u16 = 4;
    const MAX_WIDTH: u16 = 80;
    const MAX_HEIGHT: u16 = 30;

    let width = constrained_axis(area.width, MARGIN, MAX_WIDTH);
    let height = constrained_axis(area.height, MARGIN, MAX_HEIGHT);

    Rect {
        x: area.x + (area.width.saturating_sub(width) / 2),
        y: area.y + (area.height.saturating_sub(height) / 2),
        width,
        height,
    }
}

fn constrained_axis(size: u16, margin: u16, max: u16) -> u16 {
    if size > margin.saturating_mul(2) {
        size.saturating_sub(margin.saturating_mul(2)).min(max)
    } else {
        size.min(max)
    }
}

fn render_distributed_footer(frame: &mut Frame<'_>, area: Rect, items: Vec<Line<'_>>) {
    if items.is_empty() {
        return;
    }

    let constraints = vec![Constraint::Ratio(1, items.len() as u32); items.len()];
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    for (item, chunk) in items.into_iter().zip(chunks.iter()) {
        frame.render_widget(Paragraph::new(item).alignment(Alignment::Center), *chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_canvas_centers_odd_width_without_right_bias() {
        let area = Rect::new(0, 0, 101, 40);
        let canvas = standard_canvas(area);
        assert_eq!(canvas, Rect::new(10, 5, 80, 30));
    }

    #[test]
    fn standard_canvas_preserves_margin_below_max_width() {
        let area = Rect::new(0, 0, 87, 35);
        let canvas = standard_canvas(area);
        assert_eq!(canvas, Rect::new(4, 4, 79, 27));
    }

    #[test]
    fn standard_canvas_uses_full_axis_when_margin_would_collapse() {
        let area = Rect::new(0, 0, 7, 6);
        let canvas = standard_canvas(area);
        assert_eq!(canvas, area);
    }
}
