//! Key event handlers for the TUI.

mod key_action;

pub use key_action::KeyAction;

use crate::app::{App, AppMode, FocusedPane, PendingAction, SortOption, WhichKeyContext};

/// Handle a key action in the application.
/// Returns true if the app should quit.
pub fn handle_key(app: &mut App, action: KeyAction) -> bool {
    // Clear g-prefix on any key that's not 'g' (in normal mode)
    if matches!(app.mode, AppMode::Normal) && !matches!(action, KeyAction::Char('g')) {
        let was_g_prefix = app.g_prefix;
        app.g_prefix = false;

        // Handle gg command
        if was_g_prefix {
            match action {
                KeyAction::Char('g') => {
                    // This case shouldn't happen due to the outer check, but handle it anyway
                    handle_go_top(app);
                    return false;
                }
                _ => {
                    // g-prefix was set but followed by non-g key - just reset and continue
                }
            }
        }
    }

    match &app.mode {
        AppMode::Normal => handle_normal_mode(app, action),
        AppMode::Search(_) => handle_search_mode(app, action),
        AppMode::Delete | AppMode::DeleteMultiple => handle_delete_mode(app, action),
        AppMode::Help => handle_help_mode(app, action),
        AppMode::Sort => handle_sort_mode(app, action),
        AppMode::WhichKey(_) => handle_which_key_mode(app, action),
        AppMode::FolderSelect => handle_folder_select_mode(app, action),
        AppMode::Compose => handle_compose_mode(app, action),
    }
}

fn handle_normal_mode(app: &mut App, action: KeyAction) -> bool {
    match action {
        KeyAction::Quit => return true,
        KeyAction::Char('q') => return true,

        // Navigation
        KeyAction::Down | KeyAction::Char('j') => handle_down(app),
        KeyAction::Up | KeyAction::Char('k') => handle_up(app),
        KeyAction::Left | KeyAction::Char('h') => handle_left(app),
        KeyAction::Right | KeyAction::Char('l') => handle_right(app),
        KeyAction::Char('g') => {
            if app.g_prefix {
                handle_go_top(app);
                app.g_prefix = false;
            } else {
                app.g_prefix = true;
            }
        }
        KeyAction::Char('G') => handle_go_bottom(app),
        KeyAction::PageDown => handle_page_down(app),
        KeyAction::PageUp => handle_page_up(app),

        // Selection
        KeyAction::ToggleSelect => handle_toggle_select(app),
        KeyAction::SelectAll => handle_select_all(app),
        KeyAction::Char('V') => app.email_selection.deselect_all(),

        // Actions
        KeyAction::Char('d') => app.enter_delete(),
        KeyAction::Char('/') | KeyAction::Char(':') => app.enter_search(),
        KeyAction::Char('s') => app.enter_sort(),
        KeyAction::Char('?') => app.enter_help(),
        KeyAction::Char('r') => app.set_status("Refreshing..."),
        KeyAction::Char('a') => app.enter_compose(),
        KeyAction::Char('S') => app.enter_folder_select(),

        // Which-key menus
        KeyAction::Char('f') => app.enter_which_key(WhichKeyContext::Folder),
        KeyAction::Char('t') => app.enter_which_key(WhichKeyContext::Actions),
        KeyAction::Char('m') => app.enter_which_key(WhichKeyContext::Mark),
        KeyAction::Char('v') => app.enter_which_key(WhichKeyContext::View),

        KeyAction::Select => handle_enter(app),
        KeyAction::Escape => app.clear_status(),

        _ => {}
    }
    false
}

fn handle_search_mode(app: &mut App, action: KeyAction) -> bool {
    match action {
        KeyAction::Escape => app.exit_search(),
        KeyAction::Select => {
            if !app.search_query.is_empty() {
                app.set_status(format!("Searching for: {}", app.search_query));
            }
            app.exit_search();
        }
        KeyAction::CycleSearchMode => app.cycle_search_mode(),
        KeyAction::Backspace => {
            app.search_query.pop();
        }
        KeyAction::Char(c) => {
            app.search_query.push(c);
        }
        _ => {}
    }
    false
}

