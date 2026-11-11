//! Native clipboard delivery. Clipboard contents never leave this service's platform boundary.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

static DELIVERY_LOCK: Mutex<()> = Mutex::new(());

// Six completed native jobs plus two explicit copies can wait for clipboard ownership.
const MAX_PENDING_DELIVERIES: usize = 8;
static PENDING_DELIVERIES: AtomicUsize = AtomicUsize::new(0);

/// Reserve before spawning a worker so renderer invocations cannot grow an unbounded native queue.
pub struct DeliveryPermit(());

pub fn admit() -> Result<DeliveryPermit, DeliveryError> {
    PENDING_DELIVERIES
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |pending| {
            (pending < MAX_PENDING_DELIVERIES).then_some(pending + 1)
        })
        .map_err(|_| DeliveryError::ClipboardFailed)?;
    Ok(DeliveryPermit(()))
}

impl Drop for DeliveryPermit {
    fn drop(&mut self) {
        PENDING_DELIVERIES.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum DeliveryError {
    ClipboardFailed,
    InsertionFailed,
}

#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryOutcome {
    Pasted,
    Copied,
}

pub trait DeliveryPlatform {
    fn snapshot(&self) -> Result<String, ()>;
    fn publish_protected(&self, text: &str) -> Result<u64, ()>;
    fn paste(&self) -> Result<(), ()>;
    fn restore_if_owned(&self, previous: &str, text: &str, generation: u64) -> Result<(), ()>;
}

pub fn serialized<T>(operation: impl FnOnce() -> T) -> Result<T, DeliveryError> {
    let _guard = DELIVERY_LOCK
        .lock()
        .map_err(|_| DeliveryError::ClipboardFailed)?;
    Ok(operation())
}

pub fn deliver(
    platform: &impl DeliveryPlatform,
    sleep: impl Fn(Duration),
    text: &str,
    paste_after_transcription: bool,
    restore_clipboard: bool,
) -> Result<DeliveryOutcome, DeliveryError> {
    let previous = if paste_after_transcription && restore_clipboard {
        platform.snapshot().ok()
    } else {
        None
    };
    let generation = platform
        .publish_protected(text)
        .map_err(|_| DeliveryError::ClipboardFailed)?;
    if !paste_after_transcription {
        return Ok(DeliveryOutcome::Copied);
    }
    sleep(Duration::from_millis(250));
    let outcome = platform
        .paste()
        .map(|()| DeliveryOutcome::Pasted)
        .map_err(|_| DeliveryError::InsertionFailed);
    if let Some(previous) = previous {
        sleep(Duration::from_millis(150));
        // Restoration is best effort; it cannot replace the authoritative insertion outcome.
        let _ = platform.restore_if_owned(&previous, text, generation);
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Clipboard {
        text: String,
        generation: u64,
        calls: Vec<&'static str>,
        fail: &'static str,
        close_bumps_generation: bool,
    }
    struct Platform(RefCell<Clipboard>);
    impl Platform {
        fn new(fail: &'static str) -> Self {
            Self(RefCell::new(Clipboard {
                text: "previous".into(),
                generation: 1,
                calls: vec![],
                fail,
                close_bumps_generation: false,
            }))
        }
    }
    struct Publication<'a>(&'a RefCell<Clipboard>);
    impl Drop for Publication<'_> {
        fn drop(&mut self) {
            let mut clipboard = self.0.borrow_mut();
            if clipboard.close_bumps_generation {
                clipboard.generation += 1;
            }
        }
    }
    impl DeliveryPlatform for Platform {
        fn snapshot(&self) -> Result<String, ()> {
            let mut clipboard = self.0.borrow_mut();
            clipboard.calls.push("snapshot");
            if clipboard.fail == "snapshot" {
                Err(())
            } else {
                Ok(clipboard.text.clone())
            }
        }
        fn publish_protected(&self, text: &str) -> Result<u64, ()> {
            self.0.borrow_mut().calls.push("protected");
            let previous = self.0.borrow().text.clone();
            crate::platform::publish_clipboard(
                Publication(&self.0),
                Some(&previous),
                |_| {
                    let mut clipboard = self.0.borrow_mut();
                    clipboard.text.clear();
                    clipboard.generation += 1;
                    if matches!(clipboard.fail, "write" | "marker") {
                        return Err(());
                    }
                    clipboard.text = text.into();
                    Ok(())
                },
                |_, previous| {
                    self.0.borrow_mut().text = previous.into();
                    Ok(())
                },
                || self.0.borrow().generation,
            )
        }
        fn paste(&self) -> Result<(), ()> {
            let mut clipboard = self.0.borrow_mut();
            clipboard.calls.push("paste");
            if clipboard.fail == "paste" {
                Err(())
            } else {
                Ok(())
            }
        }
        fn restore_if_owned(&self, previous: &str, text: &str, generation: u64) -> Result<(), ()> {
            let mut clipboard = self.0.borrow_mut();
            clipboard.calls.push("restore");
            if clipboard.fail == "restore" {
                return Err(());
            }
            if crate::platform::clipboard_is_owned(
                generation,
                clipboard.generation,
                text,
                &clipboard.text,
            ) {
                clipboard.text = previous.into();
                clipboard.generation += 1;
            }
            Ok(())
        }
    }

    // Keep: Windows synthesizes text formats at close, changing the ownership sequence.
    #[test]
    fn transcript_delivery_restores_using_generation_after_publication_closes() {
        let platform = Platform::new("");
        platform.0.borrow_mut().close_bumps_generation = true;
        assert_eq!(
            deliver(&platform, |_| {}, "transcript", true, true),
            Ok(DeliveryOutcome::Pasted)
        );
        assert_eq!(platform.0.borrow().text, "previous");
    }

