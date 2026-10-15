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

## Security configuration notes

- Tauri CSP is set (not `null`) in `src-tauri/tauri.conf.json`.
- Capabilities are kept minimal in `src-tauri/capabilities/default.json`.
- No secrets, tokens, or local databases are committed (see `.gitignore`).
