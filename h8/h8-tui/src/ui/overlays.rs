//! Modal overlay widgets (help, sort menu, delete confirm, etc).

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::app::{App, AppMode, SortOption, WhichKeyContext};
use super::centered_rect;

/// Draw the help overlay.
pub fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let popup_area = centered_rect(70, 90, area);

    // Clear the area behind the popup
    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta))
        .title(" Help (press Esc to close) ");

    let help_text = get_help_text();
    let lines: Vec<Line> = help_text
        .lines()
        .skip(app.help_scroll)
        .map(|line| {
            if line.starts_with('#') {
                Line::from(Span::styled(
                    line,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ))
            } else if line.contains(':') && !line.starts_with(' ') {
                let parts: Vec<&str> = line.splitn(2, ':').collect();
                if parts.len() == 2 {
                    Line::from(vec![
                        Span::styled(
                            format!("{}:", parts[0]),
                            Style::default().fg(Color::Yellow),
                        ),
                        Span::raw(parts[1]),
                    ])
                } else {
                    Line::from(line)
                }
            } else {
                Line::from(line)
            }
        })
        .collect();

    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });

    frame.render_widget(para, popup_area);
}

/// Draw the sort menu overlay.
pub fn draw_sort_menu(frame: &mut Frame, app: &App, area: Rect) {
    let popup_area = centered_rect(40, 30, area);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(" Sort By ");

    let items: Vec<ListItem> = SortOption::ALL
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            let marker = if *opt == app.sort_option { ">" } else { " " };
            let style = if i == app.sort_selection {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(
                format!("{} {}", marker, opt.as_str()),
                style,
            )))
        })
        .collect();

    let list = List::new(items).block(block);

    let mut state = ListState::default();
    state.select(Some(app.sort_selection));

    frame.render_stateful_widget(list, popup_area, &mut state);
}

/// Draw the delete confirmation overlay.
pub fn draw_delete_confirm(frame: &mut Frame, app: &App, area: Rect) {
    let popup_area = centered_rect(50, 20, area);

    frame.render_widget(Clear, popup_area);

    let count = app.selected_count();
    let message = if matches!(app.mode, AppMode::DeleteMultiple) {
        format!("Delete {} emails?", count)
    } else {
        "Delete this email?".to_string()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .title(" Confirm Delete ");

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            message,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("[y]", Style::default().fg(Color::Green)),
            Span::raw(" Yes   "),
            Span::styled("[n]", Style::default().fg(Color::Red)),
            Span::raw(" No"),
        ]),
    ];

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, popup_area);
}

/// Draw the which-key overlay.
pub fn draw_which_key(frame: &mut Frame, context: &WhichKeyContext, area: Rect) {
    // Which-key appears as a bar at the bottom
    let bar_height = 3;
    let bar_area = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(bar_height + 1), // +1 for status bar
        area.width,
        bar_height,
    );

    frame.render_widget(Clear, bar_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(format!(" {} ", context.title()));

    let options = context.options();
    let spans: Vec<Span> = options
        .iter()
        .flat_map(|(key, label)| {
            vec![
                Span::styled(
                    format!("[{}]", key),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" {} ", label)),
                Span::styled("|", Style::default().fg(Color::DarkGray)),
                Span::raw(" "),
            ]
        })
        .collect();

    let line = Line::from(spans);
    let para = Paragraph::new(line).block(block);

    frame.render_widget(para, bar_area);
}

/// Draw the search input overlay.
pub fn draw_search(frame: &mut Frame, app: &App, area: Rect) {
    let bar_height = 3;
    let bar_area = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(bar_height + 1),
        area.width,
        bar_height,
    );

    frame.render_widget(Clear, bar_area);

    let mode_str = if let AppMode::Search(mode) = &app.mode {
        mode.as_str()
    } else {
        "Search"
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green))
        .title(format!(" Search: {} ", mode_str));

    let cursor_char = if app.search_query.is_empty() {
        "_"
    } else {
        ""
    };

    let line = Line::from(vec![
        Span::raw(&app.search_query),
        Span::styled(
            cursor_char,
            Style::default().add_modifier(Modifier::SLOW_BLINK),
        ),
    ]);

    let para = Paragraph::new(line).block(block);
    frame.render_widget(para, bar_area);
}

fn get_help_text() -> &'static str {
    r#"# Navigation

j / Down:     Move down
k / Up:       Move up
h / Left:     Focus left pane
l / Right:    Focus right pane
gg:           Go to top
G:            Go to bottom
Ctrl-d:       Page down
Ctrl-u:       Page up

# Selection

Space:        Toggle selection + move down
Ctrl-a:       Select all
V:            Clear all selections

# Actions

d:            Delete (respects multi-select)
e:            Edit / open external editor
a:            Compose new email
r:            Refresh
Enter:        Open selected email

# Search & Sort

/ or ::       Open search
s:            Open sort menu
Tab:          Cycle search mode (in search)

# Which-Key Menus

f:            Folder actions
g:            Go to actions
t:            Email actions

# Other

?:            Show this help
q:            Quit
Esc:          Cancel / return to normal
"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SearchMode;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn test_draw_help() {
        let backend = TestBackend::new(80, 40);
        let mut terminal = Terminal::new(backend).unwrap();

        let app = App::new();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_help(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_sort_menu() {
        let backend = TestBackend::new(60, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        app.enter_sort();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_sort_menu(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_delete_confirm_single() {
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        app.mode = AppMode::Delete;

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_delete_confirm(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_delete_confirm_multiple() {
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        app.mode = AppMode::DeleteMultiple;

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_delete_confirm(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_which_key() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_which_key(frame, &WhichKeyContext::Folder, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_search() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        app.mode = AppMode::Search(SearchMode::Subject);
        app.search_query = "test query".to_string();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_search(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_draw_search_empty_query() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        app.mode = AppMode::Search(SearchMode::All);

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_search(frame, &app, area);
            })
            .unwrap();
    }

    #[test]
    fn test_get_help_text_not_empty() {
        let text = get_help_text();
        assert!(!text.is_empty());
        assert!(text.contains("Navigation"));
        assert!(text.contains("Selection"));
    }
}
