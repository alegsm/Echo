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

The explicit build target is Chrome 111, Edge 111, Firefox 114 and Safari 16.4. Supported desktop webviews are macOS WebKit 16.4 or later and Windows WebView2 evergreen. WebKit 16.4 ships with macOS 13.3, so the packaged Mac app needs macOS 13.3 or later; `bundle.macOS.minimumSystemVersion` in [tauri.conf.json](../src-tauri/tauri.conf.json) writes that minimum to `LSMinimumSystemVersion` in the app's `Info.plist`.

## Before you run a build

Background mode adds two behaviors that reach outside a checkout. Read both before you run `bun run tauri:dev` or a packaged build.

- **Single instance.** Echo allows one running copy per app identifier. After a packaged Echo of this version has started, `bun run tauri:dev` with the same identifier (`app.echostt.desktop`) hands its launch to the packaged Echo through the single-instance plugin and exits, so no development window opens. Quit the packaged Echo first, from the tray's **Quit Echo** or, on macOS, with Cmd+Q while its dashboard is open. With the dashboard hidden, Echo is out of the Dock and Cmd+Tab, so use **Quit Echo**. A running development build does the same to a packaged launch. A development build also writes a real login item for its own executable if you turn on **Start Echo at login**. Turn it off before you quit.
- **Settings downgrade.** This version stores `runInBackground` and `launchAtLogin` in the settings row, and any settings write adds both. Echo 0.3.2 rejects a row with unknown keys and does not start. Back up `echo.sqlite3` (see [locations](../README.md#locations)) before you run this version against a database you want to keep. To go back to 0.3.2, restore the backup or remove both keys from the row. [Storage](architecture.md#storage) has the details.

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

## Linux source build

Run on x86_64 Linux with the pinned Bun and Rust toolchains. On Debian or Ubuntu, install the WebKitGTK, tray, audio and D-Bus development packages:

```sh
sudo apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libasound2-dev libdbus-1-dev libssl-dev pkg-config
```

Other distributions need the same libraries under their own package names, as listed in the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/#linux). Run the shared dependency setup, then launch development:

```sh
bun run tauri:dev
```

Build a package from the repository root:

```sh
bun run tauri build --bundles deb
```

Pass `--bundles rpm` or `--bundles appimage` for the other formats. Outputs are under `src-tauri/target/release/bundle`.

The [platform layer](../src-tauri/src/platform/linux.rs) picks its input path from the session. On X11 it uses XInput2 for Control+Super and XTest for paste. On Wayland it reads `/dev/input` for the chord and runs `wtype` or `ydotool` for paste. The X11 path has an integration test that drives a real X server; it needs `xvfb`, `xdotool` and `xclip`:

```sh
cd src-tauri && xvfb-run -a cargo test linux_x11 -- --ignored --test-threads=1
```

Xvfb has only a synthetic keyboard, so that test proves the X11 protocol path, not a physical keyboard, the Wayland path or behavior in each desktop environment. Check those by hand as in [Physical acceptance](#physical-acceptance).

## Publish a release

A release puts signed builds for both platforms, their checksums and the `latest.json` manifest on a draft GitHub release, then publishes it. Installed copies of Echo read `latest.json` to offer the update. [Architecture](architecture.md#updates) describes the update client. Installed copies never downgrade, so a bad release is corrected only by a higher fixed version or a manual reinstall.

**Immutable releases.** The repository has immutable releases turned on. Publishing locks the release's files and its tag: no file can be added, replaced or deleted afterwards, and the tag cannot be moved, deleted or reused. Only the title, the notes, the pre-release mark and the Latest mark can still change. Every file therefore goes on the draft, and the draft is checked before it is published. While it is a draft, `gh release upload --clobber` replaces any file.

**Update key.** The release operator generates the update key pair once, at a hidden prompt, straight into 1Password; the key has no password. The public half replaces `ECHO_UPDATE_PUBLIC_KEY_NOT_SET` in `plugins.updater.pubkey` of [tauri.conf.json](../src-tauri/tauri.conf.json) in the release commit. While the placeholder is committed, [update-manifest.ts](../scripts/update-manifest.ts) and `build-windows.ps1 -Release` refuse to run. The macOS build does not read the pubkey; a macOS release is refused the same way when update-manifest.ts runs, so no manifest can offer it. The private key reaches a build only through the `TAURI_SIGNING_PRIVATE_KEY` environment variable, injected by the secret store, for example with 1Password CLI's `op run`. No script reads it from a file in the repository, prints it or passes it as an argument. `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` can stay unset: the Tauri CLI assumes an empty password and builds without a prompt.

**Release overlay.** [tauri.release.conf.json](../src-tauri/tauri.release.conf.json) turns on `createUpdaterArtifacts`, which signs the updater artifact with the key. Only release and staging builds pass it with `--config`, so `bun run tauri:build --bundles app` and CI's `tauri build --ci --no-bundle` still succeed without a key.

1. Set the same version in [package.json](../package.json), [tauri.conf.json](../src-tauri/tauri.conf.json) and [Cargo.toml](../src-tauri/Cargo.toml), then tag the commit `v<version>`, push the tag and create the draft release from it. Steps 2 to 5 upload to this draft. Push the tag only once the release commit is final: the tag ruleset blocks moving or deleting `v*` tags, so a fix found during the builds needs a new version.

   ```sh
   git push origin v<version>
   gh release create v<version> --draft --verify-tag --title "Echo <version>" --notes-file <notes.md>
   ```

2. On an Apple Silicon Mac, build with the Developer ID identity for public releases:

   ```sh
   APPLE_SIGNING_IDENTITY=<sha1> TAURI_SIGNING_PRIVATE_KEY=<from the secret store> bun run tauri:build --bundles app,dmg --config src-tauri/tauri.release.conf.json
   ```

   Notarize and staple the DMG from `src-tauri/target/release/bundle/dmg/` and upload it as `Echo-macos.dmg`. From the same build, upload `src-tauri/target/release/bundle/macos/Echo.app.tar.gz` as `Echo-macos.app.tar.gz` and its `.sig` as `Echo-macos.app.tar.gz.sig`. The app inside the tar.gz is not stapled: the DMG's notarization covers its code hash, and the updater's own download carries no quarantine flag.
3. On Windows, with the key in the environment, run:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\scripts\build-windows.ps1 -Release
   ```

   `-Release` builds the NSIS installer only, refuses a missing key or the placeholder pubkey, passes the release overlay and copies the signed installer to `Echo-windows-setup.exe` and `Echo-windows-setup.exe.sig` in `src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis`. Upload both. A Windows build regenerates `src-tauri/gen/schemas/windows-schema.json`; a Mac build cannot. Commit that file from the Windows checkout when it changes, and never edit it by hand.
4. The builds come from two machines, so download the draft's files to one directory before writing anything that describes them. Then write `SHA256SUMS.txt` for the DMG, the tar.gz and the setup exe, and upload it:

   ```sh
   gh release download v<version> --dir <dir>
   (cd <dir> && shasum -a 256 Echo-macos.dmg Echo-macos.app.tar.gz Echo-windows-setup.exe > SHA256SUMS.txt)
   gh release upload v<version> <dir>/SHA256SUMS.txt
   ```

5. Write the manifest from the two `.sig` files in `<dir>`, `Echo-macos.app.tar.gz.sig` and `Echo-windows-setup.exe.sig`, and upload it to the draft:

   ```sh
   bun scripts/update-manifest.ts --tag v<version> --assets <dir>
   gh release upload v<version> <dir>/latest.json
   ```

   The script reads both `.sig` files from `<dir>`, because a draft's files have no public download URL yet; `gh release download` reads them with your login. It exits non-zero, writing nothing, when one is missing, when a signature's key id differs from the configured pubkey, when the pubkey is the placeholder or not a minisign public key, or when the tag is not exactly `v` followed by package.json's version. The manifest lists `darwin-aarch64` and `windows-x86_64` with URLs under `releases/download/<tag>/`, which resolve once the release is published. Current Windows builds ignore the Windows `.sig` and the `windows-x86_64` entry, because they only open the releases page ([architecture](architecture.md#updates)); the scripts still produce and require both, for 0.5.0 installs, which install their own updates, and for signed builds once the switch includes Windows.
6. Check that the draft holds exactly the seven files:

   ```sh
   gh release view v<version> --json isDraft,assets --jq '[.isDraft, (.assets | map(.name) | sort)]'
   ```

   It must print `true` and `Echo-macos.app.tar.gz`, `Echo-macos.app.tar.gz.sig`, `Echo-macos.dmg`, `Echo-windows-setup.exe`, `Echo-windows-setup.exe.sig`, `SHA256SUMS.txt` and `latest.json`. A file missing at publication cannot be added later. A published release without `latest.json` that is marked Latest leaves both update endpoints answering 404, so no installed copy is offered any update until another release is marked Latest.
7. Publish the draft, mark it Latest and check GitHub's release attestation:

   ```sh
   gh release edit v<version> --draft=false --latest
   gh release verify v<version>
   ```

   Publishing locks the files and the tag and records an attestation of the tag, the commit and each file's digest. If `gh release verify` finds no attestation straight after publishing, wait a minute and run it again. `gh release verify-asset v<version> <file>` checks a downloaded file against it.

Before publication, replace a draft's file with `gh release upload --clobber`. After any build file or `.sig` changes, run steps 4 and 5 again with `gh release download --clobber`, so `<dir>` holds the new files, and replace `SHA256SUMS.txt` and `latest.json` with `--clobber`. A manifest written from an old `.sig` passes the key check but fails signature verification on every copy that installs the update once the release is published. A published release cannot be repaired. To stop offering it, mark the previous release Latest:

```sh
gh release edit v<previous version> --latest
```

Both update endpoints, the README's download links, echostt.app's download links and the Windows download button (`releases/latest`) follow the release marked Latest. Installed copies of the bad version keep it, because Echo never downgrades. Copies older than the previous version are offered the previous version. The bad release still heads the releases page, so add a warning to its notes or mark it pre-release; both stay editable. Ship the fix as a higher version, then mark that release Latest.

## Stage an update

[update-staging.ts](../scripts/update-staging.ts) proves the whole update loop on one machine without the update key or GitHub. It builds vN (package.json's version) and vN+1 (the next patch) from the checkout with the release overlay and a throwaway key pair, and serves vN+1's `latest.json` and artifact on `127.0.0.1`. The key pair, both config overlays and copies of the builds stay in one new `echo-update-staging-*` directory under the OS temp dir. The overlays set the version, the staging pubkey, the loopback endpoint and `dangerousInsecureTransportProtocol`; `ECHO_APP_VERSION` makes the sidebar show each build's own version. No tracked file changes. The two release builds take several minutes each. On Windows, staging proves the check and the link only: the vN build reads the loopback manifest and opens the releases page on GitHub, and nothing is downloaded or installed.

```sh
APPLE_SIGNING_IDENTITY=<personal identity sha1> bun scripts/update-staging.ts
```

On macOS, set `APPLE_SIGNING_IDENTITY` to a personal Developer ID, never the identity for public releases, so the installed copy keeps its OS permissions across the update. Without it the script says so and builds unsigned bundles. The script never installs or starts Echo. It prints the vN app (on Windows, the vN installer) to install by hand, the manifest URL and each request it serves. Ctrl+C stops the server and keeps the directory. To check that a tampered artifact is refused, serve the same directory again with one artifact byte flipped:

```sh
bun scripts/update-staging.ts --serve <staging directory> --tamper
```

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

The checked-in [CI workflow](../.github/workflows/ci.yml) runs on manual dispatch only; nothing runs on pushes or pull requests. Start it from the Actions tab and run the [required checks](#required-checks) locally before submitting a change. Its frontend job installs, checks, tests, builds and audits dependencies. Separate macOS, Windows and Linux jobs run native formatting, Clippy and tests, in parallel with the frontend job. The Linux job also runs the X11 integration test under Xvfb. A separate Windows job runs `bun run tauri build --ci --no-bundle`, and a cargo-deny job checks Rust advisories, licenses, duplicate versions and sources against `src-tauri/deny.toml`.

CI uploads no package artifact and publishes no release. Its Windows build check does not create an installer. Passing CI proves neither signing and notarization nor physical microphone, shortcut or paste behavior. Branch protection determines required checks.

Gitleaks scans full fetched history on manual runs; its pull request range scan applies only if the pull request trigger is restored. It redacts detected values. The workflow token has only `contents: read`, and pull requests need no repository secrets. Bun downloads and Cargo dependencies and build outputs are cached; superseded runs are cancelled.

A zizmor job, pinned by version and wheel hash in [.github/zizmor/requirements.txt](../.github/zizmor/requirements.txt) and kept current by Dependabot, audits the workflows for template injection, dangerous triggers, cache poisoning, unpinned and impostor actions and credential persistence. The repository allows only GitHub-owned actions and `oven-sh/setup-bun`, and requires every action to be pinned to a full commit SHA; a new third-party action must be added to that allow-list first.

Workflow rules that keep this repository safe to accept outside pull requests:

- Do not add `pull_request_target`, `workflow_run`, `issue_comment` or `issues` triggers. Use `pull_request` if automatic runs are restored.
- Do not run an AI agent with shell or write tools on issue, pull request or comment text.
- Pass untrusted values such as titles and branch names to `run:` through `env:`, quoted; never expand them with `${{ }}` inside a script.
- A future release or signing job restores no caches and requests only the permissions it needs.

GitHub private vulnerability reporting, secret scanning and push protection are enabled on the repository, so the channel in [SECURITY](../SECURITY.md#report-a-vulnerability) is available. [.github/CODEOWNERS](../.github/CODEOWNERS) names the repository owner for ownership review.

## Dependency policy

New registry versions must be at least 72 hours old. The [dependency-age verifier](../scripts/verify-dependency-age.ts) checks registry timestamps, integrity and yanked Cargo releases. Missing evidence fails. Local dependencies are rejected; registry packages need lockfile integrity.

```sh
bun scripts/verify-dependency-age.ts --base <reviewed-base-commit>
bun scripts/verify-dependency-age.ts --full
```

Manual workflow runs perform the full age audit; compare a change against its base locally with `--base`. [Dependabot](../.github/dependabot.yml) checks npm, Cargo, GitHub Actions and the pinned zizmor wheel monthly and opens one grouped pull request per ecosystem, with a seven-day cooldown for version updates. CI still applies the 72-hour gate to security updates. Compatible updates stay within the approved version families; major migrations need separate review.

CI uses `bun audit --audit-level=moderate`: moderate, high and critical npm advisories block it. From `src-tauri`, run `cargo deny --locked --config deny.toml check` for advisories, licenses, duplicate versions and dependency sources. [SECURITY](../SECURITY.md#accepted-dependency-exceptions) records accepted exceptions and their rationale. The Cargo policy lives in [deny.toml](../src-tauri/deny.toml).

Keep bundled font license and source records with their assets. Keep the ParticleHalo source notice equal to its shipped notice; [README](../README.md#contribute) identifies both. The static build copies the shipped notice into `build/licenses/ParticleHalo-MIT.txt`.

## Physical acceptance

Use a packaged build on each target OS. Follow [the product quick start](../README.md#quick-start), then test ordinary speech, tap and hold, immediate re-press, repeated short and long recordings, recording after idle, cancellation during capture and transcription, provider failure and device disconnect. Confirm that each attempt releases the microphone and that the next attempt can acquire it.

Review History, Copy, cancelled and confirmed deletion, clearing and retention. Check Phrasebook add, search and removal, permission recovery and Check key. A valid transcript must remain available if persistence or insertion fails. Test the actual foreground paste targets, held modifiers, concurrent clipboard copies and clipboard managers used for acceptance.

Enable Debug log in Settings before reproducing capture failures. Use the [canonical locations and deletion guidance](../README.md#privacy-and-local-data). Logs must contain lifecycle evidence without audio, transcripts, prompts, keys, clipboard contents or device names and IDs. Enabling diagnostics during an existing capture starts full observation on the next recording.

Measure release-to-paste time on a release build for speech across pauses and sentence continuations. The target of about two seconds for a sixty-second dictation is an acceptance target, not an automated-test guarantee. Record provider, platform, build provenance, observed results and skipped checks. Real microphone acquisition, the macOS Fn event tap, Windows hooks, permission prompts, real provider accounts and OS insertion require host evidence.

### Linux

Run these on an installed `.deb`, `.rpm` or AppImage, on an X11 session and on a Wayland session. Record the distribution, desktop and session type with each run.

1. X11: hold Control+Super in a text editor, dictate and confirm the paste lands. Repeat in a terminal and confirm it pastes with Control+Shift+V.
2. Wayland with the user in the `input` group and `ydotool` 1.0 or newer running: hold Control+Super, dictate and confirm the paste lands in a text editor. In a terminal the transcript stays on the clipboard for a manual Control+Shift+V.
3. Wayland without the `input` group: Keybinds shows "Native shortcut unavailable" with the input group hint, and the fallback shortcut records while an Echo window is focused.
4. Clipboard: after a paste with **Restore clipboard after paste** on, Control+V gives back what was copied before.
5. Background mode, with a tray that supports app indicators (on GNOME, the AppIndicator extension): the icon appears, Open Echo shows the dashboard and Quit Echo ends the process.
6. Start Echo at login writes `~/.config/autostart/Echo.desktop`. Sign out and in and confirm Echo starts hidden with background mode on. Turning the switch off removes the file.
7. The app menu lists Echo under Utility, the dock or taskbar shows Echo's icon, and `/usr/bin/echo` still belongs to coreutils after a `.deb` install.

### Background mode

Run these on a packaged build on each OS, started from `/Applications` or a similar folder rather than from Downloads or a disk image, and quit any development build first. The setting names are **Keep Echo running when the dashboard closes** and **Start Echo at login** in Settings > Background. The [architecture note](architecture.md#background-mode) describes the behavior behind each item.

1. Default install: closing the dashboard (the macOS red button, the Windows Close button or Alt+F4) quits Echo. No tray icon shows and no login item exists.
2. Turn on **Keep Echo running when the dashboard closes**. The icon appears at once. Close the dashboard, dictate into another app with the shortcut and confirm the paste lands. Choose **Open Echo** and confirm History shows the dictation. Choose **Quit Echo** and confirm the process ends in Activity Monitor or Task Manager.
3. macOS: close the dashboard and confirm Echo leaves the Dock and Cmd+Tab while the menu bar icon stays (`lsappinfo info -only ApplicationType Echo` reads `UIElement`). Choose **Open Echo** and confirm the Dock icon returns before the dashboard comes to the front with focus (`Foreground`). Repeat the return by starting Echo again. With the dashboard open, Cmd+Q and the Dock icon's Quit each quit Echo completely, menu bar icon included. A click on the menu bar icon opens the menu, and the icon renders in light and dark menu bars.
4. Windows: a left-click on the icon shows the dashboard and a right-click opens the menu. Alt+F4 hides the dashboard. Starting Echo again from the Start menu shows the running dashboard and leaves one process. Repeat the second-start check on macOS by running the executable from Terminal.
5. Turn on **Start Echo at login**. The OS lists Echo (System Settings > General > Login Items on macOS, Task Manager > Startup apps on Windows). Sign out and in. With background mode on, Echo starts with the dashboard hidden and the icon showing, and on macOS with no Dock icon. With it off, the dashboard opens. An unsigned or ad-hoc macOS build lists a weak name; that is expected.
6. Break the login record, then repair it. On Windows, disable Echo in Task Manager > Startup apps. On macOS, delete `~/Library/LaunchAgents/Echo.plist`. Reopen Settings > Background. The card shows "Not registered with Windows" or "Not registered with macOS" and asks you to turn the switch off and on. Doing so restores the entry and clears the line. Also turn Echo off under System Settings > Allow in the Background on macOS and confirm that no mismatch line appears; this is a known limit.
7. macOS: with **Start Echo at login** on, move `Echo.app` to another folder and start it from there. Sign out and in and confirm the login item still starts Echo from the new location.
8. Windows: uninstall Echo with the NSIS installer while **Start Echo at login** is on. Startup apps no longer lists Echo and the `Echo` value is gone from `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. Record the installer type, because an MSI uninstall is not covered by this check.
9. Turn both settings off. The icon disappears at once, the OS no longer lists Echo, and on macOS `Echo.plist` is gone. On Windows the `Echo` values under `Run` and `Explorer\StartupApproved\Run` are gone.

### Updates

Run these with [update-staging.ts](../scripts/update-staging.ts) on each OS, quitting any other Echo first. Echo writes `update_check`, `update_install` and, on Windows, `update_open` lines to its log (see [locations](../README.md#locations)).

On macOS:

1. Install the printed vN build: move `Echo.app` into `/Applications`. Start it. The sidebar shows an icon button beside `v<N>` named "Update Echo to v<N+1>", and the log shows `update_check outcome=available`.
2. Turn on **Keep Echo running when the dashboard closes**. The tray menu reads Open Echo, Update Echo, a separator and Quit Echo.
3. Select the sidebar button. It shows download progress, then installing, and Echo restarts on vN+1 with no button and one Echo process. The server output shows one download request even if you select the button or the tray item again during the download, and the tray item reads "Updating Echo…" and is disabled.
4. Repeat with a fresh staging run, choosing **Update Echo** from the tray with the dashboard hidden.
5. Start a dictation and request the install before it ends. The button says the install waits for the dictation to end, the paste lands, and the install starts at least 10 seconds after the last capture, job or paste.
6. Serve again with `--tamper` against a vN install and request the install. The button reads "Update failed. Try again", vN keeps running and nothing is installed. Selecting it again starts a new download.
7. Before the first `latest.json` exists, start a release build of the shipped configuration. The log shows `update_check outcome=none`, not `failed`, which proves HTTPS and the redirect chain reach GitHub.

On Windows:

1. Run the printed vN installer and start Echo. The sidebar shows a download icon button beside `v<N>` named "Download Echo v<N+1>", whose tooltip adds "Opens the download page in your browser; run the installer there to update", and the log shows `update_check outcome=available`.
2. Turn on **Keep Echo running when the dashboard closes**. The tray menu reads Open Echo, Download Echo Update, a separator and Quit Echo.
3. Select the sidebar button. The default browser opens `https://github.com/alegsm/Echo/releases/latest`, Echo keeps running on vN with the button unchanged, and the log shows `update_open outcome=opened version=<N+1>`. The server output shows only the `latest.json` request, never the artifact.
4. Hide the dashboard and choose **Download Echo Update** from the tray. The same page opens, the dashboard stays hidden and the log shows another `update_open outcome=opened`.
5. The failed-open path is not checked by hand: Windows 11 offers no practical way to make the shell fail to open an `https` URL, and without a handler ShellExecuteW shows an app picker and reports success. `download_page_opens_only_the_fixed_url` and `notify_loop_walks_from_offer_to_the_download_page_over_loopback` in [updater_tests.rs](../src-tauri/src/services/updater_tests.rs), and the notify test in [dashboard-update.test.ts](../tests/dashboard-update.test.ts), prove the `download_page_failed` state, its label and announcement, the `update_open outcome=failed` line and the retry.
6. Before the first `latest.json` exists, start a release build of the shipped configuration. The log shows `update_check outcome=none`, not `failed`.
7. Scan the release `echo.exe` for the updater plugin, for example with `Select-String -Path echo.exe -Pattern 'tauri-plugin-updater','tauri_plugin_updater','update_install outcome' -SimpleMatch -Encoding ascii`. No line matches.

Record the platform, build provenance and the server output with each run.
