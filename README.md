# Echo

Echo is an [MIT-licensed](LICENSE) desktop dictation app for macOS and Windows. Speak into a focused text field, then review the transcript in History. Echo uses your API key to send recorded audio to your selected transcription provider.

Hold Fn on macOS or Control+Windows on Windows to record. Release to finish. For hands-free recording, tap once to start, then tap again to finish. Settings also offers a configurable shortcut.

Future screenshot: `docs/images/echo.png`. No screenshot is included yet.

## Install

- **macOS:** release downloads are planned. No published or notarized package is offered here yet. Use the [macOS source-build recipe](docs/development.md#macos-source-build).
- **Windows:** build from source with the [Windows source-build recipe](docs/development.md#windows-source-build). No Windows release download is offered here.

Both recipes include prerequisites, pinned toolchains and commands. Packaged microphone, shortcut and paste behavior still needs [physical acceptance](docs/development.md#physical-acceptance).

Runtime requirements: macOS WebKit 16.4 or later, or Windows WebView2 evergreen.

## Quick start

1. Open Echo's dashboard. It opens on History. First-launch setup offers Permissions, then Provider and key when no provider has a key available yet. You can keep browsing and reopen setup from Settings.
2. Grant the permissions listed in setup or Settings > Permissions. macOS needs Microphone for capture, Input Monitoring for Fn and Accessibility for automatic paste. Windows lists Microphone; use Windows Settings if access is denied or unknown. Re-check after changing OS settings. On macOS, the separate Globe tap action control can change the system Fn action and restore its previous value.
3. In Settings > Provider, choose a provider and paste your key into the password field. Keys are saved in Keychain on macOS or Credential Manager on Windows. Select **Check key**. For a nonempty key draft, Echo explains where the key will be saved and asks you to acknowledge with **Save to Keychain** or **Save to Credential Manager**. Cancel or Escape keeps the draft without saving or checking it. After acknowledgement, Echo saves and checks the credential with the provider without recording from the microphone. Each replacement requires acknowledgement again. If your settings are new but your key is still in Keychain or Credential Manager, select **Find saved key** instead of pasting it, then select **Check key**. Echo reads the store only when you select Find saved key.
4. Focus a text field in the app where you want the transcript. Hold the shortcut, speak and release, or tap once, speak and tap again. Echo finishes transcription and inserts the result when automatic paste is enabled. Stop in the overlay also finishes a recording. Cancellation discards that dictation's pending result.
5. Review the final result in History. Copy is available if insertion fails or automatic paste is disabled. If a transcription piece fails, Echo retries it once. It keeps other speech and marks any unrecovered piece with “[Transcription gap]”. The overlay reports retries and gaps during recording. If every piece fails, History keeps the gap markers for review without automatic paste or copy. Existing transcripts cannot be edited in Echo.

In the final minute before the ten-minute recording limit, the overlay shows a countdown above the orb and pulses its light red once per second. Reduced motion keeps the red light steady. The warning also applies while holding the shortcut.

Settings controls language, custom prompts, automatic paste, clipboard restoration, Debug log, keeping Echo running when the dashboard closes and starting Echo at login. Phrasebook lets you add, search and remove spelling terms. Terms bias recognition on all providers; they do not guarantee a spelling. Custom prompts apply only to Groq and OpenAI.

By default, closing the dashboard quits Echo. To keep dictating after you close it, turn on **Keep Echo running when the dashboard closes** in Settings > Background. Closing the dashboard then hides it, and the shortcut, overlay and transcription keep working while an icon shows in the macOS menu bar or the Windows notification area. To bring the dashboard back, choose **Open Echo** from the icon's menu, left-click the icon on Windows, click the Dock icon on macOS, or start Echo again. To quit while it runs in the background, choose **Quit Echo** from the icon's menu, or press Cmd+Q on macOS. **Start Echo at login** adds a per-user login item. After you sign in, Echo then starts with the dashboard hidden if background mode is on, and with the dashboard open if it is off. The [development guide](docs/development.md#background-mode) lists what to check on a packaged build.

### History

History supports Copy, individual deletion, clearing and retention changes. Delete and Clear require confirmation. Cancel or Escape leaves the records intact. Retention offers 7, 30 or 90 days and Forever; a reduction that deletes records also requires confirmation. History and settings updates synchronize across Echo's windows.

## Transcription providers

Echo offers one code-defined model per provider. These model names come from [the native provider catalog](src-tauri/src/providers/mod.rs). This is the single provider catalog for the documentation.

| Provider | Model |
| --- | --- |
| Groq | `whisper-large-v3-turbo` |
| OpenAI | `gpt-transcribe` |
| xAI | `grok-voice-transcribe-2.0` |
| ElevenLabs | `scribe_v2` |
| Deepgram | `nova-3` |
| Mistral | `voxtral-mini-transcribe-3-0` |

Echo reads saved keys from the OS credential store when you explicitly find or check a saved key, or transcribe. Configured indicators use non-secret saved-key metadata, so they do not prove that an OS item still exists or that a key works. Dashboard load and settings refresh do not inspect saved secrets. Check key reports whether the configured credential worked at the time of the check; provider network access and account limits still apply to later dictations.

## Privacy and local data

- **Audio:** Rust captures and holds audio in native memory. Raw capture bytes do not pass through renderer IPC and are not saved to SQLite or diagnostic files. Audio is discarded after completion, failure or cancellation, subject to active native work releasing it.
- **Provider uploads:** recorded audio and the selected language, prompt and Phrasebook terms can leave the device for provider processing. Some chunks upload while recording continues. Cancellation suppresses results but cannot recall a request already sent. Provider retention and account policies apply.
- **Renderer text:** the webviews receive transcript text and metadata. The overlay can show partial text during recording. Final transcripts reach History and clipboard delivery.
- **Credentials:** keys entered in dashboard drafts pass to native code for OS credential storage. Capture and transcription responses do not return saved keys to the renderer. Keys are not stored in SQLite or logs.
- **SQLite:** the local database stores plaintext transcript text, settings, prompts, language, provider/model, recording metadata, timings and delivery outcomes. It stores no recorded audio or API keys.
- **Clipboard:** automatic delivery requests exclusion from OS history and clipboard managers. Third-party managers may ignore those markers. Restoration preserves text only, so images and files are not restored. Explicit Copy uses ordinary clipboard behavior. See [delivery mechanics and limits](docs/architecture.md#transcript-delivery).
- **Diagnostics:** Debug log is off by default. It records bounded lifecycle events and aggregate signal statistics, excluding audio, transcripts, prompts, credentials, clipboard contents, raw provider bodies and device names or IDs. Settings > Debug log displays the actual log file path. Logs stay local unless you share them.

### Locations

| Data | macOS | Windows |
| --- | --- | --- |
| Database | `~/Library/Application Support/app.echostt.desktop/echo.sqlite3` | `%LOCALAPPDATA%\app.echostt.desktop\echo.sqlite3` |
| Logs | `~/Library/Logs/app.echostt.desktop/` | `%LOCALAPPDATA%\app.echostt.desktop\logs\` |
| Start Echo at login | `~/Library/LaunchAgents/Echo.plist` | `Echo` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` |

The log directory contains `Echo.log`, `Echo.1.log` and `Echo.2.log`. Each is bounded to 1 MiB by [the native log writer](src-tauri/src/bounded_log.rs).

The login record holds only the path to Echo and the `--launched-at-login` argument, never transcripts, settings or secrets. Turning **Start Echo at login** off removes it. On Windows it also removes the Task Manager startup switch for Echo under `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run`.

### Delete local data

For ordinary history cleanup, use History's Delete, Clear or retention controls. Settings and provider credentials remain. SQLite overwrites deleted transcript content from this version onward and attempts to truncate the write-ahead log. If cleanup remains pending because the database is busy, a later purge or launch attempts it again.

To remove diagnostic files, turn off Debug log, quit Echo, then delete `Echo.log`, `Echo.1.log` and `Echo.2.log` from the log directory above. This does not clear History, settings or credentials.

For a full local reset, first turn off **Start Echo at login** in Settings > Background, restore any Fn preference changed through Echo on macOS and remove saved provider keys through Settings > Provider. Quit Echo, then delete `echo.sqlite3` and any adjacent `echo.sqlite3-wal` and `echo.sqlite3-shm` files from the data directory above. Remove the three log files separately. Deleting the Echo app on macOS does not remove the login record, so turn the switch off before you delete or uninstall Echo on either OS. This resets local History and settings; removing the database alone does not remove OS credentials. Keys left in Keychain or Credential Manager can be reused through **Find saved key** after the reset, so remove them in Settings > Provider first if you don't want that.

Deletion does not retroactively compact free pages left by older versions, erase backups, or erase data already sent to providers or copied by other apps. [Storage mechanics](docs/architecture.md#storage) explain these limits.

## Contribute

Start with [CONTRIBUTING](CONTRIBUTING.md), then use the canonical [development guide](docs/development.md) and [architecture reference](docs/architecture.md). Report security issues through [SECURITY](SECURITY.md).

The overlay's [ParticleHalo implementation](src/lib/ui/overlay/ParticleHalo.svelte) points to the upstream MIT notice in its source comment. The [source MIT notice](src/lib/ui/overlay/ParticleHalo.LICENSE), copyright 2026 Alejandro Sanchez, is also [shipped with the app](static/licenses/ParticleHalo-MIT.txt). Bundled fonts have [source and license records](static/fonts/SOURCES.md). Artwork maintenance lives in [the brand guide](design/brand/README.md).
