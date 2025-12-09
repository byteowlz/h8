//! Right pane: Email preview.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::app::{App, FocusedPane};

/// Draw the right pane (email preview).
pub fn draw_right_pane(frame: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_pane == FocusedPane::Right;

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(" Preview ");

    let Some(email) = app.current_email() else {
        let para = Paragraph::new(Line::from(Span::styled(
            "Select an email to preview",
            Style::default().fg(Color::DarkGray),
        )))
        .block(block);
        frame.render_widget(para, area);
        return;
    };

    // Build preview content
    let mut lines = Vec::new();

    // Subject
    let subject = email.subject.as_deref().unwrap_or("(no subject)");
    lines.push(Line::from(vec![
        Span::styled("Subject: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(subject),
    ]));

    // From
    let from = email.from_addr.as_deref().unwrap_or("unknown");
    lines.push(Line::from(vec![
        Span::styled("From: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(from),
    ]));

    // Date
    if let Some(date) = &email.received_at {
        lines.push(Line::from(vec![
            Span::styled("Date: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(date.as_str()),
        ]));
    }

    // Status indicators
    let mut status_parts = Vec::new();
    if !email.is_read {
        status_parts.push(Span::styled("UNREAD", Style::default().fg(Color::Yellow)));
    }
    if email.is_draft {
        status_parts.push(Span::styled("DRAFT", Style::default().fg(Color::Magenta)));
    }
    if email.has_attachments {
        status_parts.push(Span::styled("ATTACHMENTS", Style::default().fg(Color::Cyan)));
    }
    if !status_parts.is_empty() {
        let mut status_line = vec![Span::styled(
            "Status: ",
            Style::default().add_modifier(Modifier::BOLD),
        )];
        for (i, part) in status_parts.into_iter().enumerate() {
            if i > 0 {
                status_line.push(Span::raw(" | "));
            }
            status_line.push(part);
        }
        lines.push(Line::from(status_line));
    }

    // Separator
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "---",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(""));

    // Body placeholder (actual body would come from loading full email)
    lines.push(Line::from(Span::styled(
        "Press Enter to view full email...",
        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
    )));

    let para = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });

    frame.render_widget(para, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use h8_core::types::MessageSync;
    use ratatui::{backend::TestBackend, Terminal};

    fn create_test_email() -> MessageSync {
        MessageSync {
            local_id: "1".to_string(),
            remote_id: "r1".to_string(),
            change_key: None,
            folder: "inbox".to_string(),
            subject: Some("Test Subject".to_string()),
            from_addr: Some("sender@example.com".to_string()),
            received_at: Some("2024-01-15 10:30:00".to_string()),
            is_read: false,
            is_draft: false,
            has_attachments: false,
            synced_at: None,
            local_hash: None,
        }
    }

    #[test]
    fn test_draw_right_pane_empty() {
        let backend = TestBackend::new(50, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        let app = App::new();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_right_pane(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_right_pane_with_email() {
        let backend = TestBackend::new(50, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        app.emails.push(create_test_email());

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_right_pane(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_right_pane_draft_email() {
        let backend = TestBackend::new(50, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        let mut email = create_test_email();
        email.is_draft = true;
        email.is_read = true;
        app.emails.push(email);

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_right_pane(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_right_pane_focused() {
        let backend = TestBackend::new(50, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        app.emails.push(create_test_email());
        app.focus_right();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_right_pane(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_right_pane_with_attachments() {
        let backend = TestBackend::new(50, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        let mut email = create_test_email();
        email.has_attachments = true;
        app.emails.push(email);

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_right_pane(frame, &app, area);
            })
            .unwrap();
    }
}
