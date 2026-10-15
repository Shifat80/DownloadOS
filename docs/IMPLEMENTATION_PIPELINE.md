# DownloadOS Implementation Pipeline

This document is the staged execution plan Claude Code must follow. A milestone is complete only when its acceptance criteria and tests pass.

## Global workflow

For each milestone:
1. Inspect the repository and relevant docs.
2. State the milestone goal and files likely to change.
3. Implement the smallest coherent vertical slice.
4. Add automated tests alongside the feature.
5. Run formatter, lint/type checks, build, and relevant tests.
6. Fix failures; do not move on with unexplained failures.
7. Update docs and provide a concise completion report.
8. Wait for user approval before a major architecture change.

## Phase 0 — Repository and toolchain audit

Tasks:
- Inspect the generated Tauri project.
- Confirm Node/npm, Rust/Cargo, Tauri prerequisites, and Git.
- Run the starter app.
- Review `CLAUDE.md` and system design.
- Establish scripts for formatting, linting, type checking, tests, and dev launch.
- Create an architecture decision log if needed.

Acceptance:
- Starter app runs locally.
- Baseline commands are documented.
- No unrelated scaffolding or secrets are committed.

## Phase 1 — Direct HTTP/HTTPS download vertical slice

Tasks:
- Define task and error types.
- Add URL and destination validation.
- Implement one-stream HTTP download to a temporary file.
- Stream bytes without buffering the entire file in memory.
- Emit actual byte progress.
- Handle HTTP errors, cancellation, disk errors, and destination collisions.
- Finalize safely after successful completion.
- Add local integration test server fixtures.

Acceptance:
- A test file downloads with exact expected bytes.
- Invalid schemes are rejected.
- Existing destination is not silently overwritten.
- Cancellation stops work cleanly.
- Errors are structured and secrets are not logged.
- Tests pass.

## Phase 2 — Task lifecycle and persistence

Tasks:
- Add SQLite schema and migrations.
- Implement queued/downloading/paused/failed/completed states.
- Add task listing, details, retry, cancel, and remove-history operations.
- Persist task state and recover interrupted tasks at startup.
- Ensure completed tasks are not incorrectly requeued.

Acceptance:
- Tasks survive app restart.
- Interrupted tasks recover to a safe state.
- State transitions are validated.
- Database migrations are tested.

## Phase 3 — Pause/resume and segmented transfers

Tasks:
- Implement cooperative pause and resume.
- Verify range support using real responses.
- Add bounded segmented transfer with strict Content-Range validation.
- Validate segment offsets, lengths, and total size.
- Add ETag/Last-Modified resource consistency checks.
- Fall back to single-stream mode when range behavior is unsupported.
- Implement bounded retries/backoff.

Acceptance:
- Range-capable test server produces byte-identical output.
- Non-range server uses single-stream fallback.
- Malformed ranges fail safely.
- A changed resource is not silently merged with old bytes.
- Retries terminate on permanent errors.
- Pause/resume and restart recovery tests pass.

## Phase 4 — Desktop UI

Tasks:
- Build the download dashboard.
- Add URL entry and destination confirmation.
- Display actual progress, speed, and ETA when computable.
- Add active, queued, paused, completed, and failed views.
- Add pause/resume/cancel/retry actions.
- Add settings for default directory and connection limits.
- Add accessible loading, empty, and error states.
- Wire UI to Tauri commands/events; remove mock production data.

Acceptance:
- Every displayed task maps to a real backend task.
- UI remains responsive during large downloads.
- Errors are visible and actionable.
- No fake progress is used in production paths.
- Frontend checks and backend tests pass.

## Phase 5 — Browser handoff

Tasks:
- Build Chromium extension with minimum permissions.
- Add an explicit “Send to DownloadOS” action for ordinary links.
- Choose native messaging if it fits the implementation; otherwise document and harden a loopback bridge.
- Validate schemas, message size, pairing, caller identity/origin where applicable, and rate limits.
- Show app-not-installed and app-unavailable states.
- Add tests for malformed requests and hostile URLs.

Acceptance:
- A normal download link can be sent to the desktop app.
- Untrusted messages are rejected.
- No password/cookie dump exists.
- The extension does not silently upload browsing data.
- Bridge security review is documented.

## Phase 6 — Media adapters, one at a time

Tasks:
- Select one website/content type with a permitted and maintainable path.
- Document method, permissions, prerequisites, and known limitations before coding.
- Prefer official APIs and platform-supported download/export methods.
- Where permitted, use a user-authorized browser flow with minimal data access.
- Keep adapter code separate from the core engine.
- Return explicit unsupported/policy-restricted errors when necessary.
- Add fixture-based tests and manual test instructions.

Acceptance:
- The adapter supports a clearly documented subset.
- Unsupported/protected cases fail safely.
- No DRM/access-control bypass is introduced.
- No broad cookie extraction or credential collection exists.
- Website changes do not destabilize the core engine.

Do not add all social sites in one phase. Prioritize a single integration and verify it before expanding.

## Phase 7 — Hardening and Windows MVP release

Tasks:
- Review file-path handling, URL validation, redirect validation, and SSRF controls.
- Review Tauri capabilities and CSP.
- Test large files, slow connections, dropped connections, disk-full behavior, and restart recovery.
- Add privacy/security documentation.
- Package Windows build and test on a clean Windows environment.
- Document signing status and installation warnings honestly.
- Add CI for tests/build where practical.

Acceptance:
- All required tests pass.
- No known critical security issue remains open.
- Release notes list supported features and limitations.
- Installation and uninstallation are tested.
- Logs do not contain credentials or signed URL secrets.

## Phase 8 — macOS release readiness

Tasks:
- Build and test on macOS or a macOS CI runner.
- Validate permissions, paths, lifecycle, and app packaging.
- Test browser extension handoff on supported browsers.
- Plan Developer ID signing/notarization for public distribution.
- Document differences from Windows.

Acceptance:
- A real macOS build is tested.
- Release artifacts are validated.
- Any unsigned/notarization limitations are disclosed.

## Optional Phase 9 — Product services

Only after the local product is stable:
- Website and documentation
- Release/update hosting
- License verification
- Optional paid Pro features
- Telemetry only with clear user consent

Do not introduce a cloud backend merely to download files.
