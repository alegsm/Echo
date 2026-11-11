use super::*;

#[test]
fn repeated_rotations_keep_active_and_two_archives() {
    let directory = test_directory("rotation");
    let mut writer = BoundedLogWriter::open(directory.clone(), 32).unwrap();
    for index in 0..12 {
        writeln!(writer, "record-{index:02}-with-content").unwrap();
        writer.flush().unwrap();
    }
    let mut names = file_names(&directory);
    names.sort();
    assert_eq!(names, ["Echo.1.log", "Echo.2.log", "Echo.log"]);
    assert!(total_bytes(&directory) <= 96);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn oversized_writes_are_bounded_and_report_consumed() {
    let directory = test_directory("line-bound");
    let mut writer = BoundedLogWriter::open(directory.clone(), 1_000_000).unwrap();
    let input = vec![b'x'; MAX_WRITE_BYTES * 2];
    assert_eq!(writer.write(&input).unwrap(), input.len());
    writer.write_all(b"\n").unwrap();
    writer.flush().unwrap();
    let content = fs::read(directory.join(ACTIVE_NAME)).unwrap();
    assert!(content.len() <= MAX_WRITE_BYTES);
    assert!(content.ends_with(TRUNCATED_SUFFIX));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn fragmented_record_stays_in_one_file() {
    let directory = test_directory("fragments");
    let mut writer = BoundedLogWriter::open(directory.clone(), 12).unwrap();
    writer.write_all(b"first-line\n").unwrap();
    writer.write_all(b"second").unwrap();
    writer.write_all(b"-line").unwrap();
    writer.write_all(b"\n").unwrap();
    writer.flush().unwrap();
    assert_eq!(
        fs::read_to_string(directory.join(ACTIVE_NAME)).unwrap(),
        "second-line\n"
    );
    assert_eq!(
        fs::read_to_string(directory.join(ARCHIVE_NAMES[0])).unwrap(),
        "first-line\n"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn serialized_concurrent_flood_keeps_complete_records_and_three_files() {
    use std::sync::{Arc, Mutex};

    let directory = test_directory("concurrent");
    let writer = Arc::new(Mutex::new(
        BoundedLogWriter::open(directory.clone(), 256).unwrap(),
    ));
    let handles: Vec<_> = (0..4)
        .map(|thread| {
            let writer = Arc::clone(&writer);
            std::thread::spawn(move || {
                for record in 0..20 {
                    writeln!(
                        writer.lock().unwrap(),
                        "thread={thread};record={record};payload=complete"
                    )
                    .unwrap();
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    writer.lock().unwrap().flush().unwrap();
    assert!(file_names(&directory).len() <= 3);
    for name in file_names(&directory) {
        let content = fs::read_to_string(directory.join(name)).unwrap();
        assert!(content
            .lines()
            .all(|line| line.ends_with("payload=complete")));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn fern_dispatch_writes_through_the_bounded_sink() {
    let directory = test_directory("fern");
    let (_, logger) = dispatch(directory.clone())
        .unwrap()
        .format(|out, message, _record| out.finish(format_args!("formatted:{message}")))
        .into_log();
    let record = log::Record::builder()
        .level(log::Level::Info)
        .target("echo")
        .args(format_args!("diagnostic-event"))
        .build();
    logger.log(&record);
    logger.flush();
    assert_eq!(
        fs::read_to_string(directory.join(ACTIVE_NAME)).unwrap(),
        "formatted:diagnostic-event\n"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rotation_does_not_touch_unowned_files() {
    let directory = test_directory("ownership");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("Other.log"), b"keep").unwrap();
    fs::write(directory.join("Echo_notes.txt"), b"keep").unwrap();
    fs::write(directory.join("Echo_legacy.log.bak"), b"remove").unwrap();
    let _writer = BoundedLogWriter::open(directory.clone(), 32).unwrap();
    assert!(directory.join("Other.log").exists());
    assert!(directory.join("Echo_notes.txt").exists());
    assert!(!directory.join("Echo_legacy.log.bak").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn startup_bounds_all_owned_files_at_complete_record_boundaries() {
    let directory = test_directory("startup-bound");
    fs::create_dir_all(&directory).unwrap();
    for name in [ACTIVE_NAME, ARCHIVE_NAMES[0], ARCHIVE_NAMES[1]] {
        fs::write(directory.join(name), repeated_records(12)).unwrap();
    }
    let _writer = BoundedLogWriter::open(directory.clone(), 40).unwrap();
    for name in [ACTIVE_NAME, ARCHIVE_NAMES[0], ARCHIVE_NAMES[1]] {
        let content = fs::read_to_string(directory.join(name)).unwrap();
        assert!(content.len() <= 40);
        assert!(content.lines().all(|line| line.starts_with("record=")));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn startup_preserves_small_existing_logs_byte_for_byte() {
    let directory = test_directory("startup-preserve");
    fs::create_dir_all(&directory).unwrap();
    let original = b"record=01\nrecord=02\n";
    fs::write(directory.join(ACTIVE_NAME), original).unwrap();
    let _writer = BoundedLogWriter::open(directory.clone(), 128).unwrap();
    assert_eq!(fs::read(directory.join(ACTIVE_NAME)).unwrap(), original);
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn directory_and_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let directory = test_directory("permissions");
    let mut writer = BoundedLogWriter::open(directory.clone(), 32).unwrap();
    writer.write_all(b"content").unwrap();
    writer.flush().unwrap();
    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(directory.join(ACTIVE_NAME))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    fs::remove_dir_all(directory).unwrap();
}

fn test_directory(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("echo-log-{label}-{}", uuid::Uuid::new_v4()))
}

fn file_names(directory: &Path) -> Vec<String> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

fn total_bytes(directory: &Path) -> u64 {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().metadata().unwrap().len())
        .sum()
}

fn repeated_records(count: usize) -> Vec<u8> {
    (0..count)
        .flat_map(|index| format!("record={index:02}\n").into_bytes())
        .collect()
}
