use std::io::{self, BufWriter, Write};
use std::sync::{Arc, Mutex};

use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;
use ratatui::{Terminal, TerminalOptions, Viewport};

use crate::app::frame_backend::{FrameBackend, FRAME_BUFFER_CAPACITY};

/// Plays the role of the tty: records every `write()` call — each entry
/// models one syscall reaching the terminal. Terminals may paint between
/// syscalls, and only the final one determines where the hardware cursor
/// comes to rest.
#[derive(Clone, Default)]
struct RecordingWriter {
    calls: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Write for RecordingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.calls.lock().unwrap().push(buf.to_vec());
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// `Terminal::draw` over a 16×4 fixed viewport: renders "hello" plus a caret
/// position (2, 3), exactly like the app's input box does every frame.
/// `Viewport::Fixed` avoids `backend.size()`, so no real tty is needed.
fn draw_one_frame<B: Backend>(backend: B) -> io::Result<()> {
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, 16, 4)),
        },
    )?;
    terminal.draw(|frame| {
        frame.render_widget(Paragraph::new("hello"), frame.area());
        frame.set_cursor_position((2, 3));
    })?;
    Ok(())
}

/// The backend under test plus the shared record of the writes it produced.
type RecordingFrameBackend = (
    FrameBackend<BufWriter<RecordingWriter>>,
    Arc<Mutex<Vec<Vec<u8>>>>,
);

fn recording_frame_backend() -> RecordingFrameBackend {
    let writer = RecordingWriter::default();
    let calls = writer.calls.clone();
    let backend = FrameBackend::new(BufWriter::with_capacity(FRAME_BUFFER_CAPACITY, writer));
    (backend, calls)
}

#[test]
fn frame_reaches_terminal_as_a_single_write() {
    let (backend, calls) = recording_frame_backend();
    draw_one_frame(backend).unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        1,
        "a frame must reach the tty as exactly one write, got {}: {:?}",
        calls.len(),
        calls
    );
    let frame = &calls[0];
    // Cell content, cursor visibility and the caret MoveTo all travel
    // together...
    assert!(
        String::from_utf8_lossy(frame).contains("hello"),
        "cell content missing from the frame: {:?}",
        frame
    );
    assert!(
        frame.windows(6).any(|w| w == b"\x1b[?25h"),
        "cursor Show missing from the frame: {:?}",
        frame
    );
    // ...and the caret MoveTo is the very last sequence, so nothing can
    // leave the hardware cursor anywhere else.
    assert!(
        frame.ends_with(b"\x1b[4;3H"),
        "frame must end with the caret MoveTo: {:?}",
        frame
    );
}

#[test]
fn stock_backend_splits_the_frame_upstream_behavior() {
    // Control test documenting WHY FrameBackend exists: ratatui 0.28's
    // CrosstermBackend emits the caret MoveTo via crossterm's `execute!`
    // (write + flush), so the frame arrives as two writes — [cells +
    // ESC[?25h], then the MoveTo. A terminal that paints between them shows
    // the steady caret resting at the last cell written (header, borders,
    // status bar) until the MoveTo lands — the reported "cursor flickers to
    // other places". If this test starts failing with 1 write, ratatui has
    // fixed it and FrameBackend can be retired.
    let writer = RecordingWriter::default();
    let calls = writer.calls.clone();
    let backend = CrosstermBackend::new(BufWriter::with_capacity(FRAME_BUFFER_CAPACITY, writer));
    draw_one_frame(backend).unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        2,
        "stock backend expected to split the frame into cells+Show / MoveTo, got: {:?}",
        calls
    );
    assert!(
        String::from_utf8_lossy(&calls[0]).contains("hello")
            && calls[0].windows(6).any(|w| w == b"\x1b[?25h"),
        "first write should carry cells + Show: {:?}",
        calls[0]
    );
    assert_eq!(
        calls[1].as_slice(),
        b"\x1b[4;3H".as_slice(),
        "second write should be the split-off caret MoveTo"
    );
}