fn handle_delete_mode(app: &mut App, action: KeyAction) -> bool {
    match action {
        KeyAction::Char('y') | KeyAction::Char('Y') => {
            let indices = app.get_operation_indices();
            let count = indices.len();
            // In a real implementation, we'd delete the emails at these indices
            // For now, we just log which indices would be deleted
            let selected_ids: Vec<&str> = indices
                .iter()
                .filter_map(|&i| app.emails.get(i).map(|e| e.local_id.as_str()))
                .collect();
            log::debug!("Would delete emails: {:?}", selected_ids);
            app.set_status(format!("Deleted {} email(s)", count));
            app.email_selection.deselect_all();
            app.return_to_normal();
        }
        KeyAction::Char('n') | KeyAction::Char('N') | KeyAction::Escape => {
            app.return_to_normal();
        }
        _ => {}
    }
    false
}

fn handle_help_mode(app: &mut App, action: KeyAction) -> bool {
    match action {
        KeyAction::Escape | KeyAction::Char('q') => app.return_to_normal(),
        KeyAction::Down | KeyAction::Char('j') => {
            app.help_scroll = app.help_scroll.saturating_add(1);
        }
        KeyAction::Up | KeyAction::Char('k') => {
            app.help_scroll = app.help_scroll.saturating_sub(1);
        }
        KeyAction::PageDown => {
            app.help_scroll = app.help_scroll.saturating_add(10);
        }
        KeyAction::PageUp => {
            app.help_scroll = app.help_scroll.saturating_sub(10);
        }
        KeyAction::Char('g') => app.help_scroll = 0,
        KeyAction::Char('G') => app.help_scroll = 100, // Scroll to bottom (approximate)
        _ => {}
    }
    false
}

fn handle_sort_mode(app: &mut App, action: KeyAction) -> bool {
    match action {
        KeyAction::Escape => app.return_to_normal(),
        KeyAction::Select => app.apply_sort(),
        KeyAction::Down | KeyAction::Char('j') => {
            if app.sort_selection < SortOption::ALL.len() - 1 {
                app.sort_selection += 1;
            }
        }
        KeyAction::Up | KeyAction::Char('k') => {
            app.sort_selection = app.sort_selection.saturating_sub(1);
        }
        // Number shortcuts
        KeyAction::Char(c) if c.is_ascii_digit() => {
            let idx = c.to_digit(10).unwrap_or(0) as usize;
            if idx > 0 && idx <= SortOption::ALL.len() {
                app.sort_selection = idx - 1;
                app.apply_sort();
            }
        }
        _ => {}
    }
    false
}

fn handle_which_key_mode(app: &mut App, action: KeyAction) -> bool {
    let context = if let AppMode::WhichKey(ctx) = &app.mode {
        ctx.clone()
    } else {
        app.return_to_normal();
        return false;
    };

    match action {
        KeyAction::Escape => app.return_to_normal(),
        KeyAction::Char(c) => {
            // Handle context-specific actions
            match (&context, c) {
                // Mark context actions
                (WhichKeyContext::Mark, 'r') => {
                    app.pending_action = PendingAction::MarkRead;
                    app.set_status("Marking as read...");
                }
                (WhichKeyContext::Mark, 'u') => {
                    app.pending_action = PendingAction::MarkUnread;
                    app.set_status("Marking as unread...");
                }
                // Folder context actions
                (WhichKeyContext::Folder, 'i') => {
                    app.pending_action = PendingAction::LoadFolder("inbox".to_string());
                }
                (WhichKeyContext::Folder, 's') => {
                    app.pending_action = PendingAction::LoadFolder("sent".to_string());
                }
                (WhichKeyContext::Folder, 'd') => {
                    app.pending_action = PendingAction::LoadFolder("drafts".to_string());
                }
                (WhichKeyContext::Folder, 't') => {
                    app.pending_action = PendingAction::LoadFolder("trash".to_string());
                }
                (WhichKeyContext::Folder, 'a') => {
                    app.pending_action = PendingAction::LoadFolder("archive".to_string());
                }
                // Default: just show status
                _ => {
                    let options = context.options();
                    if let Some((_, label)) = options.iter().find(|(k, _)| *k == c) {
                        app.set_status(format!("Selected: {}", label));
                    }
                }
            }
            app.return_to_normal();
        }
        _ => {}
    }
    false
}

