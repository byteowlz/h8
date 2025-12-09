//! Left pane: Folder/filter navigation.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

use crate::app::{App, FocusedPane};

/// Draw the left pane (folder list).
pub fn draw_left_pane(frame: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_pane == FocusedPane::Left;

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(" Folders ");

    let items: Vec<ListItem> = app
        .folders
        .iter()
        .enumerate()
        .map(|(i, folder)| {
            let is_current = folder.name == app.current_folder;
            let is_selected = i == app.folder_selection.index;

            let marker = if is_current { ">" } else { " " };

            let content = if folder.unread_count > 0 {
                format!(
                    "{} {} ({})",
                    marker, folder.display_name, folder.unread_count
                )
            } else {
                format!("{} {}", marker, folder.display_name)
            };

            let style = if is_selected && is_focused {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if is_current {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            ListItem::new(Line::from(Span::styled(content, style)))
        })
        .collect();

    let list = List::new(items).block(block);

    // Use ListState for highlighting
    let mut state = ListState::default();
    if is_focused {
        state.select(Some(app.folder_selection.index));
    }

    frame.render_stateful_widget(list, area, &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn test_draw_left_pane_renders() {
        let backend = TestBackend::new(30, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        let app = App::new();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_left_pane(frame, &app, area);
            })
            .unwrap();

        // Just verify it doesn't panic
    }

    #[test]
    fn test_draw_left_pane_with_focus() {
        let backend = TestBackend::new(30, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        app.focus_left();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_left_pane(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_left_pane_with_unread() {
        let backend = TestBackend::new(30, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        if let Some(folder) = app.folders.get_mut(0) {
            folder.unread_count = 5;
        }

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_left_pane(frame, &app, area);
            })
            .unwrap();
    }
}
