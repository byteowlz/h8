//! Application state and mode management for the TUI.

use std::collections::HashSet;

use h8_core::types::MessageSync;

/// Application modes for the modal TUI system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    /// Default browsing mode.
    Normal,
    /// Text input for searching.
    Search(SearchMode),
    /// Confirmation for single item deletion.
    Delete,
    /// Confirmation for batch deletion.
    DeleteMultiple,
    /// Scrollable help overlay.
    Help,
    /// Sort option selection menu.
    Sort,
    /// Command discovery submenu.
    WhichKey(WhichKeyContext),
    /// Folder selection.
    FolderSelect,
    /// Compose new email.
    Compose,
}

/// Search modes for filtering emails.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SearchMode {
    #[default]
    Subject,
    From,
    To,
    Body,
    All,
}

impl SearchMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SearchMode::Subject => "Subject",
            SearchMode::From => "From",
            SearchMode::To => "To",
            SearchMode::Body => "Body",
            SearchMode::All => "All",
        }
    }

    pub fn cycle(&self) -> Self {
        match self {
            SearchMode::Subject => SearchMode::From,
            SearchMode::From => SearchMode::To,
            SearchMode::To => SearchMode::Body,
            SearchMode::Body => SearchMode::All,
            SearchMode::All => SearchMode::Subject,
        }
    }
}

/// WhichKey contexts for command discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhichKeyContext {
    /// Folder operations.
    Folder,
    /// Email actions.
    Actions,
    /// Mark operations.
    Mark,
    /// View options.
    View,
}

impl WhichKeyContext {
    pub fn title(&self) -> &'static str {
        match self {
            WhichKeyContext::Folder => "Folder",
            WhichKeyContext::Actions => "Actions",
            WhichKeyContext::Mark => "Mark",
            WhichKeyContext::View => "View",
        }
    }

    pub fn options(&self) -> Vec<(char, &'static str)> {
        match self {
            WhichKeyContext::Folder => vec![
                ('i', "Inbox"),
                ('s', "Sent"),
                ('d', "Drafts"),
                ('t', "Trash"),
                ('a', "Archive"),
            ],
            WhichKeyContext::Actions => vec![
                ('r', "Reply"),
                ('R', "Reply All"),
                ('f', "Forward"),
                ('d', "Delete"),
                ('m', "Move"),
            ],
            WhichKeyContext::Mark => vec![
                ('r', "Read"),
                ('u', "Unread"),
                ('f', "Flag"),
                ('F', "Unflag"),
            ],
            WhichKeyContext::View => vec![
                ('t', "Toggle Preview"),
                ('w', "Wide View"),
                ('n', "Narrow View"),
            ],
        }
    }
}

/// Sort options for the email list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOption {
    #[default]
    DateDesc,
    DateAsc,
    SubjectAsc,
    SubjectDesc,
    FromAsc,
    FromDesc,
}

impl SortOption {
    pub const ALL: [SortOption; 6] = [
        SortOption::DateDesc,
        SortOption::DateAsc,
        SortOption::SubjectAsc,
        SortOption::SubjectDesc,
        SortOption::FromAsc,
        SortOption::FromDesc,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            SortOption::DateDesc => "Date (Newest First)",
            SortOption::DateAsc => "Date (Oldest First)",
            SortOption::SubjectAsc => "Subject (A-Z)",
            SortOption::SubjectDesc => "Subject (Z-A)",
            SortOption::FromAsc => "From (A-Z)",
            SortOption::FromDesc => "From (Z-A)",
        }
    }
}

/// Which pane is currently focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusedPane {
    /// Left pane (folders/filters).
    Left,
    /// Middle pane (email list).
    #[default]
    Middle,
    /// Right pane (preview).
    Right,
}

/// Selection state for a list with multi-selection support.
#[derive(Debug, Clone)]
pub struct Selection {
    /// Current cursor position.
    pub index: usize,
    /// Viewport scroll offset.
    pub offset: usize,
    /// Multi-selected item indices.
    pub selected_indices: HashSet<usize>,
}

