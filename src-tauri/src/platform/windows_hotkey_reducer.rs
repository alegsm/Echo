//! Pure state for the Windows Control+Windows hook, also tested on other hosts.

use super::ModifierState;

const LEFT_CONTROL: usize = 0;
const RIGHT_CONTROL: usize = 1;
const LEFT_SHIFT: usize = 2;
const RIGHT_SHIFT: usize = 3;
const LEFT_ALT: usize = 4;
const RIGHT_ALT: usize = 5;
const LEFT_WINDOWS: usize = 6;
const RIGHT_WINDOWS: usize = 7;

pub(super) struct KeyInput {
    pub virtual_key: u16,
    pub scan_code: u32,
    pub extended: bool,
    pub is_down: bool,
    pub injected: bool,
}

#[derive(Debug, Default, PartialEq)]
pub(super) struct KeyResult {
    pub transition: Option<bool>,
    pub consume: bool,
    pub mask_start_menu: bool,
}

#[derive(Clone, Copy, Default)]
struct KeyState {
    down: bool,
    consumed: bool,
}

#[derive(Default)]
pub(super) struct HotkeyReducer {
    modifiers: [KeyState; 8],
    chord: ModifierState,
}

impl HotkeyReducer {
    pub fn set_capture_enabled(&mut self, enabled: bool) -> Option<bool> {
        self.chord.set_capture_enabled(enabled)
    }

    pub fn reset(&mut self) -> Option<bool> {
        self.modifiers = [KeyState::default(); 8];
        self.chord.release_pressed()
    }

    /// Sample order is left/right Control, Shift, Alt and Windows. Sampling cannot
    /// create a recording press, and a recovered key-up must pass through to Windows.
    pub fn resync(&mut self, snapshot: [bool; 8]) -> Option<bool> {
        let release = self.reset();
        self.modifiers = snapshot.map(|down| KeyState {
            down,
            consumed: false,
        });
        release
    }

    pub fn reduce(&mut self, input: KeyInput) -> KeyResult {
        // Synthetic input cannot trigger recording or change physical modifier state.
        if input.injected {
            return KeyResult::default();
        }
        let index = match input.virtual_key {
            0x11 if input.extended => RIGHT_CONTROL,
            0x11 | 0xA2 => LEFT_CONTROL,
            0xA3 => RIGHT_CONTROL,
            0x10 if input.scan_code == 0x36 => RIGHT_SHIFT,
            0x10 | 0xA0 => LEFT_SHIFT,
            0xA1 => RIGHT_SHIFT,
            0x12 if input.extended => RIGHT_ALT,
            0x12 | 0xA4 => LEFT_ALT,
            0xA5 => RIGHT_ALT,
            0x5B => LEFT_WINDOWS,
            0x5C => RIGHT_WINDOWS,
            _ => return KeyResult::default(),
        };
        let previous = self.modifiers[index];
        self.modifiers[index].down = input.is_down;
        let control_down = self.modifiers[LEFT_CONTROL].down || self.modifiers[RIGHT_CONTROL].down;
        let windows_down = self.modifiers[LEFT_WINDOWS].down || self.modifiers[RIGHT_WINDOWS].down;
        let pressed = control_down && windows_down;
        let chord = self.chord.update_pressed(pressed);
        let chord_key = matches!(
            index,
            LEFT_CONTROL | RIGHT_CONTROL | LEFT_WINDOWS | RIGHT_WINDOWS
        );

        // A prefix already forwarded to Windows must keep its repeats and release.
        // A swallowed down owns its repeats and up even after capture is disabled.
        let consume = if previous.down {
            previous.consumed
        } else {
            input.is_down && chord_key && pressed && chord.should_consume
        };
        self.modifiers[index].consumed = input.is_down && consume;

        // Mask at chord activation, while any forwarded Win key is still held.
        // Its eventual up must pass through so Windows cannot retain a stuck modifier.
        let mask_start_menu = chord.transition == Some(true)
            && [LEFT_WINDOWS, RIGHT_WINDOWS]
                .iter()
                .any(|&side| self.modifiers[side].down && !self.modifiers[side].consumed);
        KeyResult {
            transition: chord.transition,
            consume,
            mask_start_menu,
        }
    }
}

#[cfg(test)]
#[path = "windows_hotkey_reducer_tests.rs"]
mod hotkey_reducer_tests;
