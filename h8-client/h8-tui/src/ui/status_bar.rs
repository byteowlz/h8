//! Status bar at the bottom of the screen.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{App, AppMode, FocusedPane};

/// Draw the status bar.
pub fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();

    // Mode indicator
    let (mode_text, mode_color) = get_mode_info(&app.mode);
    spans.push(Span::styled(
        format!(" {} ", mode_text),
        Style::default()
            .fg(Color::Black)
            .bg(mode_color)
            .add_modifier(Modifier::BOLD),
    ));

    // Focused pane indicator
    let pane_text = match app.focused_pane {
        FocusedPane::Left => " Folders ",
        FocusedPane::Middle => " List ",
        FocusedPane::Right => " Preview ",
    };
    spans.push(Span::styled(
        pane_text,
        Style::default().fg(Color::Black).bg(Color::DarkGray),
    ));

    // Current folder
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        app.current_folder_display(),
        Style::default().fg(Color::Cyan),
    ));

    // Email count / selection info
    spans.push(Span::raw(" | "));
    if app.email_selection.has_selections() {
        spans.push(Span::styled(
            format!("{} selected", app.email_selection.selected_indices.len()),
            Style::default().fg(Color::Green),
        ));
    } else if !app.emails.is_empty() {
        spans.push(Span::raw(format!(
            "{}/{}",
            app.email_selection.index + 1,
            app.emails.len()
        )));
    } else {
        spans.push(Span::styled("Empty", Style::default().fg(Color::DarkGray)));
    }

    // G-prefix indicator
    if app.g_prefix {
        spans.push(Span::raw(" | "));
        spans.push(Span::styled("g-", Style::default().fg(Color::Yellow)));
    }

    // Status message
    if let Some(msg) = &app.status_message {
        spans.push(Span::raw(" | "));
        spans.push(Span::styled(
            msg.as_str(),
            Style::default().fg(Color::Yellow),
        ));
    }

    // Right-aligned help hint
    let help_text = get_help_hint(&app.mode);
    let content_width: usize = spans.iter().map(|s| s.content.len()).sum();
    let padding = area.width as usize - content_width - help_text.len() - 1;
    if padding > 0 {
        spans.push(Span::raw(" ".repeat(padding)));
    }
    spans.push(Span::styled(
        help_text,
        Style::default().fg(Color::DarkGray),
    ));

    let para = Paragraph::new(Line::from(spans));
    frame.render_widget(para, area);
}

fn get_mode_info(mode: &AppMode) -> (&'static str, Color) {
    match mode {
        AppMode::Normal => ("NORMAL", Color::Blue),
        AppMode::Search(_) => ("SEARCH", Color::Green),
        AppMode::Delete | AppMode::DeleteMultiple => ("DELETE", Color::Red),
        AppMode::Help => ("HELP", Color::Magenta),
        AppMode::Sort => ("SORT", Color::Yellow),
        AppMode::WhichKey(_) => ("WHICH", Color::Cyan),
        AppMode::FolderSelect => ("FOLDER", Color::Yellow),
        AppMode::Compose => ("COMPOSE", Color::Green),
    }
}

fn get_help_hint(mode: &AppMode) -> &'static str {
    match mode {
        AppMode::Normal => "? help | q quit",
        AppMode::Search(_) => "Tab: mode | Enter: search | Esc: cancel",
        AppMode::Delete | AppMode::DeleteMultiple => "y: confirm | n: cancel",
        AppMode::Help => "j/k: scroll | Esc: close",
        AppMode::Sort => "j/k: move | Enter: apply | Esc: cancel",
        AppMode::WhichKey(_) => "key: select | Esc: cancel",
        AppMode::FolderSelect => "Enter: select | Esc: cancel",
        AppMode::Compose => "Ctrl-s: send | Esc: cancel",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SearchMode;
    use h8_core::types::MessageSync;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn test_draw_status_bar_normal() {
        let backend = TestBackend::new(80, 1);
        let mut terminal = Terminal::new(backend).unwrap();

        let app = App::new();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_status_bar(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_status_bar_search_mode() {
        let backend = TestBackend::new(80, 1);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        app.mode = AppMode::Search(SearchMode::Subject);

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_status_bar(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_status_bar_with_selections() {
        let backend = TestBackend::new(80, 1);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        app.emails.push(MessageSync {
            local_id: "1".to_string(),
            remote_id: "r1".to_string(),
            change_key: None,
            folder: "inbox".to_string(),
            subject: Some("Test".to_string()),
            from_addr: None,
            received_at: None,
            is_read: true,
            is_draft: false,
            has_attachments: false,
            synced_at: None,
            local_hash: None,
        });
        app.email_selection.toggle_selection();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_status_bar(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_status_bar_g_prefix() {
        let backend = TestBackend::new(80, 1);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        app.g_prefix = true;

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_status_bar(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_status_bar_with_message() {
        let backend = TestBackend::new(80, 1);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        app.set_status("Emails deleted");

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_status_bar(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_get_mode_info() {
        assert_eq!(get_mode_info(&AppMode::Normal).0, "NORMAL");
        assert_eq!(get_mode_info(&AppMode::Help).0, "HELP");
        assert_eq!(get_mode_info(&AppMode::Delete).0, "DELETE");
    }

    #[test]
    fn test_get_help_hint() {
        assert!(get_help_hint(&AppMode::Normal).contains("help"));
        assert!(get_help_hint(&AppMode::Search(SearchMode::Subject)).contains("Esc"));
    }
}