impl Default for Selection {
    fn default() -> Self {
        Self::new()
    }
}

impl Selection {
    pub fn new() -> Self {
        Self {
            index: 0,
            offset: 0,
            selected_indices: HashSet::new(),
        }
    }

    /// Move cursor down, adjusting offset if necessary.
    pub fn next(&mut self, max: usize, page_size: usize) {
        if max == 0 {
            return;
        }
        if self.index < max.saturating_sub(1) {
            self.index += 1;
            // Adjust offset to keep cursor visible
            if self.index >= self.offset + page_size {
                self.offset = self.index.saturating_sub(page_size - 1);
            }
        }
    }

    /// Move cursor up, adjusting offset if necessary.
    pub fn previous(&mut self) {
        if self.index > 0 {
            self.index -= 1;
            if self.index < self.offset {
                self.offset = self.index;
            }
        }
    }

    /// Jump to the top of the list.
    pub fn top(&mut self) {
        self.index = 0;
        self.offset = 0;
    }

    /// Jump to the bottom of the list.
    pub fn bottom(&mut self, max: usize, page_size: usize) {
        if max == 0 {
            return;
        }
        self.index = max.saturating_sub(1);
        self.offset = self.index.saturating_sub(page_size.saturating_sub(1));
    }

    /// Move down by page size.
    pub fn page_down(&mut self, max: usize, page_size: usize) {
        if max == 0 {
            return;
        }
        let new_index = (self.index + page_size).min(max.saturating_sub(1));
        self.index = new_index;
        if self.index >= self.offset + page_size {
            self.offset = self.index.saturating_sub(page_size - 1);
        }
    }

    /// Move up by page size.
    pub fn page_up(&mut self, page_size: usize) {
        self.index = self.index.saturating_sub(page_size);
        if self.index < self.offset {
            self.offset = self.index;
        }
    }

    /// Toggle selection on current item.
    pub fn toggle_selection(&mut self) {
        if self.selected_indices.contains(&self.index) {
            self.selected_indices.remove(&self.index);
        } else {
            self.selected_indices.insert(self.index);
        }
    }

    /// Toggle selection and move to next item.
    pub fn toggle_and_next(&mut self, max: usize, page_size: usize) {
        self.toggle_selection();
        self.next(max, page_size);
    }

    /// Select all items.
    pub fn select_all(&mut self, max: usize) {
        self.selected_indices = (0..max).collect();
    }

    /// Deselect all items.
    pub fn deselect_all(&mut self) {
        self.selected_indices.clear();
    }

    /// Check if any items are selected.
    pub fn has_selections(&self) -> bool {
        !self.selected_indices.is_empty()
    }

    /// Get sorted list of selected indices.
    pub fn get_selected_indices(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = self.selected_indices.iter().copied().collect();
        indices.sort_unstable();
        indices
    }

    /// Get indices to operate on (selected or current).
    pub fn get_operation_indices(&self) -> Vec<usize> {
        if self.has_selections() {
            self.get_selected_indices()
        } else {
            vec![self.index]
        }
    }

    /// Reset selection state.
    pub fn reset(&mut self) {
        self.index = 0;
        self.offset = 0;
        self.selected_indices.clear();
    }
}

/// Folder info for the left pane.
#[derive(Debug, Clone)]
pub struct FolderInfo {
    pub name: String,
    pub display_name: String,
    pub unread_count: usize,
    pub total_count: usize,
}

impl FolderInfo {
    pub fn new(name: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            display_name: display_name.into(),
            unread_count: 0,
            total_count: 0,
        }
    }
}

/// Pending data operation to be executed by main loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingAction {
    /// No pending action.
    None,
    /// Mark selected emails as read.
    MarkRead,
    /// Mark selected emails as unread.
    MarkUnread,
    /// Load emails for a specific folder.
    LoadFolder(String),
    /// Open email in viewer.
    ViewEmail(String),
}

