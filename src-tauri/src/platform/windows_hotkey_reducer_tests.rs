use super::{HotkeyReducer, KeyInput, KeyResult};

const LEFT_CTRL: u16 = 0xA2;
const RIGHT_CTRL: u16 = 0xA3;
const LEFT_WIN: u16 = 0x5B;
const RIGHT_WIN: u16 = 0x5C;

fn key(virtual_key: u16, is_down: bool) -> KeyInput {
    KeyInput {
        virtual_key,
        scan_code: 0,
        extended: false,
        is_down,
        injected: false,
    }
}

fn enabled() -> HotkeyReducer {
    let mut state = HotkeyReducer::default();
    state.set_capture_enabled(true);
    state
}

#[test]
fn either_control_can_be_tapped_while_the_other_holds_the_chord() {
    for (held, tapped) in [(LEFT_CTRL, RIGHT_CTRL), (RIGHT_CTRL, LEFT_CTRL)] {
        let mut state = enabled();
        state.reduce(key(held, true));
        assert_eq!(state.reduce(key(LEFT_WIN, true)).transition, Some(true));
        assert_eq!(state.reduce(key(tapped, true)).transition, None);
        assert_eq!(state.reduce(key(tapped, false)).transition, None);
        assert_eq!(state.reduce(key(held, false)).transition, Some(false));
        assert_eq!(state.reduce(key(LEFT_WIN, false)).transition, None);
    }
}

#[test]
fn releasing_one_of_two_windows_keys_preserves_the_chord() {
    for (held, tapped) in [(LEFT_WIN, RIGHT_WIN), (RIGHT_WIN, LEFT_WIN)] {
        let mut state = enabled();
        state.reduce(key(LEFT_CTRL, true));
        assert_eq!(state.reduce(key(held, true)).transition, Some(true));
        state.reduce(key(tapped, true));
        assert_eq!(state.reduce(key(tapped, false)).transition, None);
        assert_eq!(state.reduce(key(held, false)).transition, Some(false));
    }
}

#[test]
fn generic_modifier_codes_resolve_each_physical_side() {
    // Windows may report generic codes; Shift uses its scan code, Ctrl/Alt use extended.
    for (virtual_key, left_scan, right_scan, left_index) in [
        (0x11, 0x1D, 0x1D, 0),
        (0x10, 0x2A, 0x36, 2),
        (0x12, 0x38, 0x38, 4),
    ] {
        let mut state = enabled();
        let input = |scan_code, extended, is_down| KeyInput {
            scan_code,
            extended,
            ..key(virtual_key, is_down)
        };
        state.reduce(input(left_scan, false, true));
        state.reduce(input(right_scan, true, true));
        state.reduce(input(right_scan, true, false));
        assert!(state.modifiers[left_index].down);
        assert!(!state.modifiers[left_index + 1].down);
        state.reduce(input(left_scan, false, false));
        assert!(!state.modifiers[left_index].down);
    }
}

#[test]
fn explicit_shift_and_alt_codes_keep_the_other_side_down() {
    for (left, right, index) in [(0xA0, 0xA1, 2), (0xA4, 0xA5, 4)] {
        let mut state = enabled();
        state.reduce(key(left, true));
        state.reduce(key(right, true));
        assert_eq!(state.reduce(key(right, false)), KeyResult::default());
        assert!(state.modifiers[index].down);
        assert!(!state.modifiers[index + 1].down);
    }
}

// Keep: third-party injection cannot start a chord, alone or beside a held prefix.
#[test]
fn injected_keys_never_match_the_hotkey() {
    for (first, second) in [(LEFT_CTRL, LEFT_WIN), (LEFT_WIN, LEFT_CTRL)] {
        for physical_prefix in [false, true] {
            let mut state = enabled();
            if physical_prefix {
                state.reduce(key(first, true));
            }
            for is_down in [true, false] {
                for virtual_key in [first, second] {
                    assert_eq!(
                        state.reduce(KeyInput {
                            injected: true,
                            ..key(virtual_key, is_down)
                        }),
                        KeyResult::default()
                    );
                }
            }
            assert_eq!(state.reduce(key(first, false)).transition, None);
        }
    }
}

// Keep: injected paste events and arbitrary modifiers cannot change physical state or consumption.
#[test]
fn injected_input_preserves_physical_modifiers_and_consumption() {
    use super::super::windows_input::{paste_batch, KeyEvent, MODIFIERS};

    for held in [false, true] {
        let mut state = enabled();
        if held {
            state.reduce(key(LEFT_WIN, true));
            assert_eq!(state.reduce(key(LEFT_CTRL, true)).transition, Some(true));
        }
        let events = paste_batch(&MODIFIERS).into_iter().chain(
            MODIFIERS
                .into_iter()
                .flat_map(|key| [false, true, false].map(|up| KeyEvent { key, up })),
        );
        for event in events {
            assert_eq!(
                state.reduce(KeyInput {
                    injected: true,
                    ..key(event.key, !event.up)
                }),
                KeyResult::default()
            );
        }
        for (index, modifier) in state.modifiers.iter().enumerate() {
            assert_eq!(modifier.down, held && matches!(index, 0 | 6));
        }
        let release = state.reduce(key(LEFT_CTRL, false));
        assert_eq!(release.transition, held.then_some(false));
        assert_eq!(release.consume, held);
    }
}

