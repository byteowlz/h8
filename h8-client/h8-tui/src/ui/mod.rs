//! UI rendering module for the TUI.
//!
//! Provides the three-pane layout with status bar and modal overlays.

mod left_pane;
mod middle_pane;
mod overlays;
mod right_pane;
mod status_bar;

pub use left_pane::draw_left_pane;
pub use middle_pane::draw_middle_pane;
pub use overlays::{draw_delete_confirm, draw_help, draw_sort_menu, draw_which_key};
pub use right_pane::draw_right_pane;
pub use status_bar::draw_status_bar;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
};

use crate::app::App;

/// Draw the main application UI.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Main layout: content area + status bar
    let main_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    let content_area = main_layout[0];
    let status_area = main_layout[1];

    // Three-pane layout: left (20%) | middle (40%) | right (40%)
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Percentage(40),
            Constraint::Percentage(40),
        ])
        .split(content_area);

    // Draw the three panes
    draw_left_pane(frame, app, panes[0]);
    draw_middle_pane(frame, app, panes[1]);
    draw_right_pane(frame, app, panes[2]);

    // Draw status bar
    draw_status_bar(frame, app, status_area);

    // Draw modal overlays if active
    use crate::app::AppMode;
    match &app.mode {
        AppMode::Help => draw_help(frame, app, area),
        AppMode::Sort => draw_sort_menu(frame, app, area),
        AppMode::Delete | AppMode::DeleteMultiple => draw_delete_confirm(frame, app, area),
        AppMode::WhichKey(ctx) => draw_which_key(frame, ctx, area),
        AppMode::Search(_) => overlays::draw_search(frame, app, area),
        _ => {}
    }
}

/// Create a centered rectangle for popups.
pub fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_centered_rect() {
        let area = Rect::new(0, 0, 100, 50);
        let centered = centered_rect(50, 50, area);

        // Should be roughly centered
        assert!(centered.x > 0);
        assert!(centered.y > 0);
        assert!(centered.x + centered.width <= area.width);
        assert!(centered.y + centered.height <= area.height);
    }

    #[test]
    fn test_centered_rect_small_area() {
        let area = Rect::new(0, 0, 10, 10);
        let centered = centered_rect(80, 80, area);

        // Should still be within bounds
        assert!(centered.x + centered.width <= area.width);
        assert!(centered.y + centered.height <= area.height);
    }
}
