//! Single-line text editing model for the search box: caret movement,
//! word-wise navigation and deletion, select-all, paste.

#[derive(Default, Clone, Debug)]
pub struct LineEdit {
    chars: Vec<char>,
    caret: usize,
    all_selected: bool,
}

impl LineEdit {
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// Caret position in chars.
    #[allow(dead_code)]
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// Text before the caret (for measuring where to draw it).
    pub fn before_caret(&self) -> String {
        self.chars[..self.caret].iter().collect()
    }

    pub fn all_selected(&self) -> bool {
        self.all_selected && !self.chars.is_empty()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn select_all(&mut self) {
        self.all_selected = true;
        self.caret = self.chars.len();
    }

    /// Replaces the selection if everything is selected. Returns true if it did.
    fn take_selection(&mut self) -> bool {
        if self.all_selected() {
            self.clear();
            true
        } else {
            self.all_selected = false;
            false
        }
    }

    pub fn insert(&mut self, s: &str) {
        self.take_selection();
        for c in s.chars().filter(|c| !c.is_control()) {
            self.chars.insert(self.caret, c);
            self.caret += 1;
        }
    }

    pub fn backspace(&mut self) {
        if self.take_selection() || self.caret == 0 {
            return;
        }
        self.caret -= 1;
        self.chars.remove(self.caret);
    }

    pub fn delete(&mut self) {
        if self.take_selection() || self.caret >= self.chars.len() {
            return;
        }
        self.chars.remove(self.caret);
    }

    pub fn delete_word_back(&mut self) {
        if self.take_selection() {
            return;
        }
        let start = self.word_left_pos();
        self.chars.drain(start..self.caret);
        self.caret = start;
    }

    pub fn left(&mut self) {
        self.all_selected = false;
        self.caret = self.caret.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.all_selected = false;
        self.caret = (self.caret + 1).min(self.chars.len());
    }

    pub fn home(&mut self) {
        self.all_selected = false;
        self.caret = 0;
    }

    pub fn end(&mut self) {
        self.all_selected = false;
        self.caret = self.chars.len();
    }

    pub fn word_left(&mut self) {
        self.all_selected = false;
        self.caret = self.word_left_pos();
    }

    pub fn word_right(&mut self) {
        self.all_selected = false;
        let n = self.chars.len();
        let mut i = self.caret;
        while i < n && !self.chars[i].is_alphanumeric() {
            i += 1;
        }
        while i < n && self.chars[i].is_alphanumeric() {
            i += 1;
        }
        self.caret = i;
    }

    fn word_left_pos(&self) -> usize {
        let mut i = self.caret;
        while i > 0 && !self.chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        while i > 0 && self.chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        i
    }
}

#[cfg(test)]
mod tests {
    use super::LineEdit;

    fn typed(s: &str) -> LineEdit {
        let mut e = LineEdit::default();
        e.insert(s);
        e
    }

    #[test]
    fn insert_in_middle() {
        let mut e = typed("ntepad");
        e.home();
        e.right();
        e.insert("o");
        assert_eq!(e.text(), "notepad");
        assert_eq!(e.caret(), 2);
    }

    #[test]
    fn backspace_and_delete() {
        let mut e = typed("abc");
        e.backspace();
        assert_eq!(e.text(), "ab");
        e.home();
        e.delete();
        assert_eq!(e.text(), "b");
        e.home();
        e.backspace(); // nothing before caret
        assert_eq!(e.text(), "b");
    }

    #[test]
    fn word_ops() {
        let mut e = typed("visual studio code");
        e.delete_word_back();
        assert_eq!(e.text(), "visual studio ");
        e.delete_word_back();
        assert_eq!(e.text(), "visual ");
        e.home();
        e.word_right();
        assert_eq!(e.caret(), 6);
        e.end();
        e.word_left();
        assert_eq!(e.caret(), 0);
    }

    #[test]
    fn select_all_then_type_replaces() {
        let mut e = typed("firefox");
        e.select_all();
        assert!(e.all_selected());
        e.insert("chrome");
        assert_eq!(e.text(), "chrome");
        e.select_all();
        e.backspace();
        assert!(e.is_empty());
    }

    #[test]
    fn paste_strips_newlines_and_handles_unicode() {
        let mut e = typed("é");
        e.insert("line1\r\nline2");
        assert_eq!(e.text(), "éline1line2");
        assert_eq!(e.before_caret().chars().count(), e.caret());
    }

    #[test]
    fn moving_clears_selection() {
        let mut e = typed("abc");
        e.select_all();
        e.left();
        assert!(!e.all_selected());
        e.insert("x");
        assert_eq!(e.text(), "abxc");
    }
}