fn handle_folder_select_mode(app: &mut App, action: KeyAction) -> bool {
    match action {
        KeyAction::Escape => app.return_to_normal(),
        KeyAction::Select => {
            if let Some(folder) = app.folders.get(app.folder_selection.index) {
                app.current_folder = folder.name.clone();
                app.set_status(format!("Switched to {}", folder.display_name));
            }
            app.return_to_normal();
        }
        KeyAction::Down | KeyAction::Char('j') => {
            app.folder_selection.next(app.folders.len(), 10);
        }
        KeyAction::Up | KeyAction::Char('k') => {
            app.folder_selection.previous();
        }
        _ => {}
    }
    false
}

fn handle_compose_mode(app: &mut App, action: KeyAction) -> bool {
    match action {
        KeyAction::Escape => {
            app.set_status("Compose cancelled");
            app.return_to_normal();
        }
        _ => {}
    }
    false
}

// Helper functions for navigation

fn handle_down(app: &mut App) {
    match app.focused_pane {
        FocusedPane::Left => {
            app.folder_selection.next(app.folders.len(), 10);
        }
        FocusedPane::Middle => {
            app.email_selection.next(app.emails.len(), 20);
        }
        FocusedPane::Right => {
            // Scroll preview if needed
        }
    }
}

fn handle_up(app: &mut App) {
    match app.focused_pane {
        FocusedPane::Left => {
            app.folder_selection.previous();
        }
        FocusedPane::Middle => {
            app.email_selection.previous();
        }
        FocusedPane::Right => {
            // Scroll preview if needed
        }
    }
}

fn handle_left(app: &mut App) {
    match app.focused_pane {
        FocusedPane::Middle => app.focus_left(),
        FocusedPane::Right => app.focus_middle(),
        FocusedPane::Left => {} // Already at leftmost
    }
}

fn handle_right(app: &mut App) {
    match app.focused_pane {
        FocusedPane::Left => app.focus_middle(),
        FocusedPane::Middle => app.focus_right(),
        FocusedPane::Right => {} // Already at rightmost
    }
}

fn handle_go_top(app: &mut App) {
    match app.focused_pane {
        FocusedPane::Left => app.folder_selection.top(),
        FocusedPane::Middle => app.email_selection.top(),
        FocusedPane::Right => {}
    }
}

fn handle_go_bottom(app: &mut App) {
    match app.focused_pane {
        FocusedPane::Left => app.folder_selection.bottom(app.folders.len(), 10),
        FocusedPane::Middle => app.email_selection.bottom(app.emails.len(), 20),
        FocusedPane::Right => {}
    }
}

fn handle_page_down(app: &mut App) {
    match app.focused_pane {
        FocusedPane::Left => app.folder_selection.page_down(app.folders.len(), 10),
        FocusedPane::Middle => app.email_selection.page_down(app.emails.len(), 20),
        FocusedPane::Right => {}
    }
}

fn handle_page_up(app: &mut App) {
    match app.focused_pane {
        FocusedPane::Left => app.folder_selection.page_up(10),
        FocusedPane::Middle => app.email_selection.page_up(20),
        FocusedPane::Right => {}
    }
}

fn handle_toggle_select(app: &mut App) {
    if matches!(app.focused_pane, FocusedPane::Middle) {
        app.email_selection.toggle_and_next(app.emails.len(), 20);
    }
}

fn handle_select_all(app: &mut App) {
    if matches!(app.focused_pane, FocusedPane::Middle) {
        app.email_selection.select_all(app.emails.len());
    }
}

