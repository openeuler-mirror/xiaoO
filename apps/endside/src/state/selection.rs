/// Represents an active text selection spanning one or more rendered lines
/// of the chat transcript.
///
/// All indices are in terms of the **flat rendered-line array** (i.e. the
/// `all_lines` vector that is built in `render_chat`), plus a char column
/// offset within that line.
#[derive(Debug, Clone)]
pub struct TranscriptSelection {
    /// The line/column where the selection was started.
    pub anchor_line: usize,
    pub anchor_col: usize,
    /// The line/column where the selection currently ends (follows cursor/drag).
    pub cursor_line: usize,
    pub cursor_col: usize,
}

impl TranscriptSelection {
    /// Create a new selection starting and ending at `(line, col)`.
    pub fn new(line: usize, col: usize) -> Self {
        Self {
            anchor_line: line,
            anchor_col: col,
            cursor_line: line,
            cursor_col: col,
        }
    }

    /// Returns `(start_line, start_col, end_line, end_col)` normalised so
    /// that start ≤ end in document order.
    pub fn normalised(&self) -> (usize, usize, usize, usize) {
        let anchor = (self.anchor_line, self.anchor_col);
        let cursor = (self.cursor_line, self.cursor_col);
        if anchor <= cursor {
            (anchor.0, anchor.1, cursor.0, cursor.1)
        } else {
            (cursor.0, cursor.1, anchor.0, anchor.1)
        }
    }

    /// Returns `true` if the selection is empty (anchor == cursor).
    pub fn is_empty(&self) -> bool {
        self.anchor_line == self.cursor_line && self.anchor_col == self.cursor_col
    }
}
