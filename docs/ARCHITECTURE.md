# ClipVault architecture

ClipVault is a Rust desktop application. History, immutable image/file blobs, FTS5 tables, and normalized float32 vectors live in the user's application-data directory. There is no web server, analytics SDK, update beacon, or local model runtime.

## Data flow

Clipboard polling → bounded capture queue → storage worker → SQLite WAL + content-addressed blobs.

UI search → latest-query queue → search worker → FTS5 immediately → cloud query embedding → local cosine retrieval → reciprocal-rank fusion.

Pending metadata → index worker → cloud text embedding or native multimodal image embedding (optional vision description + text embedding) → local vector table. Provider identity and both model names namespace vectors. Changing models invalidates old vectors. Images are never stored in the search index. A cloud timeout affects semantic results, never capture or keyword results.

All disk operations, clipboard reads, decoding, and HTTP calls run outside the UI thread. Worker queues are bounded. The UI drops stale search responses by generation. Capture skips oversized clipboard items rather than exhausting memory. SQLite is the source of truth; FTS triggers and foreign-key vector deletion keep derived indexes consistent.

## Cloud disclosure

Cloud is required for semantic search. It must be enabled explicitly in settings. Text snippets, file names and supported text-file contents, image thumbnails, and semantic queries leave the device. OpenRouter defaults to direct multimodal embeddings through google/gemini-embedding-2. Fireworks and text-only models can use an explicitly selected vision-description-plus-text mode. This two-stage method is caption-mediated image retrieval, not a CLIP/shared-space image embedding. Provider retention and training policies apply; “no telemetry” does not mean providers cannot retain API input. API keys are read from environment variables and never written into settings or history.

## Threat model

Protect against accidental collection in excluded apps, unbounded history growth, path traversal in stored file names, unsafe clipboard formats, accidental cloud activation, provider error-body disclosure, and UI hangs. Per-app matching is a best-effort foreground observation, not a trusted clipboard-owner attestation. Unknown applications are excluded by default. Polling can miss very short-lived clipboard changes.

The database is not encrypted. Other processes running as the user, root/administrators, backups, swap, crash dumps, and malware can read history. Use OS disk encryption and exclude the data directory from backups if required. Unix directories/files are restricted to 0700/0600; Windows inherits the user's application-data ACL. SQLite secure deletion and checkpointing reduce recoverable data but cannot promise forensic erasure on SSDs or backups. Pinning can exceed retention limits and is visible in the UI.

## File support

Regular files are copied into managed local storage so history survives source deletion. Directories and symbolic links are rejected. File names are searchable. UTF-8 text/code files have bounded content extraction; binary documents are preserved without text extraction. Restoring files places managed copies on the clipboard. File bundles have a size cap and maximum file count. Images are lossless PNG files.

## Vector trade-off

An exact cosine vector index is stored in SQLite as normalized float32 blobs keyed by entry and model. It scans only matching-model dimensions on a background thread. At the default 10,000-item history limit this favors reliability, transactional deletion, and low dependency cost over approximate nearest-neighbor complexity. It is O(history × dimensions); larger histories may need an ANN index. Raw images never enter SQLite or FTS.