impl Default for PendingAction {
    fn default() -> Self {
        PendingAction::None
    }
}

/// Application state.
pub struct App {
    /// Current mode.
    pub mode: AppMode,
    /// Currently focused pane.
    pub focused_pane: FocusedPane,
    /// Whether the app should quit.
    pub should_quit: bool,

    /// Folder list.
    pub folders: Vec<FolderInfo>,
    /// Folder selection state.
    pub folder_selection: Selection,
    /// Current folder name.
    pub current_folder: String,

    /// Email list.
    pub emails: Vec<MessageSync>,
    /// Email selection state.
    pub email_selection: Selection,

    /// Search query buffer.
    pub search_query: String,
    /// Current search mode.
    pub search_mode: SearchMode,

    /// Current sort option.
    pub sort_option: SortOption,
    /// Sort menu selection index.
    pub sort_selection: usize,

    /// Help scroll offset.
    pub help_scroll: usize,

    /// Status message (ephemeral).
    pub status_message: Option<String>,

    /// G-prefix state for vim gg command.
    pub g_prefix: bool,

    /// Pending action to be executed by main loop.
    pub pending_action: PendingAction,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let default_folders = vec![
            FolderInfo::new("inbox", "Inbox"),
            FolderInfo::new("sent", "Sent"),
            FolderInfo::new("drafts", "Drafts"),
            FolderInfo::new("trash", "Trash"),
            FolderInfo::new("archive", "Archive"),
        ];

