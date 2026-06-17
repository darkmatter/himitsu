//! Top-level view modules for the ratatui TUI.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

pub(crate) use crate::tui::layout::standard_canvas;

pub mod command_palette;
pub mod help;
pub mod init_wizard;
pub mod new_secret;
pub mod outputs;
pub mod outputs_dsl_editor;
pub mod outputs_text;
pub mod recipient_add;
pub mod recipient_list;
pub mod remote_add;
pub mod search;
pub mod secret_viewer;
pub mod store_picker;

fn render_distributed_footer(frame: &mut Frame<'_>, area: Rect, items: Vec<Line<'_>>) {
    if items.is_empty() {
        return;
    }

    let constraints = items
        .iter()
        .map(|item| {
            let width = item.width().saturating_add(2).min(u16::MAX as usize) as u16;
            Constraint::Length(width)
        })
        .collect::<Vec<_>>();
    let chunks = Layout::horizontal(constraints)
        .flex(Flex::SpaceBetween)
        .split(area);

    for (item, chunk) in items.into_iter().zip(chunks.iter()) {
        frame.render_widget(Paragraph::new(item).alignment(Alignment::Center), *chunk);
    }
}
