//! Passive hook recovery policy, independent of Windows APIs and wall-clock sleeps.

const SILENCE_MS: u64 = 5_000;
const COOLDOWN_MS: u64 = 60_000;
const MAX_ATTEMPTS: u32 = 3;

pub(super) struct HookObservation {
    pub alive: bool,
    pub session_active: bool,
    pub unlock_generation: u32,
    pub last_input_tick: Option<u32>,
    pub last_callback_tick: u32,
}

pub(super) trait HookPlatform {
    fn install(&mut self) -> bool;
    fn observe(&mut self) -> HookObservation;
    fn stop(&mut self);
    fn resync(&mut self);
    fn lifecycle(&mut self, available: bool);
}

#[derive(Default)]
pub(super) struct HookHealth {
    initialized: bool,
    available: bool,
    require_callback: bool,
    awaiting_callback: Option<u32>,
    attempts: u32,
    next_attempt_ms: u64,
    stable_since_ms: u64,
    suspect_since_ms: Option<u64>,
    last_callback_tick: u32,
    input_baseline_tick: Option<u32>,
    session_active: Option<bool>,
    unlock_generation: u32,
}

impl HookHealth {
    pub fn tick(&mut self, now_ms: u64, platform: &mut impl HookPlatform) {
        let observation = platform.observe();
        if !self.initialized {
            self.initialized = true;
            platform.lifecycle(false);
        }
        if (self.session_active == Some(false) && observation.session_active)
            || self.unlock_generation != observation.unlock_generation
        {
            self.attempts = 0;
            self.next_attempt_ms = now_ms;
            self.suspect_since_ms = None;
            self.input_baseline_tick = observation.last_input_tick;
            if self.available {
                platform.resync();
            }
        }
        if !observation.session_active {
            self.input_baseline_tick = observation.last_input_tick;
        }
        self.session_active = Some(observation.session_active);
        self.unlock_generation = observation.unlock_generation;
        if let Some(baseline) = self.awaiting_callback {
            if observation.alive {
                if !observation.session_active || observation.last_callback_tick == baseline {
                    return;
                }
                self.awaiting_callback = None;
                self.require_callback = false;
                platform.resync();
                self.available = true;
                self.stable_since_ms = now_ms;
                self.input_baseline_tick = observation.last_input_tick;
                platform.lifecycle(true);
            } else {
                self.awaiting_callback = None;
                platform.stop();
                self.next_attempt_ms = now_ms.saturating_add(if self.attempts >= MAX_ATTEMPTS {
                    COOLDOWN_MS
                } else {
                    100
                });
            }
        }
        if self.available {
            if now_ms.saturating_sub(self.stable_since_ms) >= COOLDOWN_MS {
                self.attempts = 0;
            }
            // Windows input ticks wrap every 49.7 days and may arrive out of order.
            let missing_callbacks = observation.last_input_tick.is_some_and(|input| {
                let newer_than_baseline = self.input_baseline_tick.is_none_or(|baseline| {
                    let delta = input.wrapping_sub(baseline);
                    delta > 0 && delta < i32::MAX as u32
                });
                let delta = input.wrapping_sub(observation.last_callback_tick);
                newer_than_baseline && delta > SILENCE_MS as u32 && delta < i32::MAX as u32
            });
            if !observation.session_active || !missing_callbacks {
                self.suspect_since_ms = None;
            } else if self.last_callback_tick != observation.last_callback_tick {
                self.suspect_since_ms = Some(now_ms);
            }
            self.last_callback_tick = observation.last_callback_tick;
            let silent_removal = observation.session_active
                && missing_callbacks
                && now_ms.saturating_sub(*self.suspect_since_ms.get_or_insert(now_ms))
                    >= SILENCE_MS;
            if observation.alive && !silent_removal {
                return;
            }
            self.require_callback |= silent_removal;
            self.available = false;
            platform.lifecycle(false);
            platform.stop();
            self.next_attempt_ms = now_ms.saturating_add(if self.attempts >= MAX_ATTEMPTS {
                COOLDOWN_MS
            } else {
                100
            });
        }
        if !observation.session_active || now_ms < self.next_attempt_ms {
            return;
        }
        if self.attempts >= MAX_ATTEMPTS {
            self.attempts = 0;
        }
        self.attempts += 1;
        if platform.install() {
            platform.resync();
            if self.require_callback {
                // Installation seeds the callback clock from last input. Sample after
                // that reset so installation alone cannot retire the fallback.
                self.awaiting_callback = Some(platform.observe().last_callback_tick);
                return;
            }
            self.available = true;
            self.stable_since_ms = now_ms;
            self.input_baseline_tick = observation.last_input_tick;
            self.suspect_since_ms = None;
            platform.lifecycle(true);
        } else {
            platform.stop();
            self.next_attempt_ms = now_ms.saturating_add(if self.attempts == MAX_ATTEMPTS {
                COOLDOWN_MS
            } else {
                100 << (self.attempts - 1)
            });
        }
    }