        Self {
            mode: AppMode::Normal,
            focused_pane: FocusedPane::Middle,
            should_quit: false,
            folders: default_folders,
            folder_selection: Selection::new(),
            current_folder: "inbox".to_string(),
            emails: Vec::new(),
            email_selection: Selection::new(),
            search_query: String::new(),
            search_mode: SearchMode::default(),
            sort_option: SortOption::default(),
            sort_selection: 0,
            help_scroll: 0,
            status_message: None,
            g_prefix: false,
            pending_action: PendingAction::None,
        }
    }

    /// Set a status message.
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status_message = Some(message.into());
    }

    /// Clear the status message.
    pub fn clear_status(&mut self) {
        self.status_message = None;
    }

    /// Get the current folder's display name.
    pub fn current_folder_display(&self) -> &str {
        self.folders
            .iter()
            .find(|f| f.name == self.current_folder)
            .map(|f| f.display_name.as_str())
            .unwrap_or(&self.current_folder)
    }

    /// Get count of selected emails.
    pub fn selected_count(&self) -> usize {
        if self.email_selection.has_selections() {
            self.email_selection.selected_indices.len()
        } else if !self.emails.is_empty() {
            1
        } else {
            0
        }
    }

    /// Enter search mode.
    pub fn enter_search(&mut self) {
        self.mode = AppMode::Search(SearchMode::default());
        self.search_query.clear();
    }

    /// Exit search mode.
    pub fn exit_search(&mut self) {
        self.mode = AppMode::Normal;
    }

    /// Enter sort mode.
    pub fn enter_sort(&mut self) {
        self.mode = AppMode::Sort;
        self.sort_selection = SortOption::ALL
            .iter()
            .position(|&o| o == self.sort_option)
            .unwrap_or(0);
    }

    /// Apply selected sort option and exit sort mode.
    pub fn apply_sort(&mut self) {
        if let Some(&option) = SortOption::ALL.get(self.sort_selection) {
            self.sort_option = option;
            self.set_status(format!("Sorted by: {}", option.as_str()));
        }
        self.mode = AppMode::Normal;
    }

    /// Enter help mode.
    pub fn enter_help(&mut self) {
        self.mode = AppMode::Help;
        self.help_scroll = 0;
    }

    /// Enter delete confirmation mode.
    pub fn enter_delete(&mut self) {
        if self.emails.is_empty() {
            return;
        }
        if self.email_selection.has_selections() {
            self.mode = AppMode::DeleteMultiple;
        } else {
            self.mode = AppMode::Delete;
        }
    }

    /// Enter which-key mode.
    pub fn enter_which_key(&mut self, context: WhichKeyContext) {
        self.mode = AppMode::WhichKey(context);
    }

    /// Return to normal mode.
    pub fn return_to_normal(&mut self) {
        self.mode = AppMode::Normal;
        self.g_prefix = false;
    }

    /// Switch focus to the left pane.
    pub fn focus_left(&mut self) {
        self.focused_pane = FocusedPane::Left;
    }

    /// Switch focus to the middle pane.
    pub fn focus_middle(&mut self) {
        self.focused_pane = FocusedPane::Middle;
    }

    /// Switch focus to the right pane.
    pub fn focus_right(&mut self) {
        self.focused_pane = FocusedPane::Right;
    }

    /// Get the currently selected email, if any.
    pub fn current_email(&self) -> Option<&MessageSync> {
        self.emails.get(self.email_selection.index)
    }

    /// Cycle search mode.
    pub fn cycle_search_mode(&mut self) {
        self.search_mode = self.search_mode.cycle();
        self.mode = AppMode::Search(self.search_mode.clone());
    }

    /// Enter folder select mode.
    pub fn enter_folder_select(&mut self) {
        self.mode = AppMode::FolderSelect;
    }

    /// Enter compose mode.
    pub fn enter_compose(&mut self) {
        self.mode = AppMode::Compose;
        self.set_status("Composing new email...");
    }

    /// Get indices of emails to operate on (selected or current).
    pub fn get_operation_indices(&self) -> Vec<usize> {
        self.email_selection.get_operation_indices()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Selection tests
    mod selection_tests {
        use super::*;

        #[test]
        fn test_selection_new() {
            let sel = Selection::new();
            assert_eq!(sel.index, 0);
            assert_eq!(sel.offset, 0);
            assert!(sel.selected_indices.is_empty());
        }

        #[test]
        fn test_selection_next() {
            let mut sel = Selection::new();
            sel.next(10, 5);
            assert_eq!(sel.index, 1);
            assert_eq!(sel.offset, 0);

            // Move to boundary of page
            for _ in 0..4 {
                sel.next(10, 5);
            }
            assert_eq!(sel.index, 5);
            assert_eq!(sel.offset, 1); // Offset adjusts to keep cursor visible

            // Don't go past max
            for _ in 0..10 {
                sel.next(10, 5);
            }
            assert_eq!(sel.index, 9);
        }

        #[test]
        fn test_selection_next_empty_list() {
            let mut sel = Selection::new();
            sel.next(0, 5);
            assert_eq!(sel.index, 0);
        }

        #[test]
        fn test_selection_previous() {
            let mut sel = Selection::new();
            sel.index = 5;
            sel.offset = 2;

            sel.previous();
            assert_eq!(sel.index, 4);
            assert_eq!(sel.offset, 2);

            // Move to offset boundary
            sel.previous();
            sel.previous();
            assert_eq!(sel.index, 2);
            assert_eq!(sel.offset, 2);

            sel.previous();
            assert_eq!(sel.index, 1);
            assert_eq!(sel.offset, 1); // Offset adjusts

            // Don't go below 0
            sel.previous();
            sel.previous();
            assert_eq!(sel.index, 0);
        }

        #[test]
        fn test_selection_top() {
            let mut sel = Selection::new();
            sel.index = 50;
            sel.offset = 45;

            sel.top();
            assert_eq!(sel.index, 0);
            assert_eq!(sel.offset, 0);
        }

        #[test]
        fn test_selection_bottom() {
            let mut sel = Selection::new();
            sel.bottom(100, 10);
            assert_eq!(sel.index, 99);
            assert_eq!(sel.offset, 90);
        }

        #[test]
        fn test_selection_bottom_empty_list() {
            let mut sel = Selection::new();
            sel.bottom(0, 10);
            assert_eq!(sel.index, 0);
        }

        #[test]
        fn test_selection_page_down() {
            let mut sel = Selection::new();
            sel.page_down(100, 10);
            assert_eq!(sel.index, 10);

            sel.page_down(100, 10);
            assert_eq!(sel.index, 20);

            // Test near end
            sel.index = 95;
            sel.page_down(100, 10);
            assert_eq!(sel.index, 99);
        }

        #[test]
        fn test_selection_page_up() {
            let mut sel = Selection::new();
            sel.index = 50;
            sel.offset = 45;

            sel.page_up(10);
            assert_eq!(sel.index, 40);
            assert_eq!(sel.offset, 40);

            // Test near beginning
            sel.index = 5;
            sel.page_up(10);
            assert_eq!(sel.index, 0);
        }

        #[test]
        fn test_selection_toggle() {
            let mut sel = Selection::new();
            assert!(!sel.has_selections());

            sel.toggle_selection();
            assert!(sel.has_selections());
            assert!(sel.selected_indices.contains(&0));

            sel.toggle_selection();
            assert!(!sel.has_selections());
        }

        #[test]
        fn test_selection_toggle_and_next() {
            let mut sel = Selection::new();
            sel.toggle_and_next(10, 5);

            assert!(sel.selected_indices.contains(&0));
            assert_eq!(sel.index, 1);
        }

        #[test]
        fn test_selection_select_all() {
            let mut sel = Selection::new();
            sel.select_all(10);
            assert_eq!(sel.selected_indices.len(), 10);
            for i in 0..10 {
                assert!(sel.selected_indices.contains(&i));
            }
        }

        #[test]
        fn test_selection_deselect_all() {
            let mut sel = Selection::new();
            sel.select_all(10);
            sel.deselect_all();
            assert!(!sel.has_selections());
        }

        #[test]
        fn test_selection_get_selected_indices_sorted() {
            let mut sel = Selection::new();
            sel.selected_indices.insert(5);
            sel.selected_indices.insert(1);
            sel.selected_indices.insert(9);
            sel.selected_indices.insert(3);

            let indices = sel.get_selected_indices();
            assert_eq!(indices, vec![1, 3, 5, 9]);
        }

        #[test]
        fn test_selection_get_operation_indices() {
            let mut sel = Selection::new();
            sel.index = 5;

            // No selections - returns current index
            let indices = sel.get_operation_indices();
            assert_eq!(indices, vec![5]);

            // With selections - returns selected indices
            sel.selected_indices.insert(1);
            sel.selected_indices.insert(3);
            let indices = sel.get_operation_indices();
            assert_eq!(indices, vec![1, 3]);
        }

        #[test]
        fn test_selection_reset() {
            let mut sel = Selection::new();
            sel.index = 50;
            sel.offset = 45;
            sel.select_all(100);

            sel.reset();
            assert_eq!(sel.index, 0);
            assert_eq!(sel.offset, 0);
            assert!(!sel.has_selections());
        }
    }

    // SearchMode tests
    mod search_mode_tests {
        use super::*;

        #[test]
        fn test_search_mode_as_str() {
            assert_eq!(SearchMode::Subject.as_str(), "Subject");
            assert_eq!(SearchMode::From.as_str(), "From");
            assert_eq!(SearchMode::To.as_str(), "To");
            assert_eq!(SearchMode::Body.as_str(), "Body");
            assert_eq!(SearchMode::All.as_str(), "All");
        }

        #[test]
        fn test_search_mode_cycle() {
            assert_eq!(SearchMode::Subject.cycle(), SearchMode::From);
            assert_eq!(SearchMode::From.cycle(), SearchMode::To);
            assert_eq!(SearchMode::To.cycle(), SearchMode::Body);
            assert_eq!(SearchMode::Body.cycle(), SearchMode::All);
            assert_eq!(SearchMode::All.cycle(), SearchMode::Subject);
        }
    }

    // SortOption tests
    mod sort_option_tests {
        use super::*;

        #[test]
        fn test_sort_option_all_count() {
            assert_eq!(SortOption::ALL.len(), 6);
        }

        #[test]
        fn test_sort_option_as_str() {
            assert_eq!(SortOption::DateDesc.as_str(), "Date (Newest First)");
            assert_eq!(SortOption::DateAsc.as_str(), "Date (Oldest First)");
            assert_eq!(SortOption::SubjectAsc.as_str(), "Subject (A-Z)");
            assert_eq!(SortOption::SubjectDesc.as_str(), "Subject (Z-A)");
            assert_eq!(SortOption::FromAsc.as_str(), "From (A-Z)");
            assert_eq!(SortOption::FromDesc.as_str(), "From (Z-A)");
        }
    }

    // WhichKeyContext tests
    mod which_key_tests {
        use super::*;

        #[test]
        fn test_which_key_context_title() {
            assert_eq!(WhichKeyContext::Folder.title(), "Folder");
            assert_eq!(WhichKeyContext::Actions.title(), "Actions");
            assert_eq!(WhichKeyContext::View.title(), "View");
        }

        #[test]
        fn test_which_key_context_options() {
            let folder_opts = WhichKeyContext::Folder.options();
            assert!(!folder_opts.is_empty());
            assert!(folder_opts.iter().any(|(k, _)| *k == 'i'));

            let action_opts = WhichKeyContext::Actions.options();
            assert!(!action_opts.is_empty());
            assert!(action_opts.iter().any(|(k, _)| *k == 'r'));
        }
    }

    // App tests
    mod app_tests {
        use super::*;

        #[test]
        fn test_app_new() {
            let app = App::new();
            assert_eq!(app.mode, AppMode::Normal);
            assert_eq!(app.focused_pane, FocusedPane::Middle);
            assert!(!app.should_quit);
            assert_eq!(app.current_folder, "inbox");
            assert!(!app.folders.is_empty());
        }

        #[test]
        fn test_app_set_status() {
            let mut app = App::new();
            assert!(app.status_message.is_none());

            app.set_status("Test message");
            assert_eq!(app.status_message, Some("Test message".to_string()));

            app.clear_status();
            assert!(app.status_message.is_none());
        }

        #[test]
        fn test_app_current_folder_display() {
            let app = App::new();
            assert_eq!(app.current_folder_display(), "Inbox");
        }

        #[test]
        fn test_app_selected_count() {
            let mut app = App::new();
            assert_eq!(app.selected_count(), 0);

            // Add some emails
            app.emails.push(MessageSync {
                local_id: "1".to_string(),
                remote_id: "r1".to_string(),
                change_key: None,
                folder: "inbox".to_string(),
                subject: Some("Test".to_string()),
                from_addr: Some("test@example.com".to_string()),
                received_at: None,
                is_read: false,
                is_draft: false,
                synced_at: None,
                local_hash: None,
            });

            // No explicit selection = 1 (current)
            assert_eq!(app.selected_count(), 1);

            // Multi-select
            app.email_selection.select_all(1);
            assert_eq!(app.selected_count(), 1);
        }

        #[test]
        fn test_app_enter_search() {
            let mut app = App::new();
            app.search_query = "old query".to_string();

            app.enter_search();
            assert!(matches!(app.mode, AppMode::Search(_)));
            assert!(app.search_query.is_empty());
        }

        #[test]
        fn test_app_exit_search() {
            let mut app = App::new();
            app.enter_search();
            app.exit_search();
            assert_eq!(app.mode, AppMode::Normal);
        }

        #[test]
        fn test_app_enter_sort() {
            let mut app = App::new();
            app.sort_option = SortOption::SubjectAsc;

            app.enter_sort();
            assert_eq!(app.mode, AppMode::Sort);
            assert_eq!(app.sort_selection, 2); // SubjectAsc is at index 2
        }

        #[test]
        fn test_app_apply_sort() {
            let mut app = App::new();
            app.enter_sort();
            app.sort_selection = 1; // DateAsc

            app.apply_sort();
            assert_eq!(app.sort_option, SortOption::DateAsc);
            assert_eq!(app.mode, AppMode::Normal);
            assert!(app.status_message.is_some());
        }

        #[test]
        fn test_app_enter_help() {
            let mut app = App::new();
            app.enter_help();
            assert_eq!(app.mode, AppMode::Help);
            assert_eq!(app.help_scroll, 0);
        }

        #[test]
        fn test_app_enter_delete() {
            let mut app = App::new();

            // Empty list - no mode change
            app.enter_delete();
            assert_eq!(app.mode, AppMode::Normal);

            // Add email
            app.emails.push(MessageSync {
                local_id: "1".to_string(),
                remote_id: "r1".to_string(),
                change_key: None,
                folder: "inbox".to_string(),
                subject: Some("Test".to_string()),
                from_addr: None,
                received_at: None,
                is_read: false,
                is_draft: false,
                synced_at: None,
                local_hash: None,
            });

            // Single delete
            app.enter_delete();
            assert_eq!(app.mode, AppMode::Delete);

            // Multi delete
            app.return_to_normal();
            app.email_selection.toggle_selection();
            app.enter_delete();
            assert_eq!(app.mode, AppMode::DeleteMultiple);
        }

        #[test]
        fn test_app_enter_which_key() {
            let mut app = App::new();
            app.enter_which_key(WhichKeyContext::Folder);
            assert_eq!(app.mode, AppMode::WhichKey(WhichKeyContext::Folder));
        }

        #[test]
        fn test_app_return_to_normal() {
            let mut app = App::new();
            app.mode = AppMode::Help;
            app.g_prefix = true;

            app.return_to_normal();
            assert_eq!(app.mode, AppMode::Normal);
            assert!(!app.g_prefix);
        }

        #[test]
        fn test_app_focus_panes() {
            let mut app = App::new();
            assert_eq!(app.focused_pane, FocusedPane::Middle);

            app.focus_left();
            assert_eq!(app.focused_pane, FocusedPane::Left);

            app.focus_right();
            assert_eq!(app.focused_pane, FocusedPane::Right);

            app.focus_middle();
            assert_eq!(app.focused_pane, FocusedPane::Middle);
        }

        #[test]
        fn test_app_current_email() {
            let mut app = App::new();
            assert!(app.current_email().is_none());

            app.emails.push(MessageSync {
                local_id: "1".to_string(),
                remote_id: "r1".to_string(),
                change_key: None,
                folder: "inbox".to_string(),
                subject: Some("Test Email".to_string()),
                from_addr: None,
                received_at: None,
                is_read: false,
                is_draft: false,
                synced_at: None,
                local_hash: None,
            });

            let email = app.current_email().unwrap();
            assert_eq!(email.subject, Some("Test Email".to_string()));
        }

        #[test]
        fn test_app_cycle_search_mode() {
            let mut app = App::new();
            app.enter_search();

            app.cycle_search_mode();
            assert_eq!(app.search_mode, SearchMode::From);
            assert!(matches!(app.mode, AppMode::Search(SearchMode::From)));
        }
    }

    // FolderInfo tests
    mod folder_info_tests {
        use super::*;

        #[test]
        fn test_folder_info_new() {
            let folder = FolderInfo::new("inbox", "Inbox");
            assert_eq!(folder.name, "inbox");
            assert_eq!(folder.display_name, "Inbox");
            assert_eq!(folder.unread_count, 0);
            assert_eq!(folder.total_count, 0);
        }
    }
}
