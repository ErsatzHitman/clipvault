# Verification record

This file records actual results, not intended coverage.

## Local

- Empty workspace confirmed before implementation.
- Rust 1.99.1 and Cargo 1.99.1 available.
- Cargo.lock resolved successfully without compiling.
- Tiny SQLite FTS5 contract check passed for nine hostile/punctuation/Unicode queries using the same escaping algorithm as Rust storage.
- `python3 scripts/storage_sql_smoke.py` passed against the actual embedded schema: deduplication, FTS insert/delete triggers, vector cascade, non-reused IDs, and retry SQL.
- `cargo fmt --check`, `cargo metadata --locked --no-deps`, `git diff --check`, and Python syntax compilation passed. These do not replace a Rust compilation or test run.
- No full suite or release build run on the laptop.

## CI

Blocked before the first matrix run. The current GitHub OAuth token lacks `workflow` scope; GitHub rejected the workflow push. Read-only preflight confirmed the scope issue. SSH authentication also failed (`Permission denied (publickey)`). Run `gh auth refresh -h github.com -s workflow` in the user's terminal, then push the pending workflow commit.

The workflow includes format, Clippy, all-target tests, release builds, clipboard text/image/file round trips, and native packaging on Ubuntu, Windows, and macOS.

## Independent static review

A read-only DeepSeek review verified the pinned clipboard API calls and the separate keyword/cloud workers. Findings fixed: empty X11 application identity failing open, startup model invalidation, missing/changed-dimension semantic-index status, bounded focus retry, restore errors reopening history, pinned items excluded from unpinned retention budgets, Clippy default-initialization patterns, and unnecessary literal scans when FTS already found matches. Windows storage features and formatting had already been fixed. The reported punctuation-only FTS issue did not reproduce in the focused SQLite check; Rust contract tests remain pending CI.

The strict resume baseline remains intentional: it can skip a legitimate copy near an exclusion transition, favoring privacy over capturing a stale value from an excluded app. Full image decoding/hashing per poll and model-namespace over-invalidation remain documented trade-offs.

## External/live checks

- OpenRouter multimodal request shape researched against provider documentation.
- A synthetic OpenRouter contract request (generated red square + public test text) was attempted with the environment key. It returned HTTP 401. No clipboard content was used, no key or error-response body was printed. Live provider success remains unverified. Set a valid `OPENROUTER_API_KEY` in the launch environment and run `python3 scripts/cloud_smoke.py`.
- Physical desktop paste, app exclusions, global shortcuts, OS consent dialogs, and installation are not yet manually verified on all platforms.
