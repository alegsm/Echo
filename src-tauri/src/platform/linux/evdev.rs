//! Reads the Control+Super chord straight from `/dev/input/event*`.
//!
//! This is the Wayland path, where compositors hide global key state from clients. It works
//! only for users allowed to read input devices (usually the `input` group). Devices are
//! opened read-only and never grabbed. New keyboards are picked up every few seconds.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::hotkey::{self, ChordKeys};

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const SYN_REPORT: u16 = 0;
const SYN_DROPPED: u16 = 3;
const KEY_LEFTCTRL: u16 = 29;
const KEY_RIGHTCTRL: u16 = 97;
const KEY_LEFTMETA: u16 = 125;
const KEY_RIGHTMETA: u16 = 126;
const KEY_MAX: usize = 0x2ff;
const RESCAN_INTERVAL: Duration = Duration::from_secs(3);
const CHORD_CODES: [u16; 4] = [KEY_LEFTCTRL, KEY_RIGHTCTRL, KEY_LEFTMETA, KEY_RIGHTMETA];
const PASTE_HELPER_DEVICE: &str = "ydotoold virtual device";

fn chord_position(code: u16) -> Option<usize> {
    match code {
        KEY_LEFTCTRL => Some(0),
        KEY_RIGHTCTRL => Some(1),
        KEY_LEFTMETA => Some(2),
        KEY_RIGHTMETA => Some(3),
        _ => None,
    }
}

struct Device {
    file: File,
    keys: ChordKeys,
    dropped: bool,
}

impl Device {
    fn new(file: File) -> Self {
        Self {
            file,
            keys: [false; 4],
            dropped: false,
        }
    }
}

fn evioc_read(number: libc::c_ulong, len: usize) -> libc::c_ulong {
    const IOC_READ: libc::c_ulong = 2;
    (IOC_READ << 30) | ((len as libc::c_ulong) << 16) | ((b'E' as libc::c_ulong) << 8) | number
}

/// EVIOCGBIT(EV_KEY, len): which key codes a device can report.
fn eviocgbit_key(len: usize) -> libc::c_ulong {
    evioc_read(0x20 + libc::c_ulong::from(EV_KEY), len)
}

fn eviocgname(len: usize) -> libc::c_ulong {
    evioc_read(0x06, len)
}

fn eviocgkey(len: usize) -> libc::c_ulong {
    evioc_read(0x18, len)
}

fn device_name(file: &File) -> String {
    let mut name = [0u8; 256];
    // SAFETY: name is writable storage of exactly the length encoded in the request.
    let result = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            eviocgname(name.len()) as _,
            name.as_mut_ptr(),
        )
    };
    if result < 0 {
        return String::new();
    }
    let end = name
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(name.len());
    String::from_utf8_lossy(&name[..end]).into_owned()
}

fn resync(device: &mut Device) {
    let mut bits = [0u8; KEY_MAX / 8 + 1];
    // SAFETY: bits is writable storage of exactly the length encoded in the request.
    let result = unsafe {
        libc::ioctl(
            device.file.as_raw_fd(),
            eviocgkey(bits.len()) as _,
            bits.as_mut_ptr(),
        )
    };
    for (down, code) in device.keys.iter_mut().zip(CHORD_CODES) {
        *down = result >= 0 && has_key(&bits, code);
    }
}

fn has_key(bits: &[u8], code: u16) -> bool {
    bits.get(usize::from(code) / 8)
        .is_some_and(|byte| byte & (1 << (code % 8)) != 0)
}

fn is_chord_keyboard(file: &File) -> bool {
    let mut bits = [0u8; KEY_MAX / 8 + 1];
    // SAFETY: bits is writable storage of exactly the length encoded in the request.
    let result = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            eviocgbit_key(bits.len()) as _,
            bits.as_mut_ptr(),
        )
    };
    result >= 0
        && (has_key(&bits, KEY_LEFTCTRL) || has_key(&bits, KEY_RIGHTCTRL))
        && (has_key(&bits, KEY_LEFTMETA) || has_key(&bits, KEY_RIGHTMETA))
}

