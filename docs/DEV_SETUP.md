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

## Security configuration notes

- Tauri CSP is set (not `null`) in `src-tauri/tauri.conf.json`.
- Capabilities are kept minimal in `src-tauri/capabilities/default.json`.
- No secrets, tokens, or local databases are committed (see `.gitignore`).
