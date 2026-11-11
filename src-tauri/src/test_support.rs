// Child test processes start from an empty environment so provider keys and proxies
// cannot leak in. Windows still needs SystemRoot for Winsock (error 10106) and
// TEMP/TMP for temporary storage, so those are carried over when present.
pub(crate) fn clear_child_environment(
    command: &mut std::process::Command,
) -> &mut std::process::Command {
    command.env_clear();
    #[cfg(windows)]
    for name in ["SystemRoot", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
}
