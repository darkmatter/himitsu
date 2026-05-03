//! Plain text-area editor for the envs DSL.
//!
//! This is a deliberately minimal text widget — line-based, plain insert
//! mode, no Helix-style modal editing. The brief allowed a fall-back to a
//! plain textarea, and that is exactly what this is. The richer pieces
//! (autocomplete, fuzzy-find overlay) are implemented around the buffer
//! rather than inside an editor framework.
//!
//! Buffer model: `Vec<String>` of lines, plus `(row, col)` cursor expressed
//! in characters (not bytes). All edits go through methods so we can keep
//! the cursor invariants in one place.

use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone)]
pub struct TextBuffer {
    lines: Vec<String>,
    /// Cursor row (0-based, always < lines.len()).
    row: usize,
    /// Cursor column in characters (0-based, can equal line.chars().count()
    /// to represent end-of-line).
    col: usize,
}

impl TextBuffer {
    pub fn new(initial: &str) -> Self {
        let mut lines: Vec<String> = initial.split('\n').map(|s| s.to_string()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self {
            lines,
            row: 0,
            col: 0,
        }
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// Current line up to cursor — used by autocomplete to extract the
    /// in-progress token.
    pub fn line_before_cursor(&self) -> &str {
        let line = &self.lines[self.row];
        let byte_idx = byte_idx_for_char(line, self.col);
        &line[..byte_idx]
    }

    /// Insert a single character at the cursor.
    pub fn insert_char(&mut self, c: char) {
        let line = &mut self.lines[self.row];
        let byte_idx = byte_idx_for_char(line, self.col);
        line.insert(byte_idx, c);
        self.col += 1;
    }

    /// Insert a literal string (no embedded newlines) at the cursor.
    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            if c == '\n' {
                self.insert_newline();
            } else {
                self.insert_char(c);
            }
        }
    }

    pub fn insert_newline(&mut self) {
        let line = &mut self.lines[self.row];
        let byte_idx = byte_idx_for_char(line, self.col);
        let tail = line.split_off(byte_idx);
        self.lines.insert(self.row + 1, tail);
        self.row += 1;
        self.col = 0;
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            let line = &mut self.lines[self.row];
            let prev = self.col - 1;
            let start = byte_idx_for_char(line, prev);
            let end = byte_idx_for_char(line, self.col);
            line.replace_range(start..end, "");
            self.col = prev;
        } else if self.row > 0 {
            // Join with previous line.
            let cur = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
            self.lines[self.row].push_str(&cur);
        }
    }

    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
        }
    }

    pub fn move_right(&mut self) {
        let len = self.lines[self.row].chars().count();
        if self.col < len {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    pub fn move_up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            let len = self.lines[self.row].chars().count();
            self.col = self.col.min(len);
        }
    }

    pub fn move_down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            let len = self.lines[self.row].chars().count();
            self.col = self.col.min(len);
        }
    }

    pub fn move_line_start(&mut self) {
        self.col = 0;
    }

    pub fn move_line_end(&mut self) {
        self.col = self.lines[self.row].chars().count();
    }

    /// Apply a key event to the buffer. Returns true if the event was
    /// handled (mutated buffer or moved cursor); false otherwise so the
    /// caller can route it elsewhere (e.g. close editor on Esc).
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        match (key.code, key.modifiers) {
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                self.insert_char(c);
                true
            }
            (KeyCode::Enter, _) => {
                self.insert_newline();
                true
            }
            (KeyCode::Backspace, _) => {
                self.backspace();
                true
            }
            (KeyCode::Left, _) => {
                self.move_left();
                true
            }
            (KeyCode::Right, _) => {
                self.move_right();
                true
            }
            (KeyCode::Up, _) => {
                self.move_up();
                true
            }
            (KeyCode::Down, _) => {
                self.move_down();
                true
            }
            (KeyCode::Home, _) => {
                self.move_line_start();
                true
            }
            (KeyCode::End, _) => {
                self.move_line_end();
                true
            }
            _ => false,
        }
    }
}

impl fmt::Display for TextBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.lines.join("\n"))
    }
}

fn byte_idx_for_char(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn insert_chars_advances_cursor() {
        let mut b = TextBuffer::new("");
        b.insert_char('h');
        b.insert_char('i');
        assert_eq!(b.to_string(), "hi");
        assert_eq!(b.cursor(), (0, 2));
    }

    #[test]
    fn newline_splits_line() {
        let mut b = TextBuffer::new("ab");
        b.move_right();
        b.insert_newline();
        assert_eq!(b.lines(), &["a".to_string(), "b".to_string()]);
        assert_eq!(b.cursor(), (1, 0));
    }

    #[test]
    fn backspace_joins_lines() {
        let mut b = TextBuffer::new("a\nb");
        b.move_down();
        b.move_line_start();
        b.backspace();
        assert_eq!(b.to_string(), "ab");
        assert_eq!(b.cursor(), (0, 1));
    }

    #[test]
    fn arrows_clamp_to_line_length() {
        let mut b = TextBuffer::new("ab\nx");
        b.move_line_end(); // (0, 2)
        b.move_down();
        assert_eq!(b.cursor(), (1, 1)); // clamped from 2 to 1
    }

    #[test]
    fn line_before_cursor_returns_prefix() {
        let mut b = TextBuffer::new("hello world");
        for _ in 0..5 {
            b.move_right();
        }
        assert_eq!(b.line_before_cursor(), "hello");
    }

    #[test]
    fn on_key_handles_typing() {
        let mut b = TextBuffer::new("");
        for c in "abc".chars() {
            b.on_key(k(c));
        }
        assert_eq!(b.to_string(), "abc");
    }

    #[test]
    fn unicode_navigation_safe() {
        let mut b = TextBuffer::new("héllo");
        b.move_line_end();
        assert_eq!(b.cursor(), (0, 5));
        b.backspace();
        assert_eq!(b.to_string(), "héll");
    }
}
