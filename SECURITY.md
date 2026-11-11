# Security

## Report a vulnerability

Use GitHub private vulnerability reporting on this repository: open the **Security** tab and select **Report a vulnerability**. Do not disclose an unfixed vulnerability in a public issue. Include the affected version or commit, platform, reproduction steps, expected and observed behavior, and impact. Replace credentials and private transcript content with `<redacted>`.

Private reporting must be enabled by the repository owner when the public repository is created, as recorded in [development](docs/development.md#ci). If the action is unavailable, do not post sensitive reproduction details publicly. No security email address is published here.

## Boundaries

Echo is a desktop dictation app that sends audio to a provider using the user's credential. Native window identity authorizes commands, OS consent gates microphone access, and provider keys authenticate remote requests. These are separate controls. A renderer-selected label or a successful Check key does not grant OS consent.

Rust owns raw audio, credential resolution, provider calls, SQLite and native paste. Dashboard key drafts pass through the dedicated credential commands. Capture and transcription IPC returns identifiers, text and metadata. Capabilities limit each window; neither renderer can emit arbitrary backend events or use generic clipboard read/write commands. See [IPC and events](docs/architecture.md#ipc-and-events) for authorization and targeted-event disclosure limits.

Provider calls share an HTTPS-only client that rejects redirects, ignores system proxy settings and bounds connect time, request time and response size. Diagnostic events accept fixed fields and omit sensitive payloads. The [architecture reference](docs/architecture.md) owns these mechanisms; [README](README.md#privacy-and-local-data) owns the data inventory, locations and deletion instructions.

## Accepted residual risks

- Providers process uploaded audio and request context under their own policies. Cancellation cannot recall a sent request.
- Local transcript text, prompts and settings are plaintext SQLite data. Filesystem restrictions do not protect them from software running with the user's access.
- Clipboard privacy markers request exclusion. Third-party clipboard managers can ignore them; explicit Copy uses ordinary clipboard behavior. Restoration preserves text only. On macOS, a foreign copy between the final `changeCount` check and restoration can be overwritten. See [delivery](docs/architecture.md#transcript-delivery).
- Storage checks reject symlinked leaf identifier directories, database files and sidecars, including Windows reparse points, but accept redirected ancestors. Ordinary deletion cannot erase backups or retroactively compact free pages from older versions. See [storage](docs/architecture.md#storage).
- The capture energy gate can omit soft speech and brief speech, and later noise after a long pause can pass the gate. Automated signal tests do not establish transcript equivalence. See [capture and segmenting](docs/architecture.md#capture-and-segmenting).
- Find saved key adopts whatever valid item exists under Echo's credential identity for the selected provider; it does not prove that Echo wrote that item. On Windows, software running with the user's access can write that Credential Manager item.
- Native events targeted at the overlay can still reach Tauri listeners using the default `Any` target in another window. The dashboard runtime does not subscribe to partial transcripts; targeting alone is not a confidentiality boundary.

## Accepted dependency exceptions

Reassess these exceptions when updating the dependency graph or changing the listed application paths. [Development](docs/development.md#dependency-policy) owns audit commands and thresholds.

| Advisory | Accepted rationale and follow-up |
| --- | --- |
| `GHSA-gcqx-w48m-w2h4` | Prototype poisoning in deepmerge through 4.3.1 reaches `mergeConfigs` in `@sveltejs/vite-plugin-svelte/src/utils/options.js`, where trusted defaults, project and plugin configuration are merged during tooling execution. This code is outside the shipped runtime. No fixed version is available in the reviewed graph. Keep build configuration trusted and reassess when upstream provides a fix; no replacement-package override is used. |
| `RUSTSEC-2025-0075`, `RUSTSEC-2025-0080`, `RUSTSEC-2025-0081`, `RUSTSEC-2025-0098`, `RUSTSEC-2025-0100` | The unmaintained `unic-*` crates reach runtime capability URL matching through `tauri-utils` and `urlpattern`. The findings indicate unmaintained crates with no known vulnerability. [deny.toml](src-tauri/deny.toml) records the ignores; revisit when Tauri removes this dependency. |
| `GHSA-wrw7-89jp-8q8g` (`RUSTSEC-2024-0429`) | Unsound `VariantStrIter` in glib 0.18.5. glib is in [Cargo.lock](src-tauri/Cargo.lock) only through Tauri's Linux GTK backend. Echo builds for macOS and Windows, where glib is not compiled; [deny.toml](src-tauri/deny.toml) limits the audited graph to those targets. The fix needs glib 0.20, which Tauri 2's GTK bindings do not accept. Revisit when Tauri updates them or before adding a Linux target. |

rusqlite 0.41.1 bundles SQLite 3.54.1, which fixes `RUSTSEC-2026-0459` in the shipped storage layer. tokio 1.53.3 fixes `RUSTSEC-2026-0386`. quinn-proto 0.11.18 clears three advisories for a crate that is locked but not compiled, because reqwest's HTTP/3 feature is off.

SvelteKit 3 removes the prior `GHSA-pfhg-q3qv-9q6r` and `GHSA-27mw-82g8-phjj` exceptions. Application reachability follows [the SPA entry and build configuration](docs/architecture.md#host-and-renderers).

The reviewed dependency refresh fixed six RustSec vulnerabilities and two certificate advisories with rustls 0.23.46 and rustls-webpki 0.103.16. These versions are recorded in [Cargo.lock](src-tauri/Cargo.lock); this statement records the supplied review result and does not claim a new audit or packaged security certification.