/// Opens newly appeared keyboards. Returns how many event nodes exist but are unreadable.
fn scan(devices: &mut HashMap<PathBuf, Device>, directory: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return 0;
    };
    let mut denied = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_event_node = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("event"));
        if !is_event_node || devices.contains_key(&path) {
            continue;
        }
        match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&path)
        {
            Ok(file) if is_chord_keyboard(&file) && device_name(&file) != PASTE_HELPER_DEVICE => {
                devices.insert(path, Device::new(file));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => denied += 1,
            Err(_) => {}
        }
    }
    denied
}

/// Drains one device. Returns false when the device is gone.
fn drain(device: &mut Device) -> bool {
    const EVENT_SIZE: usize = std::mem::size_of::<libc::input_event>();
    // SAFETY: input_event is plain old data, so an all-zero array is a valid value.
    let mut events: [libc::input_event; 64] = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: events is writable storage of the byte length passed to read.
        let read = unsafe {
            libc::read(
                device.file.as_raw_fd(),
                events.as_mut_ptr().cast(),
                EVENT_SIZE * events.len(),
            )
        };
        if read < 0 {
            let error = std::io::Error::last_os_error();
            return matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
            );
        }
        if read == 0 {
            return false;
        }
        for event in &events[..read as usize / EVENT_SIZE] {
            if event.type_ == EV_SYN {
                if event.code == SYN_DROPPED {
                    device.dropped = true;
                } else if event.code == SYN_REPORT && device.dropped {
                    device.dropped = false;
                    resync(device);
                }
                continue;
            }
            if device.dropped || event.type_ != EV_KEY || event.value == 2 {
                continue;
            }
            if let Some(position) = chord_position(event.code) {
                device.keys[position] = event.value != 0;
            }
        }
    }
}

fn combined(devices: &HashMap<PathBuf, Device>) -> ChordKeys {
    let mut keys = [false; 4];
    for device in devices.values() {
        for (combined, down) in keys.iter_mut().zip(device.keys) {
            *combined |= down;
        }
    }
    keys
}