fn handle_enter(app: &mut App) {
    match app.focused_pane {
        FocusedPane::Left => {
            if let Some(folder) = app.folders.get(app.folder_selection.index) {
                let folder_name = folder.name.clone();
                app.pending_action = PendingAction::LoadFolder(folder_name);
                app.focus_middle();
            }
        }
        FocusedPane::Middle => {
            if let Some(email) = app.current_email() {
                let local_id = email.local_id.clone();
                app.pending_action = PendingAction::ViewEmail(local_id);
            }
        }
        FocusedPane::Right => {
            // Already viewing email, nothing to do
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SearchMode;
    use h8_core::types::MessageSync;

    fn create_test_email(id: &str) -> MessageSync {
        MessageSync {
            local_id: id.to_string(),
            remote_id: format!("r{}", id),
            change_key: None,
            folder: "inbox".to_string(),
            subject: Some(format!("Test {}", id)),
            from_addr: Some("test@example.com".to_string()),
            received_at: None,
            is_read: true,
            is_draft: false,
            has_attachments: false,
            synced_at: None,
            local_hash: None,
        }
    }

    // Normal mode tests
    mod normal_mode {
        use super::*;

        #[test]
        fn test_quit_on_q() {
            let mut app = App::new();
            assert!(handle_key(&mut app, KeyAction::Char('q')));
        }

        #[test]
        fn test_quit_on_ctrl_c() {
            let mut app = App::new();
            assert!(handle_key(&mut app, KeyAction::Quit));
        }

        #[test]
        fn test_navigation_down() {
            let mut app = App::new();
            app.emails = vec![create_test_email("1"), create_test_email("2")];

            handle_key(&mut app, KeyAction::Char('j'));
            assert_eq!(app.email_selection.index, 1);
        }

        #[test]
        fn test_navigation_up() {
            let mut app = App::new();
            app.emails = vec![create_test_email("1"), create_test_email("2")];
            app.email_selection.index = 1;

            handle_key(&mut app, KeyAction::Char('k'));
            assert_eq!(app.email_selection.index, 0);
        }

        #[test]
        fn test_navigation_left_right() {
            let mut app = App::new();
            assert_eq!(app.focused_pane, FocusedPane::Middle);

            handle_key(&mut app, KeyAction::Char('h'));
            assert_eq!(app.focused_pane, FocusedPane::Left);

            handle_key(&mut app, KeyAction::Char('l'));
            assert_eq!(app.focused_pane, FocusedPane::Middle);

            handle_key(&mut app, KeyAction::Char('l'));
            assert_eq!(app.focused_pane, FocusedPane::Right);
        }

        #[test]
        fn test_gg_go_top() {
            let mut app = App::new();
            app.emails = (0..10).map(|i| create_test_email(&i.to_string())).collect();
            app.email_selection.index = 5;

            // First g sets prefix
            handle_key(&mut app, KeyAction::Char('g'));
            assert!(app.g_prefix);

            // Second g goes to top
            handle_key(&mut app, KeyAction::Char('g'));
            assert_eq!(app.email_selection.index, 0);
            assert!(!app.g_prefix);
        }

        #[test]
        fn test_shift_g_go_bottom() {
            let mut app = App::new();
            app.emails = (0..10).map(|i| create_test_email(&i.to_string())).collect();

            handle_key(&mut app, KeyAction::Char('G'));
            assert_eq!(app.email_selection.index, 9);
        }

        #[test]
        fn test_enter_search() {
            let mut app = App::new();
            handle_key(&mut app, KeyAction::Char('/'));
            assert!(matches!(app.mode, AppMode::Search(_)));
        }

        #[test]
        fn test_enter_sort() {
            let mut app = App::new();
            handle_key(&mut app, KeyAction::Char('s'));
            assert_eq!(app.mode, AppMode::Sort);
        }

        #[test]
        fn test_enter_help() {
            let mut app = App::new();
            handle_key(&mut app, KeyAction::Char('?'));
            assert_eq!(app.mode, AppMode::Help);
        }

        #[test]
        fn test_toggle_select() {
            let mut app = App::new();
            app.emails = vec![create_test_email("1"), create_test_email("2")];

            handle_key(&mut app, KeyAction::ToggleSelect);
            assert!(app.email_selection.selected_indices.contains(&0));
            assert_eq!(app.email_selection.index, 1);
        }

        #[test]
        fn test_select_all() {
            let mut app = App::new();
            app.emails = vec![create_test_email("1"), create_test_email("2")];

            handle_key(&mut app, KeyAction::SelectAll);
            assert_eq!(app.email_selection.selected_indices.len(), 2);
        }

        #[test]
        fn test_deselect_all() {
            let mut app = App::new();
            app.emails = vec![create_test_email("1"), create_test_email("2")];
            app.email_selection.select_all(2);

            handle_key(&mut app, KeyAction::Char('V'));
            assert!(!app.email_selection.has_selections());
        }

        #[test]
        fn test_which_key_folder() {
            let mut app = App::new();
            handle_key(&mut app, KeyAction::Char('f'));
            assert!(matches!(
                app.mode,
                AppMode::WhichKey(WhichKeyContext::Folder)
            ));
        }

        #[test]
        fn test_g_prefix_resets_on_non_g_key() {
            let mut app = App::new();
            app.g_prefix = true;

            handle_key(&mut app, KeyAction::Char('j'));
            assert!(!app.g_prefix);
        }
    }

    // Search mode tests
    mod search_mode {
        use super::*;

        #[test]
        fn test_escape_exits_search() {
            let mut app = App::new();
            app.enter_search();

            handle_key(&mut app, KeyAction::Escape);
            assert_eq!(app.mode, AppMode::Normal);
        }

        #[test]
        fn test_typing_in_search() {
            let mut app = App::new();
            app.enter_search();

            handle_key(&mut app, KeyAction::Char('h'));
            handle_key(&mut app, KeyAction::Char('e'));
            handle_key(&mut app, KeyAction::Char('l'));
            handle_key(&mut app, KeyAction::Char('l'));
            handle_key(&mut app, KeyAction::Char('o'));

            assert_eq!(app.search_query, "hello");
        }

        #[test]
        fn test_backspace_in_search() {
            let mut app = App::new();
            app.enter_search();
            app.search_query = "hello".to_string();

            handle_key(&mut app, KeyAction::Backspace);
            assert_eq!(app.search_query, "hell");
        }

        #[test]
        fn test_cycle_search_mode() {
            let mut app = App::new();
            app.enter_search();

            handle_key(&mut app, KeyAction::CycleSearchMode);
            assert_eq!(app.search_mode, SearchMode::From);
        }

        #[test]
        fn test_enter_submits_search() {
            let mut app = App::new();
            app.enter_search();
            app.search_query = "test".to_string();

            handle_key(&mut app, KeyAction::Select);
            assert_eq!(app.mode, AppMode::Normal);
            assert!(app.status_message.is_some());
        }
    }

    // Delete mode tests
    mod delete_mode {
        use super::*;

        #[test]
        fn test_confirm_delete() {
            let mut app = App::new();
            app.emails = vec![create_test_email("1")];
            app.enter_delete();

            handle_key(&mut app, KeyAction::Char('y'));
            assert_eq!(app.mode, AppMode::Normal);
            assert!(app.status_message.is_some());
        }

        #[test]
        fn test_cancel_delete() {
            let mut app = App::new();
            app.emails = vec![create_test_email("1")];
            app.enter_delete();

            handle_key(&mut app, KeyAction::Char('n'));
            assert_eq!(app.mode, AppMode::Normal);
        }

        #[test]
        fn test_escape_cancels_delete() {
            let mut app = App::new();
            app.emails = vec![create_test_email("1")];
            app.enter_delete();

            handle_key(&mut app, KeyAction::Escape);
            assert_eq!(app.mode, AppMode::Normal);
        }
    }

    // Help mode tests
    mod help_mode {
        use super::*;

        #[test]
        fn test_escape_exits_help() {
            let mut app = App::new();
            app.enter_help();

            handle_key(&mut app, KeyAction::Escape);
            assert_eq!(app.mode, AppMode::Normal);
        }

        #[test]
        fn test_scroll_down_in_help() {
            let mut app = App::new();
            app.enter_help();

            handle_key(&mut app, KeyAction::Char('j'));
            assert_eq!(app.help_scroll, 1);
        }

        #[test]
        fn test_scroll_up_in_help() {
            let mut app = App::new();
            app.enter_help();
            app.help_scroll = 5;

            handle_key(&mut app, KeyAction::Char('k'));
            assert_eq!(app.help_scroll, 4);
        }

        #[test]
        fn test_go_top_in_help() {
            let mut app = App::new();
            app.enter_help();
            app.help_scroll = 50;

            handle_key(&mut app, KeyAction::Char('g'));
            assert_eq!(app.help_scroll, 0);
        }
    }

    // Sort mode tests
    mod sort_mode {
        use super::*;

        #[test]
        fn test_escape_exits_sort() {
            let mut app = App::new();
            app.enter_sort();

            handle_key(&mut app, KeyAction::Escape);
            assert_eq!(app.mode, AppMode::Normal);
        }

        #[test]
        fn test_navigate_sort_options() {
            let mut app = App::new();
            app.enter_sort();

            handle_key(&mut app, KeyAction::Char('j'));
            assert_eq!(app.sort_selection, 1);

            handle_key(&mut app, KeyAction::Char('k'));
            assert_eq!(app.sort_selection, 0);
        }

        #[test]
        fn test_apply_sort() {
            let mut app = App::new();
            app.enter_sort();
            app.sort_selection = 2;

            handle_key(&mut app, KeyAction::Select);
            assert_eq!(app.sort_option, SortOption::SubjectAsc);
            assert_eq!(app.mode, AppMode::Normal);
        }

        #[test]
        fn test_number_shortcut_sort() {
            let mut app = App::new();
            app.enter_sort();

            handle_key(&mut app, KeyAction::Char('2'));
            assert_eq!(app.sort_option, SortOption::DateAsc);
            assert_eq!(app.mode, AppMode::Normal);
        }
    }

    // Which-key mode tests
    mod which_key_mode {
        use super::*;

        #[test]
        fn test_escape_exits_which_key() {
            let mut app = App::new();
            app.enter_which_key(WhichKeyContext::Folder);

            handle_key(&mut app, KeyAction::Escape);
            assert_eq!(app.mode, AppMode::Normal);
        }

        #[test]
        fn test_select_which_key_option() {
            let mut app = App::new();
            app.enter_which_key(WhichKeyContext::Folder);

            handle_key(&mut app, KeyAction::Char('i'));
            assert_eq!(app.mode, AppMode::Normal);
            // Folder selection triggers LoadFolder pending action
            assert_eq!(
                app.pending_action,
                PendingAction::LoadFolder("inbox".to_string())
            );
        }

        #[test]
        fn test_which_key_mark_read() {
            let mut app = App::new();
            app.enter_which_key(WhichKeyContext::Mark);

            handle_key(&mut app, KeyAction::Char('r'));
            assert_eq!(app.mode, AppMode::Normal);
            assert_eq!(app.pending_action, PendingAction::MarkRead);
        }

        #[test]
        fn test_which_key_mark_unread() {
            let mut app = App::new();
            app.enter_which_key(WhichKeyContext::Mark);

            handle_key(&mut app, KeyAction::Char('u'));
            assert_eq!(app.mode, AppMode::Normal);
            assert_eq!(app.pending_action, PendingAction::MarkUnread);
        }
    }

    // Folder select mode tests
    mod folder_select_mode {
        use super::*;

        #[test]
        fn test_navigate_folders() {
            let mut app = App::new();
            app.mode = AppMode::FolderSelect;

            handle_key(&mut app, KeyAction::Char('j'));
            assert_eq!(app.folder_selection.index, 1);
        }

        #[test]
        fn test_select_folder() {
            let mut app = App::new();
            app.mode = AppMode::FolderSelect;
            app.folder_selection.index = 1; // Sent folder

            handle_key(&mut app, KeyAction::Select);
            assert_eq!(app.current_folder, "sent");
            assert_eq!(app.mode, AppMode::Normal);
        }
    }

    // Pane-specific navigation tests
    mod pane_navigation {
        use super::*;

        #[test]
        fn test_left_pane_navigation() {
            let mut app = App::new();
            app.focus_left();

            handle_key(&mut app, KeyAction::Char('j'));
            assert_eq!(app.folder_selection.index, 1);

            handle_key(&mut app, KeyAction::Char('k'));
            assert_eq!(app.folder_selection.index, 0);
        }

        #[test]
        fn test_enter_on_folder() {
            let mut app = App::new();
            app.focus_left();
            app.folder_selection.index = 1; // Sent folder

            handle_key(&mut app, KeyAction::Select);
            // Now sets pending action instead of directly changing folder
            assert_eq!(app.pending_action, PendingAction::LoadFolder("sent".to_string()));
            assert_eq!(app.focused_pane, FocusedPane::Middle);
        }

        #[test]
        fn test_enter_on_email() {
            let mut app = App::new();
            app.emails = vec![create_test_email("1")];

            handle_key(&mut app, KeyAction::Select);
            // Now sets pending action instead of directly focusing right
            assert_eq!(app.pending_action, PendingAction::ViewEmail("1".to_string()));
        }
    }
}
