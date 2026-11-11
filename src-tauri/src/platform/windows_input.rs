//! Pure Windows paste batches; no Windows API dependency so every prefix runs on host tests.
pub(super) const MODIFIERS: [u16; 8] = [0xA2, 0xA3, 0xA0, 0xA1, 0xA4, 0xA5, 0x5B, 0x5C];
const CONTROL: u16 = 0xA2;
const V: u16 = 0x56;
const MASK: [KeyEvent; 2] = [
    KeyEvent {
        key: 0xE8,
        up: false,
    },
    KeyEvent {
        key: 0xE8,
        up: true,
    },
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct KeyEvent {
    pub key: u16,
    pub up: bool,
}

pub(super) fn paste_batch(held: &[u16]) -> Vec<KeyEvent> {
    // A no-op pair prevents bare Alt/Win releases from activating menus, including later
    // physical releases after we restore those modifiers.
    MASK.into_iter()
        .chain(held.iter().map(|&key| KeyEvent { key, up: true }))
        .chain([
            KeyEvent {
                key: CONTROL,
                up: false,
            },
            KeyEvent { key: V, up: false },
            KeyEvent { key: V, up: true },
            KeyEvent {
                key: CONTROL,
                up: true,
            },
        ])
        .chain(held.iter().map(|&key| KeyEvent { key, up: false }))
        .chain(MASK)
        .collect()
}

pub(super) fn insert(held: &[u16], mut send: impl FnMut(&[KeyEvent]) -> usize) -> Result<(), ()> {
    let batch = paste_batch(held);
    if send(&batch) == batch.len() {
        return Ok(());
    }
    // Never replay V-down. Even a rejected batch gets balanced synthetic ups and modifier recovery.
    let recovery: Vec<_> = [
        KeyEvent { key: V, up: true },
        KeyEvent {
            key: CONTROL,
            up: true,
        },
    ]
    .into_iter()
    .chain(held.iter().map(|&key| KeyEvent { key, up: false }))
    .chain(MASK)
    .collect();
    let mut remaining = recovery.as_slice();
    for _ in 0..3 {
        let sent = send(remaining);
        remaining = &remaining[sent..];
        if remaining.is_empty() {
            break;
        }
    }
    Err(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    // Windows treats an unmasked Alt/Win release as a menu request.
    fn assert_menu_keys_masked(state: &BTreeSet<u16>, masked: &mut BTreeSet<u16>, event: KeyEvent) {
        if matches!(event.key, 0xA4 | 0xA5 | 0x5B | 0x5C) {
            if event.up && state.contains(&event.key) {
                assert!(
                    masked.remove(&event.key),
                    "unmasked menu release: {event:?}"
                );
            } else if !event.up && !state.contains(&event.key) {
                masked.remove(&event.key);
            }
        } else if !event.up {
            masked.extend(state.iter().copied());
        }
    }

    fn assert_physical_release_masked(state: &mut BTreeSet<u16>, masked: &mut BTreeSet<u16>) {
        for key in state.clone() {
            let event = KeyEvent { key, up: true };
            assert_menu_keys_masked(state, masked, event);
            apply(state, &[event]);
        }
    }

    fn apply(state: &mut BTreeSet<u16>, events: &[KeyEvent]) {
        for event in events {
            if event.up {
                state.remove(&event.key);
            } else {
                state.insert(event.key);
            }
        }
    }

    // Keep: left and right modifiers must all be absent at V-down and restored afterward.
    #[test]
    fn clipboard_windows_modifiers_are_released_for_every_combination() {
        for mask in 0..256 {
            let held: Vec<_> = MODIFIERS
                .iter()
                .enumerate()
                .filter_map(|(i, key)| (mask & (1 << i) != 0).then_some(*key))
                .collect();
            let mut state: BTreeSet<_> = held.iter().copied().collect();
            let mut masked = BTreeSet::new();
            let mut pastes = 0;
            insert(&held, |batch| {
                for event in batch {
                    assert_menu_keys_masked(&state, &mut masked, *event);
                    if event.key == V && !event.up {
                        assert_eq!(state, BTreeSet::from([CONTROL]), "mask={mask}");
                        pastes += 1;
                    }
                    apply(&mut state, &[*event]);
                }
                batch.len()
            })
            .unwrap();
            assert_eq!(pastes, 1);
            assert_eq!(state, held.into_iter().collect());
            assert_physical_release_masked(&mut state, &mut masked);
        }
    }

    // Keep: each partial prefix fails without replaying paste and recovers the original modifiers.
    #[test]
    fn clipboard_windows_every_partial_prefix_recovers_without_replay() {
        for mask in 0..256 {
            let held: Vec<_> = MODIFIERS
                .iter()
                .enumerate()
                .filter_map(|(i, key)| (mask & (1 << i) != 0).then_some(*key))
                .collect();
            for prefix in 0..paste_batch(&held).len() {
                let mut state: BTreeSet<_> = held.iter().copied().collect();
                let mut masked = BTreeSet::new();
                let mut calls = 0;
                assert_eq!(
                    insert(&held, |batch| {
                        calls += 1;
                        let accepted = if calls == 1 { prefix } else { batch.len() };
                        if calls > 1 {
                            assert!(!batch.iter().any(|e| e.key == V && !e.up));
                        }
                        for &event in &batch[..accepted] {
                            assert_menu_keys_masked(&state, &mut masked, event);
                            apply(&mut state, &[event]);
                        }
                        accepted
                    }),
                    Err(())
                );
                assert!(calls <= 4);
                assert_eq!(
                    state,
                    held.iter().copied().collect(),
                    "mask={mask} prefix={prefix}"
                );
                assert_physical_release_masked(&mut state, &mut masked);
            }
        }
        let mut rejected_calls = 0;
        assert_eq!(
            insert(&MODIFIERS, |_| {
                rejected_calls += 1;
                0
            }),
            Err(())
        );
        assert_eq!(rejected_calls, 4);
    }
}
