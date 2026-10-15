# DownloadOS — Development Setup & Baseline Commands

This file documents the toolchain and the baseline commands established in **Phase 0**
of `IMPLEMENTATION_PIPELINE.md`.

## Toolchain (verified on Windows 11)

| Tool  | Version used | Purpose |
|-------|--------------|---------|
| Node  | v24.15.0     | React/Vite UI |
| npm   | 11.12.1      | JS package manager |
| Rust  | 1.99.0       | Core engine + Tauri shell |
| cargo | 1.99.0       | Rust build/test |
| git   | 2.54.0       | Version control |

WebView2 runtime ships with Windows 11. MSVC build tools come via the default
`rustup` MSVC toolchain.

## Project layout

```
src/            React + TypeScript UI (Vite)
src-tauri/      Rust: Tauri shell + (future) download_engine, task_store, security
docs/           Engineering docs
```

## Baseline commands

Run from the repository root.

### Frontend (JS/TS)

| Command | What it does |
|---------|--------------|
| `npm install`        | Install JS dependencies |
| `npm run dev`        | Vite dev server only (UI in a browser) |
| `npm run build`      | Type-check (`tsc`) + production UI build |
| `npm run typecheck`  | Type-check only, no emit |
| `npm run format`     | Format with Prettier |
| `npm run format:check` | Verify formatting (CI-friendly) |

### Desktop app (Tauri)

| Command | What it does |
|---------|--------------|
| `npm run tauri dev`   | Launch the desktop app with hot reload |
| `npm run tauri build` | Produce a packaged Windows build |

### Rust core (`src-tauri/`)

| Command | What it does |
|---------|--------------|
| `cargo build`            | Compile the Rust backend |
| `cargo test`             | Run Rust unit/integration tests |
| `cargo fmt`              | Format Rust code |
| `cargo fmt --check`      | Verify Rust formatting (CI-friendly) |
| `cargo clippy -- -D warnings` | Lint Rust, fail on warnings |

## Pre-milestone check (run all before marking a milestone complete)

```
npm run format:check
npm run typecheck
cd src-tauri && cargo fmt --check && cargo clippy -- -D warnings && cargo test
```

## Known environment constraints (this Windows machine)

Two machine-specific issues were hit during setup and worked around:

1. **OneDrive read-only folder attribute.** The repo lives under a
   OneDrive-synced path, which sets the read-only attribute on directories.
   `autocfg`-based build scripts then fail with *"output path is not a
   writable directory."* Fixed by redirecting Cargo's target directory
   outside OneDrive via `src-tauri/.cargo/config.toml` (git-ignored, since the
   absolute path is machine-specific):

   ```toml
   [build]
   target-dir = "C:/Users/<you>/AppData/Local/downloados/target"
   ```

2. **Smart App Control (SAC) is ENFORCED.** SAC blocks execution of unsigned /
   unknown-reputation binaries. Code **compiles** fine, but freshly built
   **binaries cannot run** — `cargo test` cannot launch its test executable
   (os error 4551) and `npm run tauri dev` cannot launch the app. There is no
   code-side workaround; running the app or the test suite locally requires
   turning SAC off in Windows Security (irreversible without an OS reset) or
   using a machine without SAC enforced. Until then, test and run status is
   reported as "compiles; execution blocked by SAC."

### Build/run must use the MSVC environment

Run `cargo` from PowerShell (not Git Bash). rustc's MSVC toolchain locates
`link.exe` and the Windows SDK via the registry once VS Build Tools (with the
"Desktop development with C++" workload, including the Windows SDK) are
installed. Git Bash's coreutils `link` shadows the MSVC linker, so avoid it for
builds.

## Data storage (Phase 2)

Task history is persisted in a local SQLite database at
`<app_data_dir>/tasks.db` (created on first run; schema versioned via
`PRAGMA user_version`). On startup the app recovers tasks that were in flight
during an unclean shutdown back to the `queued` state; completed, failed,
cancelled, and paused tasks are left as-is. The `source_url` is stored to
enable resume and is treated as a secret (never logged); `display_url` holds
the redacted form.

Tauri commands exposed: `list_downloads`, `get_download_details`,
`retry_download`, `cancel_download`, `remove_download`. Wiring a live transfer
through the store (progress + state updates during a running download) is
Phase 4 work.

## Transfer strategies (Phase 3)

The engine verifies byte-range support from a real ranged probe (never from
`Accept-Ranges` alone):

- `range.rs` — `Content-Range` parsing, resource validators (ETag/Last-Modified),
  `probe()`, and segment planning.
- `segmented.rs` — bounded parallel-range download: each segment's
  `Content-Range` is validated against the request, validators are re-checked
  (a resource that changes mid-download yields `RESOURCE_CHANGED`, never a
  merged file), and bytes are written at the correct offset of a preallocated
  temp file, then finalized atomically.
- `orchestrator.rs` — `download_resource()` probes, uses segmented transfer
  when ranges are verified and the resource is large enough, otherwise falls
  back to a single stream; transient (network) failures retry with bounded
  exponential backoff while permanent failures stop immediately.
- `single_stream.rs::download_resumable` — resumes from a `.part` file via a
  `Range` + `If-Range` request (a `200` answer discards the partial and
  restarts, so old/new bytes are never merged); cooperative pause preserves the
  partial, cancel removes it.

Segmented per-segment checkpointing (resuming a partially-done segmented
transfer) and status-aware retry of 429/503 are deferred to Phase 7.

## Desktop UI and download manager (Phase 4)

The React UI (`src/`) is wired to real backend commands — no mock data or
synthetic progress:

- `manager/` — owns control tokens for running transfers, runs each download on
  a background task via the resumable single-stream engine, persists real byte
  progress and lifecycle state, and emits events: `download:progress`
  (throttled `{id, downloaded, total}`) and `downloads:changed` (refresh
  signal). Speed/ETA are derived in the UI from real progress deltas.
- Tauri commands: `add_download`, `pause_download`, `resume_download`,
  `cancel_download`, `retry_download`, `remove_download`, `list_downloads`,
  `get_download_details`, `default_download_dir`.
- UI: dashboard with URL entry + optional filename, a destination folder shown
  in the add bar and editable in Settings (persisted per-viewer in
  localStorage), filter tabs (All/Active/Paused/Completed/Failed with counts),
  per-task cards with a real progress bar, size, speed, ETA, and
  pause/resume/cancel/retry/remove actions, plus accessible empty and error
  states. Light/dark via `prefers-color-scheme`.

The live manager now uses **multi-connection segmented** downloads when the
server verifiably supports byte ranges and the file is large enough (with a
configurable connection count, default 4, clamped 1–16); otherwise it falls
back to a single stream. Pause/resume/cancel/retry work for both: segmented
downloads persist per-segment checkpoints (schema v2 `segments` table) and
resume by re-fetching only incomplete segments. Settings (download folder and
connection count) are persisted in the store (`settings` table) and surfaced
via `get_settings`/`set_settings`.

The browser bridge (Phase 5) is wired into startup: it binds `127.0.0.1:14900`,
uses a persisted pairing token, and `bridge_info` exposes the port + code for
**Settings → Browser pairing**. A valid handoff creates and starts a download
just like the UI. Remaining follow-ups: native folder picker, auto-starting
tasks recovered to `queued` after a crash, and speeding up the
truncated-response test (currently gated by the read timeout).

## Security configuration notes

- Tauri CSP is set (not `null`) in `src-tauri/tauri.conf.json`.
- Capabilities are kept minimal in `src-tauri/capabilities/default.json`.
- No secrets, tokens, or local databases are committed (see `.gitignore`).