    pub fn next_delay_ms(&self, now_ms: u64) -> u64 {
        if self.available || self.awaiting_callback.is_some() || self.session_active == Some(false)
        {
            return 1_000;
        }
        self.next_attempt_ms.saturating_sub(now_ms).clamp(1, 1_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::windows_hotkey_reducer::{HotkeyReducer, KeyInput};
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Platform {
        installs: VecDeque<bool>,
        attempts: usize,
        alive: bool,
        events: Vec<bool>,
        stops: usize,
        reducer: HotkeyReducer,
        snapshot: [bool; 8],
        resyncs: usize,
        locked: bool,
        unlock_generation: u32,
        input_tick: Option<u32>,
        callback_tick: u32,
        reset_callback_on_install: bool,
    }
    impl HookPlatform for Platform {
        fn install(&mut self) -> bool {
            self.attempts += 1;
            if self.reset_callback_on_install {
                self.callback_tick = self.input_tick.unwrap_or(0);
            }
            self.installs.pop_front().unwrap_or(false)
        }
        fn observe(&mut self) -> HookObservation {
            HookObservation {
                alive: self.alive,
                session_active: !self.locked,
                unlock_generation: self.unlock_generation,
                last_input_tick: self.input_tick,
                last_callback_tick: self.callback_tick,
            }
        }
        fn stop(&mut self) {
            self.stops += 1;
        }
        fn resync(&mut self) {
            self.resyncs += 1;
            self.reducer.resync(self.snapshot);
        }
        fn lifecycle(&mut self, available: bool) {
            self.events.push(available);
        }
    }

    // Keep: failures retry with bounded backoff and leave the runtime on fallback.
    #[test]
    fn hook_health_install_failure_has_three_bounded_attempts() {
        let mut health = HookHealth::default();
        let mut platform = Platform::default();
        for time in [0, 99, 100, 299, 300, 999, 10_000] {
            health.tick(time, &mut platform);
            let expected = if time < 100 {
                1
            } else if time < 300 {
                2
            } else {
                3
            };
            assert_eq!(platform.attempts, expected);
            assert!((1..=1_000).contains(&health.next_delay_ms(time)));
        }
        assert_eq!(platform.events, [false]);
        assert_eq!(platform.resyncs, 0);
    }

    // Keep: thread exit and passive silent-removal detection use the same recovery boundary.
    #[test]
    fn hook_health_loss_and_recovery_publish_balanced_lifecycle_events() {
        let mut health = HookHealth::default();
        let mut platform = Platform {
            installs: [false, true, true].into(),
            alive: true,
            ..Platform::default()
        };
        health.tick(0, &mut platform);
        health.tick(100, &mut platform);
        assert_eq!(platform.events, [false, true]);
        health.tick(200, &mut platform);
        assert_eq!(platform.attempts, 2);
        platform.alive = false;
        health.tick(300, &mut platform);
        assert_eq!(platform.events, [false, true, false]);
        health.tick(399, &mut platform);
        assert_eq!(platform.attempts, 2);
        health.tick(400, &mut platform);
        assert_eq!(platform.events, [false, true, false, true]);
        assert_eq!(platform.resyncs, 2);
        assert_eq!(platform.stops, 2);
    }

    // Keep: an exhausted episode can recover without restarting the process.
    #[test]
    fn hook_health_cooldown_and_unlock_rearm_exhausted_attempts() {
        for rearm in ["cooldown", "desktop", "unlock"] {
            let mut health = HookHealth::default();
            let mut platform = Platform::default();
            for time in [0, 100, 300] {
                health.tick(time, &mut platform);
            }
            platform.installs.push_back(true);
            platform.alive = true;
            if rearm != "cooldown" {
                if rearm == "desktop" {
                    platform.locked = true;
                    health.tick(400, &mut platform);
                    platform.locked = false;
                } else {
                    // WTS can observe an unlock between health timer observations.
                    platform.unlock_generation += 1;
                }
                health.tick(500, &mut platform);
            } else {
                health.tick(60_299, &mut platform);
                assert_eq!(platform.attempts, 3);
                health.tick(60_300, &mut platform);
            }
            assert_eq!(platform.attempts, 4);
            assert_eq!(platform.events, [false, true]);
        }
    }

    // Keep: secure sessions and idle or blocked input cannot prove silent removal.
    #[test]
    fn hook_health_passive_evidence_requires_active_input_and_hysteresis() {
        let mut health = HookHealth::default();
        let mut platform = Platform {
            installs: [true, true].into(),
            alive: true,
            ..Platform::default()
        };
        health.tick(0, &mut platform);
        for time in [1_000, 10_000, 20_000] {
            health.tick(time, &mut platform);
        }
        assert_eq!(platform.events, [false, true]);
        platform.locked = true;
        platform.input_tick = Some(30_000);
        for time in [30_000, 40_000] {
            health.tick(time, &mut platform);
        }
        assert_eq!(platform.events, [false, true]);
        platform.locked = false;
        platform.unlock_generation += 1;
        health.tick(40_001, &mut platform);
        health.tick(49_000, &mut platform);
        assert_eq!(platform.events, [false, true]); // locked-session input is not active evidence
        platform.callback_tick = 30_000;
        health.tick(49_001, &mut platform); // a delayed callback clears suspicion
        platform.input_tick = Some(50_000);
        health.tick(50_000, &mut platform);
        health.tick(54_999, &mut platform);
        assert_eq!(platform.events, [false, true]);
        health.tick(55_000, &mut platform);
        assert_eq!(platform.events, [false, true, false]);
        health.tick(55_100, &mut platform);
        assert_eq!(platform.events, [false, true, false]);
        platform.callback_tick = 55_101;
        health.tick(55_101, &mut platform);
        assert_eq!(platform.events, [false, true, false, true]);
    }

    // Keep: elevated-window input must not repeatedly swap fallback and native sources.
    #[test]
    fn hook_health_silent_replacement_waits_for_its_own_callback() {
        let mut health = HookHealth::default();
        let mut platform = Platform {
            installs: [true, true, true].into(),
            alive: true,
            reset_callback_on_install: true,
            ..Platform::default()
        };
        health.tick(0, &mut platform);
        platform.input_tick = Some(10_000);
        health.tick(10_000, &mut platform);
        health.tick(15_000, &mut platform);
        assert_eq!(platform.events, [false, true, false]);
        health.tick(15_100, &mut platform);
        for time in [16_000, 21_000, 80_000] {
            platform.input_tick = Some(time as u32);
            health.tick(time, &mut platform);
            assert_eq!(platform.events, [false, true, false]);
            assert_eq!(platform.attempts, 2);
            assert_eq!(health.next_delay_ms(time), 1_000);
        }
        platform.alive = false;
        health.tick(80_001, &mut platform);
        assert_eq!(platform.events, [false, true, false]);
        platform.alive = true;
        health.tick(80_101, &mut platform);
        health.tick(80_102, &mut platform);
        assert_eq!(platform.attempts, 3);
        assert_eq!(platform.events, [false, true, false]);
        platform.callback_tick = 80_103;
        health.tick(80_103, &mut platform);
        assert_eq!(platform.events, [false, true, false, true]);
        assert_eq!(platform.resyncs, 4);
    }

    // Keep: short-lived recoveries share a budget, preventing repeated source swaps.
    #[test]
    fn hook_health_short_recoveries_do_not_reset_the_episode_budget() {
        let mut health = HookHealth::default();
        let mut platform = Platform {
            installs: [true, true, true, true].into(),
            alive: true,
            ..Platform::default()
        };
        for time in [0, 200, 400] {
            health.tick(time, &mut platform);
            platform.alive = false;
            health.tick(time + 1, &mut platform);
            platform.alive = true;
        }
        health.tick(1_000, &mut platform);
        assert_eq!(platform.attempts, 3);
        health.tick(60_400, &mut platform);
        assert_eq!(platform.attempts, 3);
        health.tick(60_401, &mut platform);
        assert_eq!(platform.attempts, 4);
    }

    // Keep: session resumption clears missed modifier releases without a hook replacement.
    #[test]
    fn hook_health_unlock_resyncs_a_live_worker_and_uses_a_quiet_timer() {
        let mut health = HookHealth::default();
        let mut platform = Platform {
            installs: [true].into(),
            alive: true,
            ..Platform::default()
        };
        health.tick(0, &mut platform);
        assert_eq!(health.next_delay_ms(0), 1_000);
        platform.locked = true;
        health.tick(100, &mut platform);
        assert_eq!(platform.resyncs, 1);
        platform.locked = false;
        platform.unlock_generation += 1;
        health.tick(200, &mut platform);
        assert_eq!(platform.resyncs, 2);
        assert_eq!(platform.attempts, 1);
        assert_eq!(platform.events, [false, true]);
        assert_eq!(health.next_delay_ms(200), 1_000);
    }

    // Keep: recovery uses the sampled sides and discards stale held/consumed state.
    #[test]
    fn hook_health_resync_applies_async_modifier_snapshot() {
        let mut platform = Platform {
            installs: [true].into(),
            ..Platform::default()
        };
        platform.reducer.set_capture_enabled(true);
        let key = |virtual_key, is_down| KeyInput {
            virtual_key,
            is_down,
            scan_code: 0,
            extended: false,
            injected: false,
        };
        platform.reducer.reduce(key(0xA2, true));
        assert_eq!(
            platform.reducer.reduce(key(0x5B, true)).transition,
            Some(true)
        );
        // Right Control and Windows remain down; left Control was released while lost.
        platform.snapshot[1] = true;
        platform.snapshot[6] = true;
        HookHealth::default().tick(0, &mut platform);
        assert_eq!(
            platform.reducer.reduce(key(0x5B, true)).transition,
            Some(true)
        );
        assert_eq!(
            platform.reducer.reduce(key(0xA3, false)).transition,
            Some(false)
        );
        assert!(!platform.reducer.reduce(key(0x5B, false)).consume);
        assert_eq!(platform.reducer.reduce(key(0x5B, true)).transition, None);
    }
}
