//! h8-tui: TUI email client for Exchange Web Services.
//!
//! A terminal-based email client with vim-style navigation,
//! three-pane layout, and modal interface.

mod app;
mod data;
mod handlers;
mod ui;

use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use app::App;
use data::DataSource;
use handlers::{handle_key, KeyAction};

/// Event polling timeout in milliseconds.
const POLL_TIMEOUT_MS: u64 = 100;

/// Maximum number of emails to load per folder.
const EMAIL_LIMIT: usize = 500;

fn main() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state and data source
    let mut app = App::new();
    let mut data_source = DataSource::default();

    // Try to load real data, fall back to demo data if not available
    if let Err(e) = load_real_data(&mut app, &mut data_source) {
        log::warn!("Failed to load real data: {}, using demo data", e);
        load_demo_data(&mut app);
    }

    // Run the main loop
    let result = run_app(&mut terminal, &mut app, &mut data_source);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    // Handle any errors from the main loop
    if let Err(err) = result {
        eprintln!("Error: {err}");
        return Err(err);
    }

    Ok(())
}

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    data_source: &mut DataSource,
) -> Result<()> {
    loop {
        // Draw the UI
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Process any pending actions
        process_pending_action(app, data_source);

        // Poll for events
        if event::poll(Duration::from_millis(POLL_TIMEOUT_MS))? {
            if let Event::Key(key_event) = event::read()? {
                let action = KeyAction::from(key_event);
                if handle_key_with_data(app, data_source, action) {
                    break;
                }
            }
        }

        // Check if we should quit
        if app.should_quit {
            break;
        }
    }

    Ok(())
}

/// Handle a key action with data source integration.
fn handle_key_with_data(app: &mut App, data_source: &mut DataSource, action: KeyAction) -> bool {
    // Check for refresh action
    if matches!(app.mode, app::AppMode::Normal) && matches!(action, KeyAction::Char('r')) {
        refresh_data(app, data_source);
        return false;
    }

    // Check for search completion
    if matches!(app.mode, app::AppMode::Search(_)) && matches!(action, KeyAction::Select) {
        if !app.search_query.is_empty() {
            execute_search(app, data_source);
        }
        app.exit_search();
        return false;
    }

    // Check for delete confirmation
    if matches!(app.mode, app::AppMode::Delete | app::AppMode::DeleteMultiple)
        && matches!(action, KeyAction::Char('y') | KeyAction::Char('Y'))
    {
        execute_delete(app, data_source);
        return false;
    }

    // Default handling
    handle_key(app, action)
}

/// Process any pending actions that need data source access.
fn process_pending_action(app: &mut App, data_source: &mut DataSource) {
    use app::PendingAction;

    let action = std::mem::take(&mut app.pending_action);
    match action {
        PendingAction::None => {}
        PendingAction::MarkRead => {
            execute_mark_read(app, data_source);
        }
        PendingAction::MarkUnread => {
            execute_mark_unread(app, data_source);
        }
        PendingAction::LoadFolder(folder) => {
            load_folder(app, data_source, &folder);
        }
        PendingAction::ViewEmail(id) => {
            view_email(app, data_source, &id);
        }
    }
}

/// Mark selected emails as read.
fn execute_mark_read(app: &mut App, data_source: &mut DataSource) {
    let indices = app.get_operation_indices();
    let ids: Vec<String> = indices
        .iter()
        .filter_map(|&i| app.emails.get(i).map(|e| e.local_id.clone()))
        .collect();

    if ids.is_empty() {
        return;
    }

    let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();

    match data_source.mark_read(&app.current_folder, &id_refs) {
        Ok(count) => {
            // Update local state
            for id in &ids {
                if let Some(email) = app.emails.iter_mut().find(|e| &e.local_id == id) {
                    email.is_read = true;
                }
            }
            app.email_selection.deselect_all();
            app.set_status(format!("Marked {} email(s) as read", count));
        }
        Err(e) => {
            app.set_status(format!("Failed to mark as read: {}", e));
        }
    }
}

