include!("src/command_registry.rs");

macro_rules! declare_echo_command_names {
    ($($name:ident => $path:path,)*) => {
        const ECHO_COMMAND_NAMES: &[&str] = &[$(stringify!($name)),*];
    };
}

for_each_echo_command!(declare_echo_command_names);

fn main() {
    let git_commit = git_output(&["rev-parse", "--short=12", "HEAD"])
        .unwrap_or_else(|| "unavailable".to_string());
    let is_dirty = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .is_ok_and(|output| output.status.success() && !output.stdout.is_empty());
    let git_provenance = if is_dirty {
        format!("{git_commit}-dirty")
    } else {
        git_commit
    };

    println!("cargo:rustc-env=ECHO_GIT_COMMIT={git_provenance}");
    if let Some(head_path) = git_output(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head_path}");
    }
    if let Some(head_reference) = git_output(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(reference_path) =
            git_output(&["rev-parse", "--git-path", head_reference.as_str()])
        {
            println!("cargo:rerun-if-changed={reference_path}");
        }
    }
    for git_path in ["packed-refs", "index"] {
        if let Some(path) = git_output(&["rev-parse", "--git-path", git_path]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!("cargo:rerun-if-changed=../src");
    println!("cargo:rerun-if-changed=../package.json");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=icons");
    let manifest = tauri_build::AppManifest::new().commands(ECHO_COMMAND_NAMES);
    let attributes = tauri_build::Attributes::new().app_manifest(manifest);
    let attributes = if embeds_windows_manifest_with_linker() {
        attributes.windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest())
    } else {
        attributes
    };
    tauri_build::try_build(attributes).expect("Echo Tauri configuration should build");
}

// tauri-build embeds its Common Controls v6 manifest in binaries only, but unit-test
// executables import comctl32 v6 entry points too. Keeping both manifests fails with
// CVT1100, and `rustc-link-arg-tests` covers neither lib unit tests nor this crate,
// which has no tests/ directory. On MSVC the linker embeds one manifest everywhere.
fn embeds_windows_manifest_with_linker() -> bool {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        return false;
    }
    let manifest_directory =
        std::env::var("CARGO_MANIFEST_DIR").expect("Cargo should set CARGO_MANIFEST_DIR");
    let manifest = std::path::Path::new(&manifest_directory).join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFESTUAC:NO");
    true
}

fn git_output(arguments: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