#[test]
fn consumption_balances_both_press_and_release_orders() {
    for ctrl in [LEFT_CTRL, RIGHT_CTRL] {
        for win in [LEFT_WIN, RIGHT_WIN] {
            for (first, second) in [(ctrl, win), (win, ctrl)] {
                for release_second_first in [false, true] {
                    let mut state = enabled();
                    assert!(!state.reduce(key(first, true)).consume);
                    assert!(state.reduce(key(second, true)).consume);
                    let order = if release_second_first {
                        [second, first]
                    } else {
                        [first, second]
                    };
                    for virtual_key in order {
                        assert_eq!(
                            state.reduce(key(virtual_key, false)).consume,
                            virtual_key == second
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn start_menu_is_masked_while_a_forwarded_windows_key_is_still_down() {
    for win in [LEFT_WIN, RIGHT_WIN] {
        let mut state = enabled();
        let prefix = state.reduce(key(win, true));
        assert!(!prefix.consume);
        assert!(!prefix.mask_start_menu);
        let chord = state.reduce(key(LEFT_CTRL, true));
        assert!(chord.mask_start_menu);
        assert!(chord.consume);
        // The hook sends a balanced VK_E8 pair here, before either physical release.
        for is_down in [true, false] {
            assert_eq!(
                state.reduce(KeyInput {
                    injected: true,
                    ..key(0xE8, is_down)
                }),
                KeyResult::default()
            );
        }
        state.reduce(key(LEFT_CTRL, false));
        assert_eq!(state.reduce(key(win, false)), KeyResult::default());
    }
    let mut state = enabled();
    state.reduce(key(LEFT_CTRL, true));
    let chord = state.reduce(key(LEFT_WIN, true));
    assert!(chord.consume);
    assert!(!chord.mask_start_menu);
    assert!(state.reduce(key(LEFT_WIN, false)).consume);
}

#[test]
fn repeats_preserve_consumption_without_repeating_hotkey_edges() {
    let mut state = enabled();
    state.reduce(key(LEFT_CTRL, true));
    assert_eq!(state.reduce(key(LEFT_WIN, true)).transition, Some(true));
    for _ in 0..3 {
        assert_eq!(state.reduce(key(LEFT_CTRL, true)), KeyResult::default());
        assert_eq!(
            state.reduce(key(LEFT_WIN, true)),
            KeyResult {
                consume: true,
                ..KeyResult::default()
            }
        );
    }
    assert_eq!(state.reduce(key(LEFT_WIN, false)).transition, Some(false));
    assert_eq!(state.reduce(key(LEFT_WIN, false)), KeyResult::default());
}

#[test]
fn disabling_capture_preserves_consumed_keyups_and_physical_sides() {
    let mut state = enabled();
    state.reduce(key(LEFT_CTRL, true));
    state.reduce(key(LEFT_WIN, true));
    assert_eq!(state.set_capture_enabled(false), Some(false));
    assert_eq!(state.set_capture_enabled(false), None);
    assert_eq!(
        state.reduce(key(LEFT_WIN, true)),
        KeyResult {
            consume: true,
            ..KeyResult::default()
        }
    );
    assert_eq!(
        state.reduce(key(LEFT_WIN, false)),
        KeyResult {
            consume: true,
            ..KeyResult::default()
        }
    );
    state.set_capture_enabled(true);
    assert_eq!(state.reduce(key(RIGHT_WIN, true)).transition, Some(true));
    assert_eq!(state.reset(), Some(false));
    assert_eq!(state.reset(), None);
    assert_eq!(state.reduce(key(LEFT_WIN, true)).transition, None);
}

#[test]
fn ordinary_keys_and_disabled_chords_pass_through() {
    let mut state = HotkeyReducer::default();
    for virtual_key in [LEFT_CTRL, LEFT_WIN, 0x56] {
        for is_down in [true, false] {
            assert_eq!(
                state.reduce(key(virtual_key, is_down)),
                KeyResult::default()
            );
        }
    }
    state.set_capture_enabled(true);
    state.reduce(key(LEFT_CTRL, true));
    assert_eq!(state.reduce(key(0x56, true)), KeyResult::default());
    assert_eq!(state.reduce(key(0x56, false)), KeyResult::default());
}
