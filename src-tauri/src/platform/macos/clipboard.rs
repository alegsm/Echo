//! Text-only pasteboard preservation. Markers are prepared before transcript publication.
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};
use objc2_foundation::{NSArray, NSString};

fn pasteboard() -> Result<Retained<AnyObject>, ()> {
    // SAFETY: AppKit's generalPasteboard returns an NSPasteboard object, usable on this worker.
    let board: Option<Retained<AnyObject>> =
        unsafe { msg_send![class!(NSPasteboard), generalPasteboard] };
    board.ok_or(())
}

fn read(board: &AnyObject) -> Result<String, ()> {
    // SAFETY: stringForType: returns an NSString or nil for the UTF-8 text representation.
    let text: Option<Retained<NSString>> =
        unsafe { msg_send![board, stringForType: &*NSString::from_str("public.utf8-plain-text")] };
    text.map(|text| text.to_string()).ok_or(())
}

fn generation(board: &AnyObject) -> u64 {
    // SAFETY: changeCount is an NSInteger on NSPasteboard.
    let count: isize = unsafe { msg_send![board, changeCount] };
    count as u64
}

fn set_string(item: &AnyObject, text: &str, kind: &str) -> Result<(), ()> {
    // SAFETY: both arguments are live NSStrings and this object implements setString:forType:.
    let ok: Bool = unsafe {
        msg_send![item, setString: &*NSString::from_str(text), forType: &*NSString::from_str(kind)]
    };
    if ok.as_bool() {
        Ok(())
    } else {
        Err(())
    }
}

fn protected_item(text: &str) -> Result<Retained<AnyObject>, ()> {
    // SAFETY: NSPasteboardItem's new returns a retained, initialized item.
    let item: Retained<AnyObject> = unsafe { msg_send![class!(NSPasteboardItem), new] };
    set_string(&item, "", "org.nspasteboard.TransientType")?;
    set_string(&item, "", "org.nspasteboard.ConcealedType")?;
    set_string(&item, text, "public.utf8-plain-text")?;
    Ok(item)
}

pub fn snapshot() -> Result<String, ()> {
    autoreleasepool(|_| read(&*pasteboard()?))
}

pub fn publish_protected(text: &str) -> Result<u64, ()> {
    autoreleasepool(|_| {
        let item = protected_item(text)?;
        let board = pasteboard()?;
        let items = NSArray::from_slice(&[&*item]);
        let previous = read(&board).ok();
        super::super::publish_clipboard(
            board.clone(),
            previous.as_deref(),
            |board| {
                // SAFETY: all item representations exist before writeObjects: publishes them.
                let ok: Bool = unsafe {
                    let _: isize = msg_send![&**board, clearContents];
                    msg_send![&**board, writeObjects: &*items]
                };
                if ok.as_bool() {
                    Ok(())
                } else {
                    Err(())
                }
            },
            |board, previous| {
                // SAFETY: board retains a live NSPasteboard implementing clearContents.
                unsafe {
                    let _: isize = msg_send![&**board, clearContents];
                }
                set_string(board, previous, "public.utf8-plain-text")
            },
            || generation(&board),
        )
    })
}

pub fn restore_if_owned(previous: &str, text: &str, expected: u64) -> Result<(), ()> {
    autoreleasepool(|_| {
        let board = pasteboard()?;
        let current = read(&board)?;
        if !super::super::clipboard_is_owned(expected, generation(&board), text, &current) {
            return Ok(());
        }
        // NSPasteboard has no compare-and-swap. A foreign write between this check and clear can race.
        // SAFETY: board retains the NSPasteboard through this synchronous clearContents call.
        unsafe {
            let _: isize = msg_send![&*board, clearContents];
        }
        set_string(&board, previous, "public.utf8-plain-text")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // Keep: inspect a real local NSPasteboardItem without changing the system clipboard.
    #[test]
    fn clipboard_macos_item_contains_both_privacy_markers_before_publication() {
        autoreleasepool(|_| {
            let item = protected_item("transcript").unwrap();
            for kind in [
                "org.nspasteboard.TransientType",
                "org.nspasteboard.ConcealedType",
                "public.utf8-plain-text",
            ] {
                // SAFETY: item is a retained NSPasteboardItem; the live NSString names a
                // representation whose stringForType: result is an NSString or nil.
                let value: Option<Retained<NSString>> =
                    unsafe { msg_send![&*item, stringForType: &*NSString::from_str(kind)] };
                assert!(value.is_some(), "missing {kind}");
            }
        });
    }
}