    // Keep: publication rollback protects saved text even when cleanup restoration is disabled.
    #[test]
    fn transcript_delivery_publication_failure_rolls_back_before_returning() {
        for paste in [false, true] {
            for restore in [false, true] {
                for failure in ["marker", "write"] {
                    let platform = Platform::new(failure);
                    assert_eq!(
                        deliver(
                            &platform,
                            |_| panic!("failed publication must not wait"),
                            "transcript",
                            paste,
                            restore
                        ),
                        Err(DeliveryError::ClipboardFailed)
                    );
                    let clipboard = platform.0.borrow();
                    assert_eq!(clipboard.text, "previous");
                    assert!(!clipboard.calls.contains(&"paste"));
                }
            }
        }
    }

    // Keep: overload is rejected before spawning, and every exit returns its admission slot.
    #[test]
    fn transcript_delivery_bounds_pending_native_work() {
        let mut permits: Vec<_> = (0..MAX_PENDING_DELIVERIES)
            .map(|_| admit().unwrap())
            .collect();
        assert!(matches!(admit(), Err(DeliveryError::ClipboardFailed)));
        permits.pop();
        let replacement = admit().unwrap();
        drop(permits);
        drop(replacement);
        assert!(admit().is_ok());
    }

    // Keep: one native operation owns snapshot, protected publication, both waits and cleanup.
    #[test]
    fn transcript_delivery_policy_and_failure_matrix() {
        for (paste, restore, failure, expected, calls, waits, text) in [
            (
                true,
                true,
                "",
                Ok(DeliveryOutcome::Pasted),
                vec!["snapshot", "protected", "paste", "restore"],
                vec![250, 150],
                "previous",
            ),
            (
                true,
                false,
                "",
                Ok(DeliveryOutcome::Pasted),
                vec!["protected", "paste"],
                vec![250],
                "transcript",
            ),
            (
                false,
                true,
                "",
                Ok(DeliveryOutcome::Copied),
                vec!["protected"],
                vec![],
                "transcript",
            ),
            (
                false,
                false,
                "",
                Ok(DeliveryOutcome::Copied),
                vec!["protected"],
                vec![],
                "transcript",
            ),
            (
                true,
                true,
                "snapshot",
                Ok(DeliveryOutcome::Pasted),
                vec!["snapshot", "protected", "paste"],
                vec![250],
                "transcript",
            ),
            (
                true,
                true,
                "write",
                Err(DeliveryError::ClipboardFailed),
                vec!["snapshot", "protected"],
                vec![],
                "previous",
            ),
            (
                true,
                true,
                "marker",
                Err(DeliveryError::ClipboardFailed),
                vec!["snapshot", "protected"],
                vec![],
                "previous",
            ),
            (
                true,
                true,
                "paste",
                Err(DeliveryError::InsertionFailed),
                vec!["snapshot", "protected", "paste", "restore"],
                vec![250, 150],
                "previous",
            ),
            (
                true,
                true,
                "restore",
                Ok(DeliveryOutcome::Pasted),
                vec!["snapshot", "protected", "paste", "restore"],
                vec![250, 150],
                "transcript",
            ),
        ] {
            let platform = Platform::new(failure);
            let slept = RefCell::new(vec![]);
            assert_eq!(
                deliver(
                    &platform,
                    |d| slept.borrow_mut().push(d.as_millis()),
                    "transcript",
                    paste,
                    restore
                ),
                expected,
                "{failure}/{paste}/{restore}"
            );
            let clipboard = platform.0.borrow();
            assert_eq!(clipboard.calls, calls, "{failure}");
            assert_eq!(*slept.borrow(), waits, "{failure}");
            assert_eq!(clipboard.text, text, "{failure}");
        }
    }

    // Keep: even an identical external copy changes ownership; text equality alone is insufficient.
    #[test]
    fn transcript_delivery_preserves_newer_copies() {
        for external in ["different", "transcript"] {
            let platform = Platform::new("");
            let sleep = |duration| {
                if duration == Duration::from_millis(150) {
                    let mut clipboard = platform.0.borrow_mut();
                    clipboard.text = external.into();
                    clipboard.generation += 1;
                }
            };
            assert_eq!(
                deliver(&platform, sleep, "transcript", true, true),
                Ok(DeliveryOutcome::Pasted)
            );
            assert_eq!(platform.0.borrow().text, external);
        }
        assert!(!crate::platform::clipboard_is_owned(
            1,
            1,
            "transcript",
            "changed"
        ));
    }

    // Keep: explicit copy and deliveries share one process lock, including cleanup waits.
    #[test]
    fn transcript_delivery_serializes_copy_and_releases_after_failure() {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let delivery = std::thread::spawn(move || {
            serialized(|| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Err::<(), _>(DeliveryError::InsertionFailed)
            })
        });
        entered_rx.recv().unwrap();
        let (copy_ready_tx, copy_ready_rx) = std::sync::mpsc::channel();
        let (copied_tx, copied_rx) = std::sync::mpsc::channel();
        let copy = std::thread::spawn(move || {
            copy_ready_tx.send(()).unwrap();
            serialized(|| copied_tx.send(()).unwrap())
        });
        copy_ready_rx.recv().unwrap();
        let premature_copy = copied_rx.recv_timeout(Duration::from_millis(30));
        release_tx.send(()).unwrap();
        assert_eq!(
            delivery.join().unwrap(),
            Ok(Err(DeliveryError::InsertionFailed))
        );
        assert_eq!(copy.join().unwrap(), Ok(()));
        assert!(
            premature_copy.is_err(),
            "copy ran before delivery cleanup finished"
        );
        copied_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    }
}
