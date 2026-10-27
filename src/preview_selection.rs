use std::ops::Range;

/// Byte offsets into the complete rendered document, never into mounted rows.
#[derive(Debug, Default)]
pub(crate) struct PreviewSelection {
    pub anchor: usize,
    pub cursor: usize,
    pub dragging: bool,
}

impl PreviewSelection {
    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }

    pub fn begin(&mut self, offset: usize, extend: bool) {
        if !extend {
            self.anchor = offset;
        }
        self.cursor = offset;
        self.dragging = true;
    }

    pub fn select_all(&mut self, text: &str) {
        self.anchor = 0;
        self.cursor = text.len();
        self.dragging = false;
    }

    pub fn select_word(&mut self, text: &str, offset: usize) {
        let mut offset = offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let word = |character: char| character.is_alphanumeric() || character == '_';
        self.anchor = text[..offset]
            .char_indices()
            .rev()
            .find(|(_, character)| !word(*character))
            .map_or(0, |(index, character)| index + character.len_utf8());
        self.cursor = text[offset..]
            .char_indices()
            .find(|(_, character)| !word(*character))
            .map_or(text.len(), |(index, _)| offset + index);
        self.dragging = false;
    }

    pub fn selected_text<'a>(&self, text: &'a str) -> &'a str {
        text.get(self.range()).unwrap_or_default()
    }

    pub fn local_range(&self, row: &Range<usize>) -> Option<Range<usize>> {
        let selected = self.range();
        let start = selected.start.max(row.start);
        let end = selected.end.min(row.end);
        (start < end).then_some(start.saturating_sub(row.start)..end.saturating_sub(row.start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_selection_includes_unmounted_middle_and_handles_reverse_drag() {
        let text = "first\n\nunmounted middle\n\nlast";
        let mut selection = PreviewSelection::default();
        selection.begin(text.len(), false);
        selection.cursor = 2;
        assert_eq!(selection.selected_text(text), &text[2..]);
        assert_eq!(selection.local_range(&(0..5)), Some(2..5));
        assert_eq!(selection.local_range(&(7..23)), Some(0..16));
        selection.begin(0, true);
        assert_eq!(selection.selected_text(text), text);
    }

    #[test]
    fn select_all_and_words_preserve_unicode_boundaries() {
        let text = "alpha café 🦀 omega";
        let mut selection = PreviewSelection::default();
        selection.select_word(text, 10);
        assert_eq!(selection.selected_text(text), "café");
        selection.select_all(text);
        assert_eq!(selection.selected_text(text), text);
        assert!(!selection.dragging);
    }
}
