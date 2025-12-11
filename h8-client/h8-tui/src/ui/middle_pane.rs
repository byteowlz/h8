//! Middle pane: Email list.

use chrono::{DateTime, Datelike, Local};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

use crate::app::{App, FocusedPane};

/// Format a datetime string to a short human-readable string for the list view.
fn format_date_short(iso_date: &str) -> String {
    let parsed = DateTime::parse_from_rfc3339(iso_date)
        .or_else(|_| DateTime::parse_from_str(iso_date, "%Y-%m-%dT%H:%M:%S%z"))
        .map(|dt| dt.with_timezone(&Local));

    let local = match parsed {
        Ok(dt) => dt,
        Err(_) => return String::new(),
    };

    let now = Local::now();
    let today = now.date_naive();
    let date = local.date_naive();

    if date == today {
        local.format("%H:%M").to_string()
    } else if date == today.pred_opt().unwrap_or(today) {
        "Yesterday".to_string()
    } else if (today - date).num_days() < 7 {
        local.format("%a").to_string()
    } else if date.year() == today.year() {
        local.format("%b %-d").to_string()
    } else {
        local.format("%b %Y").to_string()
    }
}

/// Draw the middle pane (email list).
pub fn draw_middle_pane(frame: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_pane == FocusedPane::Middle;

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = format!(" {} ({}) ", app.current_folder_display(), app.emails.len());

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    if app.emails.is_empty() {
        let items = vec![ListItem::new(Line::from(Span::styled(
            "  No emails",
            Style::default().fg(Color::DarkGray),
        )))];
        let list = List::new(items).block(block);
        frame.render_widget(list, area);
        return;
    }

    let inner_height = area.height.saturating_sub(2) as usize; // Account for borders
    let offset = app.email_selection.offset;
    let visible_end = (offset + inner_height).min(app.emails.len());

    let items: Vec<ListItem> = app
        .emails
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible_end - offset)
        .map(|(i, email)| {
            let is_selected_cursor = i == app.email_selection.index;
            let is_multi_selected = app.email_selection.selected_indices.contains(&i);

            // Unread indicator
            let unread = if email.is_read { " " } else { "*" };

            // Attachment indicator
            let attach = if email.has_attachments { "@" } else { " " };

            // Word ID
            let id = &email.local_id;

            // Format date
            let date = email
                .received_at
                .as_deref()
                .map(|dt| format_date_short(dt))
                .unwrap_or_default();

            // From address - extract name or email (truncate)
            let from: String = email
                .from_addr
                .as_deref()
                .unwrap_or("unknown")
                .chars()
                .take(18)
                .collect();

            // Subject (truncate if needed - use remaining space)
            let fixed_width = 3 + 2 + 18 + 1 + 10 + 1 + id.len() + 2; // "[ ] *@ from date id"
            let max_subject_len = (area.width as usize).saturating_sub(fixed_width);
            let subject: String = email
                .subject
                .as_deref()
                .unwrap_or("(no subject)")
                .chars()
                .take(max_subject_len.max(10))
                .collect();

            // Selection marker
            let marker = if is_multi_selected { "[x]" } else { "[ ]" };

            let content = format!(
                "{} {}{} {:18} {:10} {} [{}]",
                marker, unread, attach, from, date, subject, id
            );

            let style = if is_selected_cursor && is_focused {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if is_multi_selected {
                Style::default().fg(Color::Green)
            } else if !email.is_read {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };

            ListItem::new(Line::from(Span::styled(content, style)))
        })
        .collect();

    let list = List::new(items).block(block);

    // ListState for highlighting - adjusted for offset
    let mut state = ListState::default();
    if is_focused && !app.emails.is_empty() {
        let visible_index = app.email_selection.index.saturating_sub(offset);
        state.select(Some(visible_index));
    }

    frame.render_stateful_widget(list, area, &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use h8_core::types::MessageSync;
    use ratatui::{Terminal, backend::TestBackend};

    fn create_test_email(id: &str, subject: &str, is_read: bool) -> MessageSync {
        MessageSync {
            local_id: id.to_string(),
            remote_id: format!("r{}", id),
            change_key: None,
            folder: "inbox".to_string(),
            subject: Some(subject.to_string()),
            from_addr: Some("test@example.com".to_string()),
            received_at: None,
            is_read,
            is_draft: false,
            has_attachments: false,
            synced_at: None,
            local_hash: None,
        }
    }

    #[test]
    fn test_draw_middle_pane_empty() {
        let backend = TestBackend::new(50, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        let app = App::new();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_middle_pane(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_middle_pane_with_emails() {
        let backend = TestBackend::new(50, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        app.emails
            .push(create_test_email("1", "Hello World", false));
        app.emails.push(create_test_email("2", "Re: Hello", true));

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_middle_pane(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_middle_pane_with_selection() {
        let backend = TestBackend::new(50, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        app.emails.push(create_test_email("1", "Hello", false));
        app.emails.push(create_test_email("2", "World", true));
        app.email_selection.toggle_selection();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_middle_pane(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_middle_pane_focused() {
        let backend = TestBackend::new(50, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        app.emails.push(create_test_email("1", "Test", true));
        app.focus_middle();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_middle_pane(frame, &app, area);
            })
            .unwrap();
    }
}
