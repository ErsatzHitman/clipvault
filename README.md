# ClipVault

Cross-platform Rust clipboard history with full-text and cloud multimodal semantic search. Local SQLite metadata/indexes, lossless PNG image files, managed file copies, and a native GPU-rendered desktop UI. No telemetry and no local model downloads.

**Status:** implementation under verification. A CI workflow (`.github/workflows/ci.yml`) builds and tests Linux, Windows, and macOS, but it has not run on GitHub yet; results are pending the first run. A green build is not a claim that every desktop/compositor, permission flow, or provider model has been manually tested. See [platform limits](#platform-limits).

## Install and run

Once CI has run, download your OS artifact from the [CI runs](https://github.com/ErsatzHitman/clipvault/actions); none exist until then. Linux artifacts include a `.deb` and portable tarball. macOS has an ad-hoc-signed `.dmg`; Windows has a per-user installer and portable zip. These development artifacts are not notarized or publisher-signed.

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

For Fireworks, set `FIREWORKS_API_KEY`, select Fireworks and **Vision caption + text** mode, and enter text-embedding and vision model IDs available to your account. Fireworks endpoint support differs by model. Caption mode embeds the generated image description; it does not compute a native image embedding. Changing provider/model/mode schedules reindexing; the pipeline namespace includes the vision model, so changing it currently reindexes text items too. Retrieval never compares vectors from different configurations.

No API call is made before cloud processing is enabled. Turning it off prevents new calls; an already submitted request cannot be recalled. Semantic queries, text, extracted text-file/PDF/Office contents, and image thumbnails go to the chosen provider. Provider retention policies apply. Secrets copied from allowed apps can reach the cloud.

## Use

- Copy text, images, or regular files. History persists across restarts.
- `Ctrl+Shift+Space` opens history as a borderless, always-on-top strip docked to the bottom of the work area (about 28% of its height, clamped to 260-420 points) on the monitor under the pointer. To change the shortcut, open Settings → General → Change and press the new combination, then Save. It takes effect immediately; if another app already owns it, the old shortcut stays active. If the shortcut cannot be registered, the app falls back to a normal 920x620 window.
- Items are cards in a horizontal strip (mouse wheel scrolls sideways): text snippets, image thumbnails, and file cards with an extension badge, name, and size. Footers show age and size; a red dot appears only if indexing failed, and pinned cards show a star. Pin/delete buttons appear on hover. The All/Text/Images/Files segmented control filters the strip. Status messages show briefly next to the gear, then shrink to a dot that keeps the message on hover.
- Search is a round button that springs open into a field. Click it, press Ctrl/Cmd+F, or just start typing. While it is open, a mode chip next to it picks Keyword, Hybrid (the default: keywords first, then semantic ranking), or Semantic, which searches only indexed items after the provider responds. The chip's menu says whether cloud is off. Esc clears the query, then closes search, then hides the strip. Item counts, Settings, Pause, and Quit are in the gear menu.
- Left/Right (or Up/Down) selects a card; Home/End jump to the ends when the query is empty. Enter or double-click restores it and pastes if enabled. Right-click offers Copy/Paste/Pin/Delete; Ctrl+P pins; Delete removes when the query is empty. Escape clears the query, then hides. Losing focus also hides the strip; capture continues. The gear menu has Settings, Pause/Resume, and **Quit**.
- Copy restores without hiding. Paste returns to the last observed app. Failed focus verification leaves the content copied for manual paste and reopens history with the error.
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

Only regular files are managed; directories and symlinks are rejected. A bundle is limited to 64 files and the item-size cap. Names and common UTF-8/UTF-16 text/code contents are searchable. Text from PDF, docx, pptx, and xlsx files is extracted in the background (files over 50 MB skipped) and becomes searchable by keyword and semantic search once extraction finishes; a failed extraction falls back to name-only indexing. Scanned or encrypted PDFs yield no text (no OCR). Other binaries and archives are preserved but not parsed. File restore uses managed copies; the app does not retain original filesystem permissions. Clipboard history is polling-based and can miss rapid intermediate changes. Rich HTML/RTF is represented by plain text; audio and arbitrary custom formats are not supported.

## Storage and security

Default data location follows the OS application-data convention (`directories::ProjectDirs`). The exact path is displayed in Settings. Use `clipvault --data-dir /path` for a separate profile. One instance per data directory is enforced with a file lock.

- `history.sqlite3` and WAL/SHM: metadata, text, FTS5, normalized float32 vectors.
- `blobs/<sha256>/`: PNG images, file manifests, managed file copies. No image bytes in the search index.
- `settings.toml`: configuration, never API keys.

History is **not encrypted**. Use OS disk encryption. Root/admin, same-user processes, backups, swap, or malware can read it. Unix permissions are 0700/0600. Clearing uses SQLite secure deletion, checkpointing, vacuum, and blob cleanup, but cannot guarantee forensic erasure on SSDs or backups. Per-app exclusions observe the foreground app, not an authenticated clipboard owner; they reduce accidental collection and are not a security boundary. The arboard backend does not expose password-manager concealment markers; use exclusions and Pause. Do not rely on marker-based secret filtering.

Extracted document text is capped at 200,000 characters. Retention runs on startup and every minute. Pins can prevent size limits from being met. The count and size limits apply to unpinned items; pins are outside those budgets. The size limit covers stored payloads, not SQLite/index overhead. Lost/unreferenced blob directories are collected. Text extraction is bounded to 100,000 characters per supported text file; cloud text inputs are capped at 12,000 characters per item/query. Long items remain fully available to keyword search, while semantic coverage uses the bounded prefix. Native image inputs are thumbnailed to 1280 pixels. OS clipboard backends may decode a full image before the application can enforce the item-size cap. Capture and search queues are bounded; an overloaded capture queue retries the current observation rather than blocking the UI.

## Build

Rust 1.88 or later. Dependencies are locked. Linux development packages (Debian/Ubuntu):

```sh
sudo apt-get install libx11-dev libxtst-dev libxdo-dev libxcb1-dev libxrandr-dev libxi-dev libgl1-mesa-dev libwayland-dev libxkbcommon-dev
cargo build --locked --release
./target/release/clipvault
```

`cargo fmt --check` is lightweight. Full `cargo test --locked --all-targets`, Clippy, release builds, clipboard round trips, and packaging are configured to run on all three OSes in GitHub Actions (format check on Linux only). Clippy (`-D warnings`) and tests pass locally on Linux; the Windows and macOS code paths (including the work-area lookup used to dock the strip) have not been compiled yet. The database schema is now v2, so older ClipVault builds cannot open it. Do not run full suites on the project owner's laptop. Provider calls require an external key and are not made by public CI. `--clipboard-self-test` deliberately replaces the clipboard and is intended for disposable CI desktops only.

See [architecture](docs/ARCHITECTURE.md) for threading, vector-index trade-offs, and threat model. See [verification](docs/VERIFICATION.md) for what has actually been tested.

## References

- [eframe 0.33.3](https://docs.rs/eframe/0.33.3/eframe/)
- [global-hotkey 0.7](https://docs.rs/global-hotkey/0.7.0/global_hotkey/)
- [enigo 0.6](https://docs.rs/enigo/0.6.1/enigo/)
- [OpenRouter embedding API](https://openrouter.ai/docs/api/reference/embeddings)
- [Fireworks embeddings](https://docs.fireworks.ai/api-reference/embeddings)

MIT licensed.
