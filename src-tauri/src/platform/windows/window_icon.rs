//! Taskbar and Alt-Tab icons loaded from the executable's multi-size icon resource.
//!
//! Tauri sets only `ICON_SMALL`, from the first frame of `icon.ico`, so Windows stretches
//! that frame for the taskbar and Alt-Tab. This module loads the embedded resource at the
//! window's system-metric sizes and sets both `ICON_BIG` and `ICON_SMALL`.
use std::ptr::null;

use windows_sys::core::PCWSTR;
use windows_sys::Win32::Foundation::{HMODULE, HWND};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetSystemMetrics, IsWindow, LoadImageW, SendMessageW, HICON, ICON_BIG, ICON_SMALL,
    IMAGE_ICON, LR_DEFAULTCOLOR, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON,
    SYSTEM_METRICS_INDEX, WM_SETICON,
};

/// tauri-build embeds the Windows `bundle.icon` (`icons/windows/icon.ico`) under this ID,
/// which is also `IDI_APPLICATION`.
const EXECUTABLE_ICON_ID: u16 = 32512;

// These small API boundaries avoid enabling unrelated windows-sys feature groups.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: PCWSTR) -> HMODULE;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn GetDpiForWindow(window: HWND) -> u32;
    fn GetSystemMetricsForDpi(index: SYSTEM_METRICS_INDEX, dpi: u32) -> i32;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IconSize {
    width: i32,
    height: i32,
}

/// Big and small icon sizes for `dpi`, or the system DPI when the window reports none.
fn icon_sizes(
    dpi: u32,
    for_dpi: impl Fn(SYSTEM_METRICS_INDEX, u32) -> i32,
    system: impl Fn(SYSTEM_METRICS_INDEX) -> i32,
) -> Option<(IconSize, IconSize)> {
    let metric = |index| {
        if dpi == 0 {
            system(index)
        } else {
            for_dpi(index, dpi)
        }
    };
    let size = |width, height| {
        let size = IconSize {
            width: metric(width),
            height: metric(height),
        };
        (size.width > 0 && size.height > 0).then_some(size)
    };
    Some((size(SM_CXICON, SM_CYICON)?, size(SM_CXSMICON, SM_CYSMICON)?))
}

/// An icon this process loaded without `LR_SHARED` and therefore must destroy.
struct OwnedIcon(HICON);

impl OwnedIcon {
    fn load(module: HMODULE, id: u16, size: IconSize) -> Result<Self, ()> {
        // SAFETY: an integer resource ID is a valid MAKEINTRESOURCE name, and a null or
        // missing module or resource makes LoadImageW return null rather than fault.
        let icon = unsafe {
            LoadImageW(
                module,
                usize::from(id) as PCWSTR,
                IMAGE_ICON,
                size.width,
                size.height,
                LR_DEFAULTCOLOR,
            )
        };
        if icon.is_null() {
            return Err(());
        }
        Ok(Self(icon))
    }
}

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        // SAFETY: the handle came from LoadImageW without LR_SHARED, and owners drop it only
        // after the window has switched to another icon or no longer exists.
        unsafe { DestroyIcon(self.0) };
    }
}

/// The executable icons currently set on one top-level window.
pub struct WindowIcons {
    window: HWND,
    big: Option<OwnedIcon>,
    small: Option<OwnedIcon>,
}

// SAFETY: window and icon handles are process-wide values. WM_SETICON from another thread is
// marshalled to the window's thread, and DestroyIcon has no thread affinity.
unsafe impl Send for WindowIcons {}

impl WindowIcons {
    pub fn new(window: HWND) -> Self {
        Self {
            window,
            big: None,
            small: None,
        }
    }

    /// Loads both icons at the window's current DPI and sets them. On any failure the window
    /// keeps the icons it already has.
    pub fn apply(&mut self) {
        // SAFETY: GetDpiForWindow returns 0 for an invalid handle; metrics calls take values.
        let dpi = unsafe { GetDpiForWindow(self.window) };
        let Some((big_size, small_size)) = icon_sizes(
            dpi,
            // SAFETY: metric indices are system constants and the DPI is a plain value.
            |index, dpi| unsafe { GetSystemMetricsForDpi(index, dpi) },
            // SAFETY: metric indices are system constants.
            |index| unsafe { GetSystemMetrics(index) },
        ) else {
            log::warn!("Windows reported no icon size; the window keeps its current icons");
            return;
        };
        // SAFETY: a null name returns the executable's module handle without a reference count.
        let module = unsafe { GetModuleHandleW(null()) };
        let (Ok(big), Ok(small)) = (
            OwnedIcon::load(module, EXECUTABLE_ICON_ID, big_size),
            OwnedIcon::load(module, EXECUTABLE_ICON_ID, small_size),
        ) else {
            log::debug!("Executable icon resource unavailable; the window keeps its current icons");
            return;
        };
        self.big = Some(self.set(ICON_BIG, big));
        self.small = Some(self.set(ICON_SMALL, small));
    }

    /// Sets `icon` and returns it for storage; the previously stored icon is dropped by the
    /// caller's assignment only after the window has stopped using it.
    fn set(&self, kind: u32, icon: OwnedIcon) -> OwnedIcon {
        // SAFETY: WM_SETICON takes an icon handle that stays alive while stored in `self`.
        // The returned previous icon is either Tauri's, which Tauri still owns, or ours.
        unsafe { SendMessageW(self.window, WM_SETICON, kind as usize, icon.0 as isize) };
        icon
    }

    /// Destroys the stored icons once the window no longer exists.
    pub fn release(&mut self) {
        self.big = None;
        self.small = None;
    }
}

impl Drop for WindowIcons {
    fn drop(&mut self) {
        // SAFETY: IsWindow accepts any handle value.
        if unsafe { IsWindow(self.window) } != 0 {
            // A live window may still draw these icons; process exit reclaims them instead.
            std::mem::forget(self.big.take());
            std::mem::forget(self.small.take());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metric(index: SYSTEM_METRICS_INDEX) -> i32 {
        match index {
            SM_CXICON => 32,
            SM_CYICON => 33,
            SM_CXSMICON => 16,
            SM_CYSMICON => 17,
            _ => 0,
        }
    }

    #[test]
    fn sizes_follow_the_window_dpi_when_known() {
        let sizes = icon_sizes(
            144,
            |index, dpi| metric(index) * dpi as i32 / 96,
            |_| panic!("the system DPI is used only when the window reports none"),
        );

        assert_eq!(
            sizes,
            Some((
                IconSize {
                    width: 48,
                    height: 49
                },
                IconSize {
                    width: 24,
                    height: 25
                }
            ))
        );
    }

    #[test]
    fn sizes_fall_back_to_system_metrics_without_a_window_dpi() {
        let sizes = icon_sizes(0, |_, _| panic!("no window DPI"), metric);

        assert_eq!(
            sizes.map(|(big, small)| (big.width, small.height)),
            Some((32, 17))
        );
    }

    #[test]
    fn a_missing_metric_keeps_the_current_icons() {
        let sizes = icon_sizes(
            96,
            |index, _| if index == SM_CYSMICON { 0 } else { 32 },
            metric,
        );

        assert_eq!(sizes, None);
    }

    #[test]
    fn a_missing_icon_resource_is_a_recoverable_error() {
        // SAFETY: a null name returns this test executable's module handle.
        let module = unsafe { GetModuleHandleW(null()) };
        let size = IconSize {
            width: 32,
            height: 32,
        };

        // Unit-test executables link no resource script, so no icon exists under this ID.
        assert!(OwnedIcon::load(module, 1, size).is_err());
    }
}
