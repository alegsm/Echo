//! Eager text publication under the Windows clipboard lock. No lock survives a service wait.
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};
use windows_sys::core::w;
use windows_sys::Win32::Foundation::{GlobalFree, HGLOBAL, HWND};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE};

const CF_UNICODETEXT: u32 = 13;
const CLIPBOARD_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_CLIPBOARD_BYTES: usize = 4 * 1024 * 1024 + 2;

struct Clipboard(HWND);
impl Clipboard {
    fn open() -> Result<Self, ()> {
        // SAFETY: STATIC is a system class. The message-only owner lives on this worker thread.
        let window = unsafe {
            CreateWindowExW(
                0,
                w!("STATIC"),
                w!(""),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                null_mut(),
                null(),
            )
        };
        if window.is_null() {
            return Err(());
        }
        let started = Instant::now();
        loop {
            // SAFETY: window is a valid HWND owned by the current thread.
            if unsafe { OpenClipboard(window) } != 0 {
                return Ok(Self(window));
            }
            let remaining = CLIPBOARD_TIMEOUT.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                // SAFETY: window is the live HWND created on this thread; no Clipboard owns it.
                unsafe {
                    DestroyWindow(window);
                }
                return Err(());
            }
            std::thread::sleep(remaining.min(Duration::from_millis(5)));
        }
    }

    fn read(&self) -> Result<String, ()> {
        // SAFETY: the clipboard is open and this standard format uses a global memory handle.
        let memory = unsafe { GetClipboardData(CF_UNICODETEXT) };
        if memory.is_null() {
            return Err(());
        }
        // SAFETY: memory is the non-null global handle from the still-open clipboard.
        let size = unsafe { GlobalSize(memory) };
        if !(2..=MAX_CLIPBOARD_BYTES).contains(&size) || size % 2 != 0 {
            return Err(());
        }
        // SAFETY: memory remains owned by the clipboard while this Clipboard holds it open.
        let pointer = unsafe { GlobalLock(memory) } as *const u16;
        if pointer.is_null() {
            return Err(());
        }
        // SAFETY: GlobalSize bounds this locked allocation; a terminator is required inside it.
        let units = unsafe { std::slice::from_raw_parts(pointer, size / 2) };
        let text = units
            .iter()
            .position(|&unit| unit == 0)
            .ok_or(())
            .and_then(|end| String::from_utf16(&units[..end]).map_err(|_| ()));
        // SAFETY: this balances the successful GlobalLock after the last use of its slice.
        unsafe {
            GlobalUnlock(memory);
        }
        text
    }

    fn empty(&self) -> Result<(), ()> {
        // SAFETY: self holds the open clipboard on its owning thread.
        if unsafe { EmptyClipboard() } != 0 {
            Ok(())
        } else {
            Err(())
        }
    }
}
impl Drop for Clipboard {
    fn drop(&mut self) {
        // SAFETY: self owns the open clipboard and live HWND on this thread. Closing
        // releases the lock; published allocations belong to Windows, not this owner.
        unsafe {
            CloseClipboard();
            DestroyWindow(self.0);
        }
    }
}

struct Memory(HGLOBAL);
impl Memory {
    fn new(bytes: &[u8]) -> Result<Self, ()> {
        // SAFETY: GMEM_MOVEABLE requests a global allocation; NULL is checked before use.
        let memory = Self(unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) });
        if memory.0.is_null() {
            return Err(());
        }
        // SAFETY: memory owns the non-null allocation, which remains live until unlocked.
        let pointer = unsafe { GlobalLock(memory.0) } as *mut u8;
        if pointer.is_null() {
            return Err(());
        }
        // SAFETY: the allocation is locked and exactly bytes.len() bytes are available.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
            GlobalUnlock(memory.0);
        }
        Ok(memory)
    }

    fn publish(mut self, format: u32) -> Result<(), ()> {
        // SAFETY: the caller holds the open clipboard; self owns an unlocked global
        // allocation in the requested format. Success transfers ownership to Windows.
        if unsafe { SetClipboardData(format, self.0) }.is_null() {
            return Err(());
        }
        self.0 = null_mut();
        Ok(())
    }
}
impl Drop for Memory {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the non-null handle is still owned here; publication clears it
            // on transfer and every successful lock was balanced before drop.
            unsafe {
                GlobalFree(self.0);
            }
        }
    }
}

fn text_memory(text: &str) -> Result<Memory, ()> {
    let bytes: Vec<_> = text
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    Memory::new(&bytes)
}

pub fn snapshot() -> Result<String, ()> {
    Clipboard::open()?.read()
}

pub fn publish_protected(text: &str) -> Result<u64, ()> {
    let formats = [
        w!("ExcludeClipboardContentFromMonitorProcessing"),
        w!("CanIncludeInClipboardHistory"),
        w!("CanUploadToCloudClipboard"),
    ];
    let mut markers = Vec::with_capacity(3);
    for name in formats {
        // SAFETY: name points to a static NUL-terminated UTF-16 format name.
        let format = unsafe { RegisterClipboardFormatW(name) };
        if format == 0 {
            return Err(());
        }
        markers.push((format, Memory::new(&0u32.to_le_bytes())?));
    }
    let text = text_memory(text)?;
    let clipboard = Clipboard::open()?;
    let previous = clipboard.read().ok();
    let saved = previous.as_deref().map(text_memory).transpose()?;
    super::super::publish_clipboard(
        clipboard,
        previous.as_deref(),
        |clipboard| {
            clipboard.empty()?;
            // Publish every exclusion before text becomes available. Fail closed on marker failure.
            for (format, memory) in markers {
                memory.publish(format)?;
            }
            text.publish(CF_UNICODETEXT)?;
            Ok(())
        },
        |clipboard, _| {
            clipboard.empty()?;
            saved.ok_or(())?.publish(CF_UNICODETEXT)
        },
        // Another app can copy between close and this read; generation plus text checks limit,
        // but cannot eliminate, that race (as with NSPasteboard's check then write).
        // SAFETY: this process-wide sequence query takes no pointers or owned handles.
        || u64::from(unsafe { GetClipboardSequenceNumber() }),
    )
}

pub fn restore_if_owned(previous: &str, text: &str, expected: u64) -> Result<(), ()> {
    let previous = text_memory(previous)?;
    let clipboard = Clipboard::open()?;
    let current = clipboard.read()?;
    // SAFETY: this sequence query takes no pointers and may run with the clipboard open.
    let generation = unsafe { GetClipboardSequenceNumber() };
    if !super::super::clipboard_is_owned(expected, u64::from(generation), text, &current) {
        return Ok(());
    }
    // The ownership check and restoration share one OS lock, including an identical-text copy.
    clipboard.empty()?;
    previous.publish(CF_UNICODETEXT)
}
