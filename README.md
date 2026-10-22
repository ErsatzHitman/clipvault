# ClipVault

Cross-platform Rust clipboard history with full-text and cloud multimodal semantic search. Local SQLite metadata/indexes, lossless PNG image files, managed file copies, and a native GPU-rendered desktop UI. No telemetry and no local model downloads.

**Status:** implementation under verification. CI builds and tests Linux, Windows, and macOS. A green build is not a claim that every desktop/compositor, permission flow, or provider model has been manually tested. See [platform limits](#platform-limits).

## Install and run

Download your OS artifact from the [CI runs](https://github.com/ErsatzHitman/clipvault/actions). Linux artifacts include a `.deb` and portable tarball. macOS has an ad-hoc-signed `.dmg`; Windows has a per-user installer and portable zip. These development artifacts are not notarized or publisher-signed.

Set the API key **in the environment that launches the app**. Do not put it in a command-line argument or settings file:

```sh
export OPENROUTER_API_KEY='your-key'
clipvault
```

PowerShell:

```powershell
$env:OPENROUTER_API_KEY = 'your-key'
.\clipvault.exe
```

Open Settings and enable cloud processing after reviewing the disclosure. The default OpenRouter model is `google/gemini-embedding-2` in **Native multimodal** mode. It embeds text and image thumbnails in the same space. All existing pending history is indexed after enabling cloud. Keyword search and capture work without an API key.

For Fireworks, set `FIREWORKS_API_KEY`, select Fireworks and **Vision caption + text** mode, and enter text-embedding and vision model IDs available to your account. Fireworks endpoint support differs by model. Caption mode embeds the generated image description; it does not compute a native image embedding. Changing provider/model/mode schedules reindexing. Retrieval never compares vectors from different configurations.

No API call is made before cloud processing is enabled. Turning it off prevents new calls; an already submitted request cannot be recalled. Semantic queries, text, supported text-file contents, and image thumbnails go to the chosen provider. Provider retention policies apply. Secrets copied from allowed apps can reach the cloud.

## Use

- Copy text, images, or regular files. History persists across restarts.
- `Ctrl+Shift+Space` opens history. Change the shortcut in Settings, then restart.
- Type to search. Keyword results appear first; hybrid results combine FTS and semantic ranking. Semantic mode searches only indexed items after the provider responds.
- Up/Down selects an item. Enter restores it and pastes if enabled. Escape or the close button hides history; capture continues. **Quit** exits the app.
- Copy restores without hiding. Paste returns to the last observed app. Failed focus verification leaves the content copied for manual paste.
- Pin important items. Pins survive age/count/storage cleanup and can exceed configured limits. Delete removes a single item; Clear all removes pins too and requires confirmation.
- Pause stops capture and background indexing. Explicit semantic searches can still send queries; disable cloud processing to prevent those.

Settings include history count, age, total payload size, per-item size, polling interval, excluded application substrings, file capture, unknown-source handling, paste behavior, and cloud configuration. Exclusions are case-insensitive foreground-app substrings. Unknown sources are excluded by default.

## Platform limits

| Platform | Capture | Shortcut / paste | Permissions and limits |
|---|---|---|---|
| Windows | Text, images, regular files | Registered global shortcut; Ctrl+V | Cannot paste into elevated applications from a normal app. Clipboard contention is retried by later polling. User-data ACL is inherited. |
| macOS | Text, images, regular files | Global shortcut; Cmd+V | Allow Accessibility for simulated input. Foreground observation uses System Events and may require Automation permission. Unsigned downloads may need approval in Privacy & Security. Clipboard access may prompt under OS policy. |
| Linux X11 | Text, images, regular files | X11 global shortcut; Ctrl+V | A running X server/window manager is required. Clipboard ownership requires the app to remain running. `_NET_ACTIVE_WINDOW` and `_NET_WM_PID`/WM_CLASS identify apps. |
| Linux Wayland | wlroots data-control or XWayland fallback | Shortcut/app identification/paste restricted | Source identification fails closed. X11 is the fully supported desktop path. Enable unknown-source capture only if you accept losing exclusion guarantees; do not treat that setting as secure source detection. |

The first clipboard value at startup or after leaving a paused/excluded state is baselined and skipped, so old sensitive clipboard values are not ingested on resume. Copy again after resuming if needed.

Only regular files are managed; directories and symlinks are rejected. A bundle is limited to 64 files and the item-size cap. Names and common UTF-8 text/code contents are searchable. PDF/Office/archive binaries are preserved but not parsed. File restore uses managed copies; the app does not retain original filesystem permissions. Clipboard history is polling-based and can miss rapid intermediate changes. Rich HTML/RTF is represented by plain text; audio and arbitrary custom formats are not supported.

## Storage and security

Default data location follows the OS application-data convention (`directories::ProjectDirs`). The exact path is displayed in Settings. Use `clipvault --data-dir /path` for a separate profile. One instance per data directory is enforced with a file lock.

- `history.sqlite3` and WAL/SHM: metadata, text, FTS5, normalized float32 vectors.
- `blobs/<sha256>/`: PNG images, file manifests, managed file copies. No image bytes in the search index.
- `settings.toml`: configuration, never API keys.

History is **not encrypted**. Use OS disk encryption. Root/admin, same-user processes, backups, swap, or malware can read it. Unix permissions are 0700/0600. Clearing uses SQLite secure deletion, checkpointing, vacuum, and blob cleanup, but cannot guarantee forensic erasure on SSDs or backups. Per-app exclusions observe the foreground app, not an authenticated clipboard owner; they reduce accidental collection and are not a security boundary. The arboard backend does not expose password-manager concealment markers; use exclusions and Pause. Do not rely on marker-based secret filtering.

Retention runs on startup and every minute. Pins can prevent size limits from being met. The size limit covers stored payloads, not SQLite/index overhead. Lost/unreferenced blob directories are collected. Text extraction is bounded to 100,000 characters per supported text file; cloud text inputs are capped at 12,000 characters per item/query. Long items remain fully available to keyword search, while semantic coverage uses the bounded prefix. Native image inputs are thumbnailed to 1280 pixels. OS clipboard backends may decode a full image before the application can enforce the item-size cap. Capture and search queues are bounded; an overloaded capture queue retries the current observation rather than blocking the UI.

## Build

Rust 1.88 or later. Dependencies are locked. Linux development packages (Debian/Ubuntu):

```sh
sudo apt-get install libx11-dev libxtst-dev libxdo-dev libxcb1-dev libxrandr-dev libxi-dev libgl1-mesa-dev libwayland-dev libxkbcommon-dev
cargo build --locked --release
./target/release/clipvault
```

`cargo fmt --check` is lightweight. Full `cargo test --locked --all-targets`, Clippy, release builds, clipboard round trips, and packaging run on all three OSes in GitHub Actions. Do not run full suites on the project owner's laptop. Provider calls require an external key and are not made by public CI. `--clipboard-self-test` deliberately replaces the clipboard and is intended for disposable CI desktops only.

See [architecture](docs/ARCHITECTURE.md) for threading, vector-index trade-offs, and threat model. See [verification](docs/VERIFICATION.md) for what has actually been tested.

## References

- [eframe 0.33.3](https://docs.rs/eframe/0.33.3/eframe/)
- [global-hotkey 0.7](https://docs.rs/global-hotkey/0.7.0/global_hotkey/)
- [enigo 0.6](https://docs.rs/enigo/0.6.1/enigo/)
- [OpenRouter embedding API](https://openrouter.ai/docs/api/reference/embeddings)
- [Fireworks embeddings](https://docs.fireworks.ai/api-reference/embeddings)

MIT licensed.
