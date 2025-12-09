//! Key action parsing and representation.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Semantic key actions parsed from raw key events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// Quit the application (Ctrl-C).
    Quit,
    /// Move up in the current list.
    Up,
    /// Move down in the current list.
    Down,
    /// Move left (switch pane or navigate).
    Left,
    /// Move right (switch pane or navigate).
    Right,
    /// Page down (Ctrl-D).
    PageDown,
    /// Page up (Ctrl-U).
    PageUp,
    /// Select / Enter / Confirm.
    Select,
    /// Escape / Cancel.
    Escape,
    /// Backspace.
    Backspace,
    /// A printable character.
    Char(char),
    /// Toggle selection (Space).
    ToggleSelect,
    /// Select all (Ctrl-A).
    SelectAll,
    /// Cycle search mode (Tab).
    CycleSearchMode,
    /// No action / Unknown key.
    Noop,
}

impl From<KeyEvent> for KeyAction {
    fn from(event: KeyEvent) -> Self {
        match (event.code, event.modifiers) {
            // Quit
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => KeyAction::Quit,

            // Navigation
            (KeyCode::Up, _) => KeyAction::Up,
            (KeyCode::Down, _) => KeyAction::Down,
            (KeyCode::Left, _) => KeyAction::Left,
            (KeyCode::Right, _) => KeyAction::Right,

            // Page navigation
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => KeyAction::PageDown,
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => KeyAction::PageUp,
            (KeyCode::PageDown, _) => KeyAction::PageDown,
            (KeyCode::PageUp, _) => KeyAction::PageUp,

            // Selection
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => KeyAction::SelectAll,
            (KeyCode::Char(' '), _) => KeyAction::ToggleSelect,

            // Actions
            (KeyCode::Enter, _) => KeyAction::Select,
            (KeyCode::Esc, _) => KeyAction::Escape,
            (KeyCode::Backspace, _) => KeyAction::Backspace,
            (KeyCode::Tab, _) => KeyAction::CycleSearchMode,

            // Characters (handle shift for uppercase)
            (KeyCode::Char(c), KeyModifiers::SHIFT) => KeyAction::Char(c.to_ascii_uppercase()),
            (KeyCode::Char(c), KeyModifiers::NONE) => KeyAction::Char(c),
            (KeyCode::Char(c), _) => KeyAction::Char(c),

            _ => KeyAction::Noop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn test_quit() {
        let action: KeyAction = key_event(KeyCode::Char('c'), KeyModifiers::CONTROL).into();
        assert_eq!(action, KeyAction::Quit);
    }

    #[test]
    fn test_arrow_keys() {
        assert_eq!(
            KeyAction::from(key_event(KeyCode::Up, KeyModifiers::NONE)),
            KeyAction::Up
        );
        assert_eq!(
            KeyAction::from(key_event(KeyCode::Down, KeyModifiers::NONE)),
            KeyAction::Down
        );
        assert_eq!(
            KeyAction::from(key_event(KeyCode::Left, KeyModifiers::NONE)),
            KeyAction::Left
        );
        assert_eq!(
            KeyAction::from(key_event(KeyCode::Right, KeyModifiers::NONE)),
            KeyAction::Right
        );
    }

    #[test]
    fn test_page_navigation() {
        assert_eq!(
            KeyAction::from(key_event(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            KeyAction::PageDown
        );
        assert_eq!(
            KeyAction::from(key_event(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            KeyAction::PageUp
        );
        assert_eq!(
            KeyAction::from(key_event(KeyCode::PageDown, KeyModifiers::NONE)),
            KeyAction::PageDown
        );
        assert_eq!(
            KeyAction::from(key_event(KeyCode::PageUp, KeyModifiers::NONE)),
            KeyAction::PageUp
        );
    }

    #[test]
    fn test_select_all() {
        let action: KeyAction = key_event(KeyCode::Char('a'), KeyModifiers::CONTROL).into();
        assert_eq!(action, KeyAction::SelectAll);
    }

    #[test]
    fn test_toggle_select() {
        let action: KeyAction = key_event(KeyCode::Char(' '), KeyModifiers::NONE).into();
        assert_eq!(action, KeyAction::ToggleSelect);
    }

    #[test]
    fn test_enter() {
        let action: KeyAction = key_event(KeyCode::Enter, KeyModifiers::NONE).into();
        assert_eq!(action, KeyAction::Select);
    }

    #[test]
    fn test_escape() {
        let action: KeyAction = key_event(KeyCode::Esc, KeyModifiers::NONE).into();
        assert_eq!(action, KeyAction::Escape);
    }

    #[test]
    fn test_backspace() {
        let action: KeyAction = key_event(KeyCode::Backspace, KeyModifiers::NONE).into();
        assert_eq!(action, KeyAction::Backspace);
    }

    #[test]
    fn test_tab() {
        let action: KeyAction = key_event(KeyCode::Tab, KeyModifiers::NONE).into();
        assert_eq!(action, KeyAction::CycleSearchMode);
    }

    #[test]
    fn test_lowercase_char() {
        let action: KeyAction = key_event(KeyCode::Char('j'), KeyModifiers::NONE).into();
        assert_eq!(action, KeyAction::Char('j'));
    }

    #[test]
    fn test_uppercase_char() {
        let action: KeyAction = key_event(KeyCode::Char('G'), KeyModifiers::SHIFT).into();
        assert_eq!(action, KeyAction::Char('G'));
    }

    #[test]
    fn test_unknown_key() {
        let action: KeyAction = key_event(KeyCode::F(1), KeyModifiers::NONE).into();
        assert_eq!(action, KeyAction::Noop);
    }
}