/// Mark selected emails as unread.
fn execute_mark_unread(app: &mut App, data_source: &mut DataSource) {
    let indices = app.get_operation_indices();
    let ids: Vec<String> = indices
        .iter()
        .filter_map(|&i| app.emails.get(i).map(|e| e.local_id.clone()))
        .collect();

    if ids.is_empty() {
        return;
    }

    let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();

    match data_source.mark_unread(&app.current_folder, &id_refs) {
        Ok(count) => {
            // Update local state
            for id in &ids {
                if let Some(email) = app.emails.iter_mut().find(|e| &e.local_id == id) {
                    email.is_read = false;
                }
            }
            app.email_selection.deselect_all();
            app.set_status(format!("Marked {} email(s) as unread", count));
        }
        Err(e) => {
            app.set_status(format!("Failed to mark as unread: {}", e));
        }
    }
}

/// Load emails from a specific folder.
fn load_folder(app: &mut App, data_source: &mut DataSource, folder: &str) {
    app.current_folder = folder.to_string();
    match data_source.load_emails(folder, EMAIL_LIMIT) {
        Ok(emails) => {
            app.emails = emails;
            app.email_selection.reset();
            let display_name = app.current_folder_display().to_string();
            app.set_status(format!("Loaded {} - {} emails", display_name, app.emails.len()));
        }
        Err(e) => {
            app.set_status(format!("Failed to load {}: {}", folder, e));
        }
    }
}

/// View an email (load full content).
fn view_email(app: &mut App, data_source: &mut DataSource, local_id: &str) {
    match data_source.get_email_content(&app.current_folder, local_id) {
        Ok(Some(content)) => {
            // For now, just show that we loaded it
            let lines = content.lines().count();
            app.set_status(format!("Loaded email ({} lines)", lines));
            app.focus_right();
        }
        Ok(None) => {
            app.set_status("Email not found in maildir");
        }
        Err(e) => {
            app.set_status(format!("Failed to load email: {}", e));
        }
    }
}

/// Try to load real data from h8-core.
fn load_real_data(app: &mut App, data_source: &mut DataSource) -> data::Result<()> {
    // Try to detect accounts
    let accounts = data_source.detect_accounts()?;
    if accounts.is_empty() {
        return Err(data::DataError::NoAccount);
    }

    // Use the first available account
    let account = &accounts[0];
    data_source.set_account(account)?;
    app.set_status(format!("Account: {}", account));

    // Load folders
    match data_source.load_folders() {
        Ok(folders) if !folders.is_empty() => {
            app.folders = folders;
        }
        Ok(_) => {
            // No folders found, keep defaults
        }
        Err(e) => {
            app.set_status(format!("Failed to load folders: {}", e));
        }
    }

    // Load emails for current folder
    match data_source.load_emails(&app.current_folder, EMAIL_LIMIT) {
        Ok(emails) => {
            app.emails = emails;
            app.email_selection.reset();
        }
        Err(e) => {
            app.set_status(format!("Failed to load emails: {}", e));
        }
    }

    Ok(())
}

/// Refresh data from data source.
fn refresh_data(app: &mut App, data_source: &mut DataSource) {
    app.set_status("Refreshing...");

    // Reload folders
    if let Ok(folders) = data_source.load_folders() {
        if !folders.is_empty() {
            app.folders = folders;
        }
    }

    // Reload emails
    match data_source.load_emails(&app.current_folder, EMAIL_LIMIT) {
        Ok(emails) => {
            app.emails = emails;
            app.email_selection.reset();
            app.set_status(format!("Loaded {} emails", app.emails.len()));
        }
        Err(e) => {
            app.set_status(format!("Refresh failed: {}", e));
        }
    }
}

/// Execute search with current query.
fn execute_search(app: &mut App, data_source: &mut DataSource) {
    let search_mode = match &app.search_mode {
        app::SearchMode::Subject => data::SearchMode::Subject,
        app::SearchMode::From => data::SearchMode::From,
        _ => data::SearchMode::All,
    };

    match data_source.search_emails(&app.current_folder, &app.search_query, search_mode, EMAIL_LIMIT)
    {
        Ok(emails) => {
            let count = emails.len();
            app.emails = emails;
            app.email_selection.reset();
            app.set_status(format!("Found {} emails matching '{}'", count, app.search_query));
        }
        Err(e) => {
            app.set_status(format!("Search failed: {}", e));
        }
    }
}

