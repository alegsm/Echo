//! X11 input through x11rb: XInput2 raw key events for the chord, XTest for paste.

use std::collections::HashSet;
use std::os::fd::{AsRawFd, RawFd};
use std::time::Duration;

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, Window, KEY_PRESS_EVENT, KEY_RELEASE_EVENT,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::CURRENT_TIME;

use super::hotkey::{self, ChordKeys};

const XK_SHIFT_L: u32 = 0xffe1;
const XK_SHIFT_R: u32 = 0xffe2;
const XK_CONTROL_L: u32 = 0xffe3;
const XK_CONTROL_R: u32 = 0xffe4;
const XK_META_L: u32 = 0xffe7;
const XK_META_R: u32 = 0xffe8;
const XK_ALT_L: u32 = 0xffe9;
const XK_ALT_R: u32 = 0xffea;
const XK_SUPER_L: u32 = 0xffeb;
const XK_SUPER_R: u32 = 0xffec;
const XK_ISO_LEVEL3_SHIFT: u32 = 0xfe03;
const XK_LOWER_V: u32 = 0x0076;
const XK_UPPER_V: u32 = 0x0056;

/// Chord keys in `ChordKeys` order.
const CHORD_KEYSYMS: [u32; 4] = [XK_CONTROL_L, XK_CONTROL_R, XK_SUPER_L, XK_SUPER_R];
/// Modifiers released around the synthetic paste so a held key cannot change its meaning.
const HELD_MODIFIER_KEYSYMS: [u32; 11] = [
    XK_SHIFT_L,
    XK_SHIFT_R,
    XK_CONTROL_L,
    XK_CONTROL_R,
    XK_META_L,
    XK_META_R,
    XK_ALT_L,
    XK_ALT_R,
    XK_SUPER_L,
    XK_SUPER_R,
    XK_ISO_LEVEL3_SHIFT,
];

/// WM_CLASS fragments of terminals, which paste with Control+Shift+V instead of Control+V.
const TERMINAL_CLASSES: [&str; 18] = [
    "terminal",
    "konsole",
    "alacritty",
    "kitty",
    "tilix",
    "terminator",
    "wezterm",
    "foot",
    "footclient",
    "ghostty",
    "terminology",
    "guake",
    "yakuake",
    "tilda",
    "kgx",
    "ptyxis",
    "sakura",
    "st-256color",
];

/// Xvfb has no physical keyboard, so the X11 integration test drives keys through XTEST.
#[cfg(test)]
pub(super) static ACCEPT_SYNTHETIC: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn accepts_synthetic() -> bool {
    #[cfg(test)]
    return ACCEPT_SYNTHETIC.load(std::sync::atomic::Ordering::SeqCst);
    #[cfg(not(test))]
    false
}

fn connect() -> Result<(RustConnection, Window), String> {
    let (conn, screen) =
        x11rb::connect(None).map_err(|error| format!("cannot open the X display: {error}"))?;
    let root = conn
        .setup()
        .roots
        .get(screen)
        .map(|screen| screen.root)
        .ok_or("the X display has no such screen")?;
    Ok((conn, root))
}

fn x_error(error: impl std::fmt::Display) -> String {
    format!("X11 request failed: {error}")
}

struct Keymap {
    min_keycode: u8,
    per_keycode: usize,
    keysyms: Vec<u32>,
}

impl Keymap {
    fn load(conn: &impl Connection) -> Result<Self, String> {
        let setup = conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let reply = conn
            .get_keyboard_mapping(min, max - min + 1)
            .map_err(x_error)?
            .reply()
            .map_err(x_error)?;
        Ok(Self {
            min_keycode: min,
            per_keycode: usize::from(reply.keysyms_per_keycode).max(1),
            keysyms: reply.keysyms,
        })
    }

    fn keycodes(&self, keysym: u32) -> Vec<u8> {
        self.keysyms
            .chunks(self.per_keycode)
            .enumerate()
            .filter(|(_, syms)| syms.contains(&keysym))
            .filter_map(|(offset, _)| u8::try_from(usize::from(self.min_keycode) + offset).ok())
            .collect()
    }

    fn chord_index(&self) -> [Option<usize>; 256] {
        let mut index = [None; 256];
        for (position, keysym) in CHORD_KEYSYMS.iter().enumerate() {
            for keycode in self.keycodes(*keysym) {
                index[usize::from(keycode)] = Some(position);
            }
        }
        index
    }
}

