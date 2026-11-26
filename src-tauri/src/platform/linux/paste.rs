//! Synthesizes the paste keystroke for the user's session.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use super::Session;

pub(super) fn paste(session: Session) -> Result<(), String> {
    match session {
        Session::X11 => super::x11::paste()
            .map_err(|error| format!("Echo could not synthesize paste on X11: {error}")),
        Session::Wayland => wayland_paste(),
    }
}

/// Wayland forbids clients from injecting input, so Echo asks a helper that can:
/// `wtype` (virtual-keyboard protocol: wlroots compositors, KDE) or `ydotool` (uinput,
/// any compositor, needs its daemon running).
fn wayland_paste() -> Result<(), String> {
    let helpers: [(&str, &[&str]); 2] = [
        ("wtype", &["-M", "ctrl", "-k", "v", "-m", "ctrl"]),
        // Linux key codes: 29 is Left Control and 47 is V.
        ("ydotool", &["key", "29:1", "47:1", "47:0", "29:0"]),
    ];
    let mut failures = Vec::new();
    for (name, args) in helpers {
        let Some(program) = find_in_path(name) else {
            continue;
        };
        match Command::new(&program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
        {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(output) => failures.push(format!(
                "{name}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => failures.push(format!("{name}: {error}")),
        }
    }
    if failures.is_empty() {
        Err("Automatic paste on Wayland needs wtype or ydotool. Install one, or turn off Paste after transcription and paste with Ctrl+V.".to_string())
    } else {
        Err(format!(
            "Echo could not synthesize paste on Wayland ({}).",
            failures.join("; ")
        ))
    }
}

/// Resolves a helper to an absolute path from absolute PATH entries only.
fn find_in_path(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .filter(|directory| directory.is_absolute())
            .map(|directory| directory.join(name))
            .find(|candidate| {
                candidate
                    .metadata()
                    .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_in_path_returns_absolute_executables_only() {
        if let Some(sh) = find_in_path("sh") {
            assert!(sh.is_absolute());
        }
        assert!(find_in_path("echo-definitely-not-installed").is_none());
    }
}
