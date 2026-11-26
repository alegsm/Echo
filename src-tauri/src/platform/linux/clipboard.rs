//! Text-only clipboard preservation on Linux.
//!
//! X11 and Wayland clipboards are served by the process that owns them, so Echo keeps one
//! long-lived clipboard handle: dropping it would clear a transcript the user has not
//! pasted yet. Neither protocol exposes a change counter, so the generation is Echo's own
//! publication count, and ownership falls back to comparing text. Another app's identical
//! copy therefore counts as Echo's, unlike on macOS and Windows.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use arboard::{Clipboard, SetExtLinux};

static CLIPBOARD: OnceLock<Mutex<Option<Clipboard>>> = OnceLock::new();
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn clipboard() -> Result<MutexGuard<'static, Option<Clipboard>>, ()> {
    let mut guard = CLIPBOARD
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if guard.is_none() {
        *guard = Some(Clipboard::new().map_err(|error| {
            log::warn!("Linux clipboard unavailable: {error}");
        })?);
    }
    Ok(guard)
}

fn read(board: &mut Clipboard) -> Result<String, ()> {
    board.get_text().map_err(|_| ())
}

fn write(board: &mut Clipboard, text: &str, protected: bool) -> Result<(), ()> {
    let set = board.set();
    // KDE Klipper and other managers honor x-kde-passwordManagerHint and skip the entry.
    let set = if protected {
        set.exclude_from_history()
    } else {
        set
    };
    set.text(text).map_err(|_| ())
}

pub fn snapshot() -> Result<String, ()> {
    let mut guard = clipboard()?;
    read(guard.as_mut().ok_or(())?)
}

pub fn publish_protected(text: &str) -> Result<u64, ()> {
    let mut guard = clipboard()?;
    let previous = guard.as_mut().and_then(|board| read(board).ok());
    super::super::publish_clipboard(
        RefCell::new(guard),
        previous.as_deref(),
        |guard| {
            let mut guard = guard.borrow_mut();
            write(guard.as_mut().ok_or(())?, text, true)?;
            GENERATION.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
        |guard, previous| {
            let mut guard = guard.borrow_mut();
            write(guard.as_mut().ok_or(())?, previous, false)
        },
        || GENERATION.load(Ordering::SeqCst),
    )
}

pub fn restore_if_owned(previous: &str, text: &str, expected: u64) -> Result<(), ()> {
    let mut guard = clipboard()?;
    let board = guard.as_mut().ok_or(())?;
    let current = read(board)?;
    let generation = GENERATION.load(Ordering::SeqCst);
    if !super::super::clipboard_is_owned(expected, generation, text, &current) {
        return Ok(());
    }
    write(board, previous, false)
}