/// Observes Control and Super until `wake` becomes readable. Returns Ok only when woken.
pub(super) fn run_hotkeys(wake: RawFd, ready: &dyn Fn()) -> Result<(), String> {
    let directory = Path::new("/dev/input");
    let mut devices = HashMap::new();
    let denied = scan(&mut devices, directory);
    if devices.is_empty() {
        return Err(if denied > 0 {
            "no permission to read /dev/input; add your user to the input group and sign in again"
                .to_string()
        } else {
            "no keyboard with Control and Super keys was found".to_string()
        });
    }
    ready();
    let mut reported = [false; 4];
    hotkey::report(reported);
    let mut scanned = Instant::now();
    loop {
        let paths: Vec<PathBuf> = devices.keys().cloned().collect();
        let mut fds: Vec<RawFd> = paths
            .iter()
            .map(|path| devices[path].file.as_raw_fd())
            .collect();
        fds.push(wake);
        let readable = hotkey::wait_readable(&fds, RESCAN_INTERVAL.as_millis() as i32)
            .map_err(|error| format!("waiting for input events failed: {error}"))?;
        if readable[paths.len()] {
            return Ok(());
        }
        for (path, is_readable) in paths.iter().zip(&readable) {
            if !is_readable {
                continue;
            }
            let alive = devices.get_mut(path).is_some_and(drain);
            if !alive {
                devices.remove(path);
            }
        }
        if scanned.elapsed() >= RESCAN_INTERVAL {
            scan(&mut devices, directory);
            scanned = Instant::now();
        }
        let keys = combined(&devices);
        if keys != reported {
            reported = keys;
            hotkey::report(keys);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviocgbit_matches_the_kernel_encoding() {
        // EVIOCGBIT(EV_KEY, 96) from <linux/input.h>.
        assert_eq!(eviocgbit_key(96), 0x8060_4521);
    }

    #[test]
    fn name_and_key_state_requests_match_the_kernel_encoding() {
        assert_eq!(eviocgname(256), 0x8100_4506);
        assert_eq!(eviocgkey(96), 0x8060_4518);
    }

    #[test]
    fn chord_codes_follow_chord_positions() {
        for (position, code) in CHORD_CODES.into_iter().enumerate() {
            assert_eq!(chord_position(code), Some(position));
        }
    }

    #[test]
    fn events_after_a_drop_are_discarded_and_a_failed_resync_releases_keys() {
        let mut device = device_fed_with(&[
            event(EV_KEY, KEY_LEFTCTRL, 1),
            event(EV_SYN, SYN_DROPPED, 0),
            event(EV_KEY, KEY_LEFTMETA, 1),
            event(EV_SYN, SYN_REPORT, 0),
        ]);
        drain(&mut device);
        assert_eq!(device.keys, [false; 4]);
        assert!(!device.dropped);
    }

    #[test]
    fn chord_positions_follow_control_then_super() {
        assert_eq!(chord_position(KEY_LEFTCTRL), Some(0));
        assert_eq!(chord_position(KEY_RIGHTMETA), Some(3));
        assert_eq!(chord_position(30), None);
    }

    fn event(kind: u16, code: u16, value: i32) -> libc::input_event {
        // SAFETY: input_event is plain old data, so an all-zero value is valid.
        let mut event: libc::input_event = unsafe { std::mem::zeroed() };
        event.type_ = kind;
        event.code = code;
        event.value = value;
        event
    }

    /// A pipe stands in for an event node: the kernel delivers whole input_event records.
    fn device_fed_with(events: &[libc::input_event]) -> Device {
        use std::io::Write;
        use std::os::fd::FromRawFd;
        let mut fds = [0; 2];
        // SAFETY: fds is writable storage for the two descriptors pipe2 returns.
        let created = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK) };
        assert_eq!(created, 0);
        // SAFETY: pipe2 succeeded, so both descriptors are open and owned here.
        let (reader, mut writer) =
            unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
        // SAFETY: the slice is initialized input_event records viewed as bytes.
        let bytes = unsafe {
            std::slice::from_raw_parts(events.as_ptr().cast::<u8>(), std::mem::size_of_val(events))
        };
        writer.write_all(bytes).unwrap();
        drop(writer);
        Device::new(reader)
    }

    #[test]
    fn drain_tracks_chord_keys_and_ignores_repeats_and_other_events() {
        let mut device = device_fed_with(&[
            event(EV_KEY, KEY_LEFTCTRL, 1),
            event(0x00, 0, 0),    // EV_SYN
            event(EV_KEY, 30, 1), // A
            event(EV_KEY, KEY_RIGHTMETA, 1),
            event(EV_KEY, KEY_RIGHTMETA, 2), // autorepeat
            event(0x04, 4, 125),             // EV_MSC scan code that happens to equal KEY_LEFTMETA
        ]);
        // The writer is closed, so the pipe reports end of file: the device is gone.
        assert!(!drain(&mut device));
        assert_eq!(device.keys, [true, false, false, true]);
    }

    #[test]
    fn chord_state_combines_keyboards() {
        let mut devices = HashMap::new();
        let mut left = device_fed_with(&[event(EV_KEY, KEY_LEFTCTRL, 1)]);
        let mut right = device_fed_with(&[
            event(EV_KEY, KEY_LEFTMETA, 1),
            event(EV_KEY, KEY_LEFTMETA, 0),
        ]);
        drain(&mut left);
        drain(&mut right);
        devices.insert(PathBuf::from("event0"), left);
        devices.insert(PathBuf::from("event1"), right);
        assert_eq!(combined(&devices), [true, false, false, false]);
    }

    #[test]
    fn key_bits_are_indexed_by_code() {
        let mut bits = [0u8; KEY_MAX / 8 + 1];
        bits[usize::from(KEY_LEFTMETA) / 8] |= 1 << (KEY_LEFTMETA % 8);
        assert!(has_key(&bits, KEY_LEFTMETA));
        assert!(!has_key(&bits, KEY_RIGHTMETA));
    }
}
