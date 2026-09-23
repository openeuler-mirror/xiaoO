use std::io::{self, BufWriter, Write};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::queue;
use ratatui::{
    backend::{Backend, CrosstermBackend, WindowSize},
    buffer::Cell,
    layout::Size,
    prelude::Position,
};

/// Capacity of the per-frame staging buffer, in bytes.
///
/// A full-screen rewrite — the 30s corruption-recovery repaint on a large
/// terminal — is the biggest frame the app emits. 512 KiB covers a dense
/// 300×100 grid with a worst-case ~30 bytes per changed cell (MoveTo + SGR +
/// symbol). Frames that somehow exceed it simply degrade to a split write
/// (BufWriter flushes mid-frame); there is no correctness cliff.
pub const FRAME_BUFFER_CAPACITY: usize = 512 * 1024;

/// A [`Backend`] that delivers each frame to the terminal as a single
/// `write()` syscall.
///
/// # Why this exists
///
/// Ratatui 0.28's `CrosstermBackend` writes cell content with crossterm's
/// `queue!` (buffered), but its cursor commands — `show_cursor`,
/// `set_cursor_position`, `hide_cursor` — use `execute!`, which **flushes
/// after every command**. A single `Terminal::draw()` therefore reaches the
/// tty as at least two writes: `[cells + ESC[?25h]`, then the caret
/// `ESC[y;xH`. Large frames are split further: `io::Stdout` is a `LineWriter`
/// with a 1024-byte buffer, so a full-screen rewrite trickles out in ~1 KiB
/// chunks.
///
/// Terminals paint between those chunks. Because the frame's cell writes
/// each reposition the hardware cursor (ratatui emits one `MoveTo` per
/// non-adjacent run, i.e. roughly one per screen row), the always-visible
/// steady caret is briefly rendered at intermediate positions — the header,
/// transcript borders, the status bar — before the trailing caret `MoveTo`
/// lands it back in the input box. With the steady (non-blinking) cursor
/// style this reads as "the caret flickers to other places", most visibly
/// once per second while idle (the periodic full repaint rewrote the whole
/// screen) and on every keystroke that also touched a header cell.
///
/// # How it works
///
/// This wrapper re-implements the cursor commands with `queue!`, so cells,
/// `ESC[?25h` and the caret `MoveTo` all accumulate in the writer, and pairs
/// with a [`BufWriter`] around stdout: `Terminal::draw`'s trailing
/// `backend.flush()` then pushes the entire frame through in one call. A
/// `BufWriter` flush larger than the `LineWriter`'s 1024-byte buffer is
/// passed straight to the fd as a single write, so both small frames (one
/// short write) and full-screen rewrites (one long write) arrive atomically.
/// Terminals parse the whole chunk before painting, and the hardware cursor
/// only ever rests at its final position.
///
/// Commands emitted outside `draw()` (`SetCursorStyle`, the OSC-12 cursor
/// color, terminal setup/teardown) still go through `io::stdout()` directly
/// — that is safe because the staging buffer is drained by the flush at the
/// end of every `draw()`, so nothing can interleave with those writes.
pub struct FrameBackend<W>
where
    W: Write,
{
    inner: CrosstermBackend<W>,
}

impl<W> FrameBackend<W>
where
    W: Write,
{
    /// Wraps a writer (normally a [`BufWriter`] over stdout) — see the type
    /// documentation for the required pairing.
    pub fn new(writer: W) -> Self {
        Self {
            inner: CrosstermBackend::new(writer),
        }
    }
}

/// Production backend: [`FrameBackend`] over a [`BufWriter`] over stdout.
pub fn stdout_frame_backend() -> FrameBackend<BufWriter<io::Stdout>> {
    FrameBackend::new(BufWriter::with_capacity(
        FRAME_BUFFER_CAPACITY,
        io::stdout(),
    ))
}

impl<W> Backend for FrameBackend<W>
where
    W: Write,
{
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        // The stock implementation already uses `queue!` — buffer the cell
        // content without flushing.
        self.inner.draw(content)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        // `queue!` instead of the stock `execute!`: no mid-frame flush. The
        // command travels through `Write for CrosstermBackend`, which
        // delegates to the wrapped writer.
        queue!(self.inner, Hide)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        queue!(self.inner, Show)
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let Position { x, y } = position.into();
        queue!(self.inner, MoveTo(x, y))
    }

    fn clear(&mut self) -> io::Result<()> {
        // `execute!`-based in the stock backend, but only invoked on
        // resize/`Terminal::clear` — a standalone screen-wide operation, not
        // part of the per-frame write stream.
        self.inner.clear()
    }

    fn size(&self) -> io::Result<Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        // The one real flush per frame: pushes the entire staged frame
        // (cells + cursor commands) out as a single write. Disambiguated to
        // the `Write` impl — `CrosstermBackend` also implements `Backend`,
        // which has its own `flush`.
        Write::flush(&mut self.inner)
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/app/frame_backend_test.rs"]
mod frame_backend_test;
