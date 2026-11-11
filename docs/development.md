# Development

This guide owns build, check and packaging commands. [README](../README.md#quick-start) covers setup and dictation. [Architecture](architecture.md) maps the implementation.

## Toolchains and dependencies

Use Bun 1.4.4, pinned by [package.json](../package.json), and Rust 1.94.1 with rustfmt and Clippy, pinned by [rust-toolchain.toml](../rust-toolchain.toml). Use the committed Bun and Cargo lockfiles. Builds must not fetch another Git revision or upgrade tools implicitly.

From a fresh checkout, verify installed toolchains and install dependencies:

```sh
bun scripts/verify-toolchain.ts
bun install --frozen-lockfile
```

The [toolchain verifier](../scripts/verify-toolchain.ts) checks the installed versions. Install the pinned tools before running it. Run commands from the repository root unless a command says otherwise.

The frontend uses SvelteKit 3, adapter-static 4, vite-plugin-svelte 7, Svelte 5, TypeScript 6 and Vite exactly 8.3.6. [vite.config.ts](../vite.config.ts) owns preprocessing, the static index fallback and disabled scheduled version polling. [package.json](../package.json) supplies native `#lib/*` imports with explicit source extensions; no test alias resolver is needed.

The explicit build target is Chrome 111, Edge 111, Firefox 114 and Safari 16.4. Supported desktop webviews are macOS WebKit 16.4 or later and Windows WebView2 evergreen.

## macOS source build

Prerequisites: the pinned Bun and Rust toolchains, Xcode command-line tools and a working Apple SDK. Install Xcode command-line tools with `xcode-select --install` if needed, then run the dependency setup above.

Launch the desktop app:

```sh
bun run tauri:dev
```

Tauri starts Vite through `beforeDevCommand` in [tauri.conf.json](../src-tauri/tauri.conf.json). Both windows need Tauri; a browser tab does not provide the desktop runtime.

Build a local app bundle:

```sh
bun run tauri:build --bundles app
```

The output is `src-tauri/target/release/bundle/macos/Echo.app`. A local test install can use an ad-hoc signature:

```sh
codesign --force --deep --sign - --identifier app.echostt.desktop src-tauri/target/release/bundle/macos/Echo.app
codesign --verify --deep --strict src-tauri/target/release/bundle/macos/Echo.app
```

For a distribution build, supply an installed signing identity through Tauri's `APPLE_SIGNING_IDENTITY` environment variable:

```sh
APPLE_SIGNING_IDENTITY='Developer ID Application: YOUR NAME (TEAMID)' bun run tauri:build --bundles app
```

The placeholder must be replaced with your installed identity. Ad-hoc signing is neither a distribution signature nor notarization. These commands do not establish a published or notarized release. Keep signing credentials outside the repository. Record the source commit and whether the checkout was dirty with each acceptance build.

## Windows source build

Run on Windows with the pinned Bun and Rust toolchains, Microsoft C++ build tools, Windows SDK, WebView2 and the `x86_64-pc-windows-msvc` Rust target. Install the C++ desktop development workload and the installer tooling required by the selected NSIS or MSI bundle. Add the Rust target if absent:

```powershell
rustup target add x86_64-pc-windows-msvc
```

Run the shared dependency setup, then launch development from PowerShell:

```powershell
bun run tauri:dev
```

Build an installer from the repository root:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\build-windows.ps1
```

The [build script](../scripts/build-windows.ps1) verifies toolchains, installs from the frozen lockfile, runs frontend checks and tests, and produces an NSIS bundle. Pass `-Bundle msi` or `-Bundle all` for other installer outputs. It does not install toolchains or change Git state. Avoid `-SkipCheck` for acceptance builds.

Outputs are under `src-tauri/target/x86_64-pc-windows-msvc/release/bundle`. A Mac-side typecheck of Windows modules does not prove a Windows application build or runtime behavior.

## Required checks

Run all six from the repository root before submitting a change:

```sh
bun run check
bun test
bun run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

Provider loopback tests bind a local TCP listener. A sandbox that denies binding cannot prove those paths; report them as `NOT_RUN: sandbox`, include the failing command and run them on a host that permits loopback. Other failures still require investigation. [CONTRIBUTING](../CONTRIBUTING.md#tests-and-evidence) explains test structure and evidence limits.

For framework migrations, compare sorted `build/` manifests and `build/index.html` against the parent build. Check the static fallback shell, client bootstrap and local asset paths, compare stable asset bytes, and explain generated hash or chunk differences. Instrument the production client's timers and fetch before startup, advance a controlled clock beyond two hours, and confirm no scheduled version timer or timer-triggered `version.json` request. Restore the hourly default deliberately and require that proof to fail. Focus and visibility checks remain enabled and are outside this scheduled-polling check.

Extract every `(selector, property)` pair from `build/**/*.css` in both builds and compare them. Explain every difference, including equivalent shorthand merges (`inset`, `place-self`, `grid-area`), selector merges and browser-target transforms. Check that the alert retains both `-webkit-backdrop-filter` and `backdrop-filter` after minification. Keep the prefixed declaration first and the unprefixed declaration last so Lightning CSS preserves both. Save the comparison script, raw differences and explanations with the migration evidence. Dispatch focus and visibility changes in the production-client proof and assert that requests stay local.

## CI

The checked-in [CI workflow](../.github/workflows/ci.yml) runs on manual dispatch only, because the repository's Actions allowance is limited; nothing runs on pushes or pull requests. Start it from the Actions tab and run the [required checks](#required-checks) locally before submitting a change. Its frontend job installs, checks, tests, builds and audits dependencies. Separate macOS and Windows jobs run native formatting, Clippy and tests, in parallel with the frontend job. A separate Windows job runs `bun run tauri build --ci --no-bundle`, and a cargo-deny job checks Rust advisories, licenses, duplicate versions and sources against `src-tauri/deny.toml`.

CI uploads no package artifact and publishes no release. Its Windows build check does not create an installer. Passing CI proves neither signing and notarization nor physical microphone, shortcut or paste behavior. Branch protection determines required checks.

Gitleaks scans full fetched history on manual runs; its pull request range scan applies only if the pull request trigger is restored. It redacts detected values. The workflow token has only `contents: read`, and pull requests need no repository secrets. Bun downloads and Cargo dependencies and build outputs are cached; superseded runs are cancelled.

GitHub private vulnerability reporting, secret scanning and push protection are enabled on the repository, so the channel in [SECURITY](../SECURITY.md#report-a-vulnerability) is available. [.github/CODEOWNERS](../.github/CODEOWNERS) names the repository owner for ownership review.

## Dependency policy

New registry versions must be at least 72 hours old. The [dependency-age verifier](../scripts/verify-dependency-age.ts) checks registry timestamps, integrity and yanked Cargo releases. Missing evidence fails. Local dependencies are rejected; registry packages need lockfile integrity.

```sh
bun scripts/verify-dependency-age.ts --base <reviewed-base-commit>
bun scripts/verify-dependency-age.ts --full
```

Manual workflow runs perform the full age audit; compare a change against its base locally with `--base`. [Dependabot](../.github/dependabot.yml) checks npm, Cargo and GitHub Actions monthly and opens one grouped pull request per ecosystem, with a seven-day cooldown for version updates. CI still applies the 72-hour gate to security updates. Compatible updates stay within the approved version families; major migrations need separate review.

CI uses `bun audit --audit-level=moderate`: moderate, high and critical npm advisories block it. From `src-tauri`, run `cargo deny --locked --config deny.toml check` for advisories, licenses, duplicate versions and dependency sources. [SECURITY](../SECURITY.md#accepted-dependency-exceptions) records accepted exceptions and their rationale. The Cargo policy lives in [deny.toml](../src-tauri/deny.toml).

Keep bundled font license and source records with their assets. Keep the ParticleHalo source notice equal to its shipped notice; [README](../README.md#contribute) identifies both. The static build copies the shipped notice into `build/licenses/ParticleHalo-MIT.txt`.

## Physical acceptance

Use a packaged build on each target OS. Follow [the product quick start](../README.md#quick-start), then test ordinary speech, tap and hold, immediate re-press, repeated short and long recordings, recording after idle, cancellation during capture and transcription, provider failure and device disconnect. Confirm that each attempt releases the microphone and that the next attempt can acquire it.

Review History, Copy, cancelled and confirmed deletion, clearing and retention. Check Phrasebook add, search and removal, permission recovery and Check key. A valid transcript must remain available if persistence or insertion fails. Test the actual foreground paste targets, held modifiers, concurrent clipboard copies and clipboard managers used for acceptance.

Enable Debug log in Settings before reproducing capture failures. Use the [canonical locations and deletion guidance](../README.md#privacy-and-local-data). Logs must contain lifecycle evidence without audio, transcripts, prompts, keys, clipboard contents or device names and IDs. Enabling diagnostics during an existing capture starts full observation on the next recording.

Measure release-to-paste time on a release build for speech across pauses and sentence continuations. The target of about two seconds for a sixty-second dictation is an acceptance target, not an automated-test guarantee. Record provider, platform, build provenance, observed results and skipped checks. Real microphone acquisition, the macOS Fn event tap, Windows hooks, permission prompts, real provider accounts and OS insertion require host evidence.
