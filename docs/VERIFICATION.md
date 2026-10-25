# Verification record

This file records actual results, not intended coverage.

## Local

- Empty workspace confirmed before implementation.
- Rust 1.99.1 and Cargo 1.99.1 available.
- Cargo.lock resolved successfully without compiling.
- Tiny SQLite FTS5 contract check passed for nine hostile/punctuation/Unicode queries using the same escaping algorithm as Rust storage.
- `python3 scripts/storage_sql_smoke.py` passed against the actual embedded schema: deduplication, FTS insert/delete triggers, vector cascade, non-reused IDs, and retry SQL.
- `cargo fmt --check`, `cargo metadata --locked --no-deps`, `git diff --check`, and Python syntax compilation passed. These do not replace a Rust compilation or test run.
- On a Linux dev machine: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings` (clean), and `cargo test --locked --all-targets` passed (21 unit tests, 7 and 1 in the integration targets). Tests cover the rank-fusion logic and cloud request helpers (image thumbnailing, data URLs, provider selection, embedding parsing); no network, clipboard, or env vars are used.
- `cargo build --locked --release`, `xvfb-run -a ./target/release/clipvault --clipboard-self-test` (text, image, file list), and `packaging/linux.sh` (.deb and tarball) succeeded on Linux under Xvfb.
- Double-clicking a history item now restores it like Enter (code change only; not manually exercised in a real desktop session).
- Windows and macOS code (`cfg` branches) has not been compiled or linted; Linux only.
- Not covered by tests: HTTP request building, error redaction and size caps in the cloud client, embedding queue retry/stale-result handling. These need small refactors first. `image_data_url` currently upscales small images to 1280px, which is pinned by a test but probably unintended.

## CI

Workflow added at `.github/workflows/ci.yml` (push, pull_request, workflow_dispatch), but **it has not run on GitHub yet**. No CI result exists; do not read it as passing. It is also uncommitted/unpushed, and pushing it needs a token with `workflow` scope (`gh auth refresh -h github.com -s workflow`).

The workflow runs on Ubuntu, Windows, and macOS: format check (Linux only), Clippy, all-target tests, release build, clipboard text/image/file round trip (Linux under Xvfb without a window manager), native packaging, and artifact upload. Windows/macOS packaging (Inno Setup, codesign, hdiutil) and the self-test on hosted runners are unverified; if the self-test is flaky there it may need `continue-on-error`.

## Independent static review

A read-only DeepSeek review verified the pinned clipboard API calls and the separate keyword/cloud workers. Findings fixed: empty X11 application identity failing open, startup model invalidation, missing/changed-dimension semantic-index status, bounded focus retry, restore errors reopening history, pinned items excluded from unpinned retention budgets, Clippy default-initialization patterns, and unnecessary literal scans when FTS already found matches. Windows storage features and formatting had already been fixed. The reported punctuation-only FTS issue did not reproduce in the focused SQLite check; Rust contract tests remain pending CI.

The strict resume baseline remains intentional: it can skip a legitimate copy near an exclusion transition, favoring privacy over capturing a stale value from an excluded app. Full image decoding/hashing per poll and model-namespace over-invalidation remain documented trade-offs.

## External/live checks

- OpenRouter multimodal request shape researched against provider documentation.
- A synthetic OpenRouter contract request (generated red square + public test text) was attempted with the environment key. It returned HTTP 401. No clipboard content was used, no key or error-response body was printed. Live provider success remains unverified. Set a valid `OPENROUTER_API_KEY` in the launch environment and run `python3 scripts/cloud_smoke.py`.
- Wayland behavior (data-control capture, source identification, shortcut, paste) is not tested.
- Physical desktop paste, app exclusions, global shortcuts, OS consent dialogs, and installation are not yet manually verified on all platforms.