fn is_down(keys: &[u8], keycode: u8) -> bool {
    keys.get(usize::from(keycode / 8))
        .is_some_and(|byte| byte & (1 << (keycode % 8)) != 0)
}

/// Devices whose events are synthetic, such as Echo's own XTest paste.
fn xtest_devices(conn: &impl Connection) -> Result<HashSet<u16>, String> {
    let reply = conn
        .xinput_xi_query_device(xinput::Device::ALL)
        .map_err(x_error)?
        .reply()
        .map_err(x_error)?;
    Ok(reply
        .infos
        .iter()
        .filter(|info| String::from_utf8_lossy(&info.name).contains("XTEST"))
        .map(|info| info.deviceid)
        .collect())
}

/// Observes Control and Super until `wake` becomes readable. Returns Ok only when woken.
pub(super) fn run_hotkeys(wake: RawFd, ready: &dyn Fn()) -> Result<(), String> {
    let (conn, root) = connect()?;
    if conn
        .extension_information(xinput::X11_EXTENSION_NAME)
        .map_err(x_error)?
        .is_none()
    {
        return Err("the X server lacks the XInputExtension".into());
    }
    let version = conn
        .xinput_xi_query_version(2, 0)
        .map_err(x_error)?
        .reply()
        .map_err(x_error)?;
    if version.major_version < 2 {
        return Err("the X server lacks XInput 2".into());
    }
    let mut keymap = Keymap::load(&conn)?;
    let mut chord_index = keymap.chord_index();
    if !chord_index.iter().flatten().any(|&i| i < 2)
        || !chord_index.iter().flatten().any(|&i| i >= 2)
    {
        return Err("the keyboard map has no Control or Super key".into());
    }
    let mut synthetic = xtest_devices(&conn)?;
    conn.xinput_xi_select_events(
        root,
        &[xinput::EventMask {
            deviceid: xinput::Device::ALL_MASTER.into(),
            mask: vec![xinput::XIEventMask::RAW_KEY_PRESS | xinput::XIEventMask::RAW_KEY_RELEASE],
        }],
    )
    .map_err(x_error)?
    .check()
    .map_err(x_error)?;

    let pressed = conn
        .query_keymap()
        .map_err(x_error)?
        .reply()
        .map_err(x_error)?;
    let mut keys: ChordKeys = [false; 4];
    for keycode in 0..=u8::MAX {
        if let Some(position) = chord_index[usize::from(keycode)] {
            keys[position] |= is_down(&pressed.keys, keycode);
        }
    }
    ready();
    hotkey::report(keys);

    let socket = conn.stream().as_raw_fd();
    loop {
        conn.flush().map_err(x_error)?;
        while let Some(event) = conn.poll_for_event().map_err(x_error)? {
            let (event, is_down) = match event {
                Event::XinputRawKeyPress(event) => (event, true),
                Event::XinputRawKeyRelease(event) => (event, false),
                Event::MappingNotify(_) => {
                    keymap = Keymap::load(&conn)?;
                    chord_index = keymap.chord_index();
                    synthetic = xtest_devices(&conn)?;
                    continue;
                }
                _ => continue,
            };
            if synthetic.contains(&event.sourceid) && !accepts_synthetic() {
                continue;
            }
            let position = usize::try_from(event.detail)
                .ok()
                .and_then(|keycode| chord_index.get(keycode).copied().flatten());
            if let Some(position) = position {
                if keys[position] != is_down {
                    keys[position] = is_down;
                    hotkey::report(keys);
                }
            }
        }
        let readable = hotkey::wait_readable(&[socket, wake], -1)
            .map_err(|error| format!("waiting for X events failed: {error}"))?;
        if readable[1] {
            return Ok(());
        }
    }
}

fn focused_window_is_terminal(conn: &impl Connection, root: Window) -> Option<bool> {
    let active_atom = conn
        .intern_atom(true, b"_NET_ACTIVE_WINDOW")
        .ok()?
        .reply()
        .ok()?
        .atom;
    let active = conn
        .get_property(false, root, active_atom, AtomEnum::WINDOW, 0, 1)
        .ok()?
        .reply()
        .ok()?
        .value32()?
        .next()?;
    let class = conn
        .get_property(false, active, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)
        .ok()?
        .reply()
        .ok()?
        .value;
    Some(is_terminal_class(&String::from_utf8_lossy(&class)))
}