/// Execute delete operation.
fn execute_delete(app: &mut App, data_source: &mut DataSource) {
    let indices = app.get_operation_indices();
    
    // Collect IDs as owned strings to avoid borrow issues
    let ids: Vec<String> = indices
        .iter()
        .filter_map(|&i| app.emails.get(i).map(|e| e.local_id.clone()))
        .collect();

    if ids.is_empty() {
        app.return_to_normal();
        return;
    }

    // Convert to &str for the API call
    let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();

    // Move to trash instead of permanent delete
    match data_source.trash_emails(&app.current_folder, &id_refs) {
        Ok(count) => {
            app.set_status(format!("Moved {} email(s) to trash", count));
            // Remove from current view
            let id_set: std::collections::HashSet<&str> = id_refs.into_iter().collect();
            app.emails.retain(|e| !id_set.contains(e.local_id.as_str()));
            app.email_selection.deselect_all();
            // Adjust cursor if needed
            if app.email_selection.index >= app.emails.len() && !app.emails.is_empty() {
                app.email_selection.index = app.emails.len() - 1;
            }
        }
        Err(e) => {
            app.set_status(format!("Delete failed: {}", e));
        }
    }

    app.return_to_normal();
}

/// Load demo data for testing the TUI when no real data is available.
fn load_demo_data(app: &mut App) {
    use h8_core::types::MessageSync;

    // Update folder counts
    if let Some(inbox) = app.folders.iter_mut().find(|f| f.name == "inbox") {
        inbox.unread_count = 3;
        inbox.total_count = 10;
    }
    if let Some(sent) = app.folders.iter_mut().find(|f| f.name == "sent") {
        sent.total_count = 25;
    }

    // Add some demo emails
    app.emails = vec![
        MessageSync {
            local_id: "h8-1".to_string(),
            remote_id: "AAMkAGQ...1".to_string(),
            change_key: None,
            folder: "inbox".to_string(),
            subject: Some("Weekly Team Update - Q4 Planning".to_string()),
            from_addr: Some("manager@company.com".to_string()),
            received_at: Some("2024-12-09 09:30:00".to_string()),
            is_read: false,
            is_draft: false,
            has_attachments: true,
            synced_at: None,
            local_hash: None,
        },
        MessageSync {
            local_id: "h8-2".to_string(),
            remote_id: "AAMkAGQ...2".to_string(),
            change_key: None,
            folder: "inbox".to_string(),
            subject: Some("Re: Code Review Request".to_string()),
            from_addr: Some("colleague@company.com".to_string()),
            received_at: Some("2024-12-09 08:45:00".to_string()),
            is_read: false,
            is_draft: false,
            has_attachments: false,
            synced_at: None,
            local_hash: None,
        },
        MessageSync {
            local_id: "h8-3".to_string(),
            remote_id: "AAMkAGQ...3".to_string(),
            change_key: None,
            folder: "inbox".to_string(),
            subject: Some("Build Failed: main branch".to_string()),
            from_addr: Some("ci@github.com".to_string()),
            received_at: Some("2024-12-09 07:15:00".to_string()),
            is_read: false,
            is_draft: false,
            has_attachments: false,
            synced_at: None,
            local_hash: None,
        },
        MessageSync {
            local_id: "h8-4".to_string(),
            remote_id: "AAMkAGQ...4".to_string(),
            change_key: None,
            folder: "inbox".to_string(),
            subject: Some("Meeting Invite: Architecture Review".to_string()),
            from_addr: Some("calendar@company.com".to_string()),
            received_at: Some("2024-12-08 16:00:00".to_string()),
            is_read: true,
            is_draft: false,
            has_attachments: true,
            synced_at: None,
            local_hash: None,
        },
        MessageSync {
            local_id: "h8-5".to_string(),
            remote_id: "AAMkAGQ...5".to_string(),
            change_key: None,
            folder: "inbox".to_string(),
            subject: Some("Your weekly digest".to_string()),
            from_addr: Some("newsletter@techweekly.com".to_string()),
            received_at: Some("2024-12-08 10:00:00".to_string()),
            is_read: true,
            is_draft: false,
            has_attachments: false,
            synced_at: None,
            local_hash: None,
        },
        MessageSync {
            local_id: "h8-6".to_string(),
            remote_id: "AAMkAGQ...6".to_string(),
            change_key: None,
            folder: "inbox".to_string(),
            subject: Some("Re: Project Deadline Extension".to_string()),
            from_addr: Some("pm@company.com".to_string()),
            received_at: Some("2024-12-07 14:30:00".to_string()),
            is_read: true,
            is_draft: false,
            has_attachments: true,
            synced_at: None,
            local_hash: None,
        },
        MessageSync {
            local_id: "h8-7".to_string(),
            remote_id: "AAMkAGQ...7".to_string(),
            change_key: None,
            folder: "inbox".to_string(),
            subject: Some("Holiday Schedule 2024".to_string()),
            from_addr: Some("hr@company.com".to_string()),
            received_at: Some("2024-12-06 09:00:00".to_string()),
            is_read: true,
            is_draft: false,
            has_attachments: true,
            synced_at: None,
            local_hash: None,
        },
        MessageSync {
            local_id: "h8-8".to_string(),
            remote_id: "AAMkAGQ...8".to_string(),
            change_key: None,
            folder: "inbox".to_string(),
            subject: Some("Security Alert: New login detected".to_string()),
            from_addr: Some("security@company.com".to_string()),
            received_at: Some("2024-12-05 22:15:00".to_string()),
            is_read: true,
            is_draft: false,
            has_attachments: false,
            synced_at: None,
            local_hash: None,
        },
        MessageSync {
            local_id: "h8-9".to_string(),
            remote_id: "AAMkAGQ...9".to_string(),
            change_key: None,
            folder: "inbox".to_string(),
            subject: Some("Expense Report Approved".to_string()),
            from_addr: Some("finance@company.com".to_string()),
            received_at: Some("2024-12-05 11:00:00".to_string()),
            is_read: true,
            is_draft: false,
            has_attachments: true,
            synced_at: None,
            local_hash: None,
        },
        MessageSync {
            local_id: "h8-10".to_string(),
            remote_id: "AAMkAGQ...10".to_string(),
            change_key: None,
            folder: "inbox".to_string(),
            subject: Some("Welcome to the team!".to_string()),
            from_addr: Some("onboarding@company.com".to_string()),
            received_at: Some("2024-12-01 09:00:00".to_string()),
            is_read: true,
            is_draft: false,
            has_attachments: false,
            synced_at: None,
            local_hash: None,
        },
    ];

    app.set_status("Demo mode - no account configured");
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn test_load_demo_data() {
        let mut app = App::new();
        load_demo_data(&mut app);

        assert!(!app.emails.is_empty());
        assert_eq!(app.emails.len(), 10);

        // Check folder counts
        let inbox = app.folders.iter().find(|f| f.name == "inbox").unwrap();
        assert_eq!(inbox.unread_count, 3);
    }

    #[test]
    fn test_app_renders_without_panic() {
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut app = App::new();
        load_demo_data(&mut app);

        terminal
            .draw(|frame| ui::draw(frame, &app))
            .expect("Should render without panic");
    }

    #[test]
    fn test_app_handles_key_events() {
        let mut app = App::new();
        load_demo_data(&mut app);

        // Test navigation
        assert_eq!(app.email_selection.index, 0);
        handle_key(&mut app, KeyAction::Down);
        assert_eq!(app.email_selection.index, 1);

        // Test mode switching
        handle_key(&mut app, KeyAction::Char('?'));
        assert!(matches!(app.mode, app::AppMode::Help));

        handle_key(&mut app, KeyAction::Escape);
        assert!(matches!(app.mode, app::AppMode::Normal));
    }

    #[test]
    fn test_quit_returns_true() {
        let mut app = App::new();
        assert!(handle_key(&mut app, KeyAction::Quit));
        assert!(handle_key(&mut app, KeyAction::Char('q')));
    }

    #[test]
    fn test_data_source_integration() {
        let mut app = App::new();
        let mut data_source = DataSource::default();

        // This should fail gracefully without an account
        let result = load_real_data(&mut app, &mut data_source);
        // Either works or fails with NoAccount - both are valid
        if result.is_err() {
            // Verify demo data can be loaded instead
            load_demo_data(&mut app);
            assert!(!app.emails.is_empty());
        }
    }
}
