use super::test_support::{cleanup, initialized, test_database, transcript};
use super::*;

#[test]
fn history_orders_offset_timestamps_by_their_instant() {
    let path = test_database("ordering");
    let storage = initialized(&path);
    storage
        .append_transcript(transcript("earlier", "9998-09-09T10:00:00+05:00"))
        .unwrap();
    storage
        .append_transcript(transcript("later", "9998-09-09T06:00:00Z"))
        .unwrap();

    let page = storage.list_history(10, None).unwrap();
    let ids: Vec<_> = page
        .records
        .iter()
        .map(|stored| stored.record.id.as_str())
        .collect();
    assert_eq!(ids, ["later", "earlier"]);
    cleanup(&path);
}

#[test]
fn append_rejects_unicode_and_leap_second_timestamps() {
    for (index, created_at) in ["2026-09-09T10:00:00Zé", "2026-09-09T10:00:60Z"]
        .into_iter()
        .enumerate()
    {
        let path = test_database(&format!("invalid-time-{index}"));
        let storage = initialized(&path);
        assert_eq!(
            storage
                .append_transcript(transcript("invalid", created_at))
                .unwrap_err(),
            StorageError::Validation("transcript.createdAt")
        );
        assert_eq!(storage.read_settings().unwrap().revision, 0);
        let page = storage.list_history(10, None).unwrap();
        assert_eq!(page.revision, 0);
        assert!(page.records.is_empty());
        cleanup(&path);
    }
}
