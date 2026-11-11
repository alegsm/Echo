use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const ACTIVE_NAME: &str = "Echo.log";
const ARCHIVE_NAMES: [&str; 2] = ["Echo.1.log", "Echo.2.log"];
const MAX_FILE_BYTES: u64 = 1_048_576;
const MAX_WRITE_BYTES: usize = 65_536;
const TRUNCATED_SUFFIX: &[u8] = b" [record-truncated]\n";

pub fn dispatch(log_directory: PathBuf) -> Result<fern::Dispatch, String> {
    let writer = BoundedLogWriter::open(log_directory, MAX_FILE_BYTES)
        .map_err(|_| "diagnostic_log_open_failed".to_string())?;
    Ok(fern::Dispatch::new().chain(Box::new(writer) as Box<dyn Write + Send>))
}

pub fn platform_log_directory(identifier: &str) -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    let directory = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Logs").join(identifier));

    #[cfg(target_os = "windows")]
    let directory = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|base| base.join(identifier).join("logs"));

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let directory = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local/share"))
        })
        .map(|base| base.join(identifier).join("logs"));

    directory.ok_or_else(|| "diagnostic_log_directory_unavailable".to_string())
}

struct BoundedLogWriter {
    directory: PathBuf,
    active: File,
    current_size: u64,
    max_size: u64,
    pending: Vec<u8>,
    pending_truncated: bool,
}

impl BoundedLogWriter {
    fn open(directory: PathBuf, max_size: u64) -> io::Result<Self> {
        create_private_directory(&directory)?;
        remove_legacy_archives(&directory)?;
        normalize_owned_files(&directory, max_size)?;
        let active = open_private_append(&directory.join(ACTIVE_NAME))?;
        let current_size = active.metadata()?.len();
        let mut writer = Self {
            directory,
            active,
            current_size,
            max_size,
            pending: Vec::with_capacity(1024),
            pending_truncated: false,
        };
        if writer.current_size >= writer.max_size {
            writer.rotate()?;
        }
        Ok(writer)
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.active.flush()?;
        let oldest = self.directory.join(ARCHIVE_NAMES[1]);
        remove_if_present(&oldest)?;
        rename_if_present(&self.directory.join(ARCHIVE_NAMES[0]), &oldest)?;
        rename_if_present(
            &self.directory.join(ACTIVE_NAME),
            &self.directory.join(ARCHIVE_NAMES[0]),
        )?;
        self.active = open_private_append(&self.directory.join(ACTIVE_NAME))?;
        self.current_size = 0;
        Ok(())
    }

    fn commit_pending(&mut self) -> io::Result<()> {
        if self.pending_truncated {
            let retained = MAX_WRITE_BYTES.saturating_sub(TRUNCATED_SUFFIX.len());
            self.pending.truncate(retained);
            self.pending.extend_from_slice(TRUNCATED_SUFFIX);
        } else {
            self.pending.push(b'\n');
        }
        if self.current_size > 0
            && self.current_size.saturating_add(self.pending.len() as u64) > self.max_size
        {
            self.rotate()?;
        }
        self.active.write_all(&self.pending)?;
        self.current_size += self.pending.len() as u64;
        self.pending.clear();
        self.pending_truncated = false;
        Ok(())
    }
}

impl Write for BoundedLogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for byte in bytes {
            if *byte == b'\n' {
                self.commit_pending()?;
            } else if self.pending.len() < MAX_WRITE_BYTES - 1 {
                self.pending.push(*byte);
            } else {
                self.pending_truncated = true;
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.active.flush()
    }
}

fn create_private_directory(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn open_private_append(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

fn remove_legacy_archives(directory: &Path) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let legacy =
            name.starts_with("Echo_") && (name.ends_with(".log") || name.ends_with(".log.bak"));
        if legacy && path.is_file() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn normalize_owned_files(directory: &Path, max_size: u64) -> io::Result<()> {
    for name in [ACTIVE_NAME, ARCHIVE_NAMES[0], ARCHIVE_NAMES[1]] {
        normalize_owned_file(&directory.join(name), max_size)?;
    }
    Ok(())
}

fn normalize_owned_file(path: &Path, max_size: u64) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "log path is a symlink",
        ));
    }
    if metadata.len() <= max_size {
        return Ok(());
    }

    let mut source = File::open(path)?;
    let tail_offset = metadata.len() - max_size;
    source.seek(SeekFrom::Start(tail_offset - 1))?;
    let mut preceding = [0_u8; 1];
    source.read_exact(&mut preceding)?;
    let mut tail = Vec::with_capacity(max_size as usize);
    source.take(max_size).read_to_end(&mut tail)?;
    let retained = if preceding[0] == b'\n' {
        tail.as_slice()
    } else {
        tail.iter()
            .position(|byte| *byte == b'\n')
            .map(|boundary| &tail[boundary + 1..])
            .unwrap_or_default()
    };
    replace_with_private_file(path, retained)
}

fn replace_with_private_file(path: &Path, content: &[u8]) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "log path has no directory"))?;
    let temporary = directory.join(format!(".echo-log-normalize-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = create_private_new(&temporary)?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn create_private_new(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn rename_if_present(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[path = "bounded_log_tests.rs"]
mod tests;
