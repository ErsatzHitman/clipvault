# Verification record

This file records actual results, not intended coverage.

## Local

- Empty workspace confirmed before implementation.
- Rust 1.99.1 and Cargo 1.99.1 available.
- Cargo.lock resolved successfully without compiling.
- Tiny SQLite FTS5 contract check passed for nine hostile/punctuation/Unicode queries using the same escaping algorithm as Rust storage.
- No full suite or release build run on the laptop.

## CI

Blocked before the first matrix run. The current GitHub OAuth token lacks `workflow` scope; GitHub rejected the workflow push. Read-only preflight confirmed the scope issue. SSH authentication also failed (`Permission denied (publickey)`). Run `gh auth refresh -h github.com -s workflow` in the user's terminal, then push the pending workflow commit.

The workflow includes format, Clippy, all-target tests, release builds, clipboard text/image/file round trips, and native packaging on Ubuntu, Windows, and macOS.

## External/live checks

- OpenRouter multimodal request shape researched against provider documentation.
- A synthetic OpenRouter contract request (generated red square + public test text) was attempted with the environment key. It returned HTTP 401. No clipboard content was used, no key or error-response body was printed. Live provider success remains unverified. Set a valid `OPENROUTER_API_KEY` in the launch environment and run `python3 scripts/cloud_smoke.py`.
- Physical desktop paste, app exclusions, global shortcuts, OS consent dialogs, and installation are not yet manually verified on all platforms.