fn is_terminal_part(part: &str, terminal: &str) -> bool {
    if terminal == "terminal" {
        return part.contains(terminal);
    }
    part == terminal
        || part
            .strip_suffix(terminal)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn is_terminal_class(wm_class: &str) -> bool {
    let wm_class = wm_class.to_ascii_lowercase();
    wm_class.split('\0').any(|part| {
        TERMINAL_CLASSES
            .iter()
            .any(|&terminal| is_terminal_part(part, terminal))
    })
}

/// Presses Control+V (Control+Shift+V in a terminal) into the focused X11 window.
pub(super) fn paste() -> Result<(), String> {
    let (conn, root) = connect()?;
    conn.xtest_get_version(2, 2)
        .map_err(x_error)?
        .reply()
        .map_err(|_| "the X server lacks the XTEST extension".to_string())?;
    let keymap = Keymap::load(&conn)?;
    let first = |keysym: u32| keymap.keycodes(keysym).first().copied();
    let control = first(XK_CONTROL_L)
        .or_else(|| first(XK_CONTROL_R))
        .ok_or("the keyboard map has no Control key")?;
    let v = first(XK_LOWER_V)
        .or_else(|| first(XK_UPPER_V))
        .ok_or("the keyboard map has no V key")?;
    let shift = if focused_window_is_terminal(&conn, root).unwrap_or(false) {
        Some(first(XK_SHIFT_L).ok_or("the keyboard map has no Shift key")?)
    } else {
        None
    };

    let pressed = conn
        .query_keymap()
        .map_err(x_error)?
        .reply()
        .map_err(x_error)?;
    let mut held: Vec<u8> = HELD_MODIFIER_KEYSYMS
        .iter()
        .flat_map(|&keysym| keymap.keycodes(keysym))
        .filter(|&keycode| is_down(&pressed.keys, keycode))
        .collect();
    held.sort_unstable();
    held.dedup();

    let mut events: Vec<(u8, bool)> = held.iter().map(|&key| (key, false)).collect();
    events.push((control, true));
    events.extend(shift.map(|key| (key, true)));
    events.extend([(v, true), (v, false)]);
    events.extend(shift.map(|key| (key, false)));
    events.push((control, false));
    events.extend(held.iter().map(|&key| (key, true)));

    for (keycode, is_press) in events {
        let kind = if is_press {
            KEY_PRESS_EVENT
        } else {
            KEY_RELEASE_EVENT
        };
        conn.xtest_fake_input(kind, keycode, CURRENT_TIME, root, 0, 0, 0)
            .map_err(x_error)?;
        conn.flush().map_err(x_error)?;
        // Some toolkits drop modifier state from events that arrive in the same instant.
        std::thread::sleep(Duration::from_millis(4));
    }
    // A round trip guarantees the server processed every fake event before returning.
    conn.get_input_focus()
        .map_err(x_error)?
        .reply()
        .map_err(x_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymap_finds_every_keycode_for_a_keysym() {
        let keymap = Keymap {
            min_keycode: 8,
            per_keycode: 2,
            keysyms: vec![
                0,
                0,
                XK_CONTROL_L,
                0,
                XK_LOWER_V,
                XK_UPPER_V,
                XK_CONTROL_L,
                0,
            ],
        };
        assert_eq!(keymap.keycodes(XK_CONTROL_L), vec![9, 11]);
        assert_eq!(keymap.keycodes(XK_UPPER_V), vec![10]);
        assert_eq!(keymap.chord_index()[11], Some(0));
    }

    #[test]
    fn keymap_bitmap_reads_bits_little_endian_within_bytes() {
        let mut keys = [0u8; 32];
        keys[4] = 0b0000_0010;
        assert!(is_down(&keys, 33));
        assert!(!is_down(&keys, 32));
    }

    #[test]
    fn terminals_are_recognized_by_either_wm_class_part() {
        assert!(is_terminal_class("gnome-terminal-server\0Gnome-terminal\0"));
        assert!(is_terminal_class("Alacritty\0Alacritty\0"));
        assert!(is_terminal_class(
            "org.wezfurlong.wezterm\0org.wezfurlong.wezterm\0"
        ));
        assert!(!is_terminal_class("Navigator\0firefox\0"));
        assert!(!is_terminal_class("gedit\0Gedit\0"));
        assert!(is_terminal_class("footclient\0footclient\0"));
        assert!(is_terminal_class("org.gnome.Ptyxis\0Org.gnome.Ptyxis\0"));
        assert!(!is_terminal_class("bigfoot\0Bigfoot\0"));
        assert!(!is_terminal_class("kittycam\0Kittycam\0"));
    }
}
