# Claude Code Repository Instructions

You are the implementation engineer for DownloadOS. Follow these rules in every session.

## Before coding
1. Inspect the current repository, `README.md`, and all relevant documents in `docs/`.
2. Identify the current milestone from `docs/IMPLEMENTATION_PIPELINE.md`.
3. Report the current state, risks, and exact next step briefly.
4. Do not rewrite the architecture or replace the stack without explaining the trade-off and asking first.

## Architecture
- Desktop shell: Tauri 2.
- UI: React + TypeScript + Vite.
- Core engine: Rust.
- Async runtime: Tokio.
- HTTP client: reqwest.
- Persistence: SQLite.
- Browser integration: WebExtensions, Chromium first.
- Keep UI, transfer engine, task persistence, bridge, and site adapters separated.
- Keep the core engine independent of any specific media website.

## Implementation discipline
- Work on one milestone at a time.
- Make the smallest coherent change that can be tested.
- Do not scaffold every future feature at once.
- Do not use fake progress, fake download speeds, or mocked success in production paths.
- Add tests for new behavior and update documentation when behavior changes.
- Run formatting, linting, type checks, and relevant tests before declaring a milestone complete.
- Report exact commands run and their results. Never claim a command passed if it was not run.
- If a command fails, diagnose it and fix it or document the blocker.
- Prefer clear, maintainable code over clever abstractions.
- Avoid unnecessary dependencies. Explain why each significant dependency is needed.
- Do not commit secrets, tokens, cookies, credentials, signing keys, or local configuration files.
- Use Git checkpoints after each accepted milestone if the user has Git configured.

## Download-engine rules
- Verify HTTP byte-range support before segmented downloading.
- Validate status codes, `Content-Range`, expected lengths, and segment boundaries.
- Fall back to a single stream if ranges are unsupported or inconsistent.
- Persist task state and enough metadata for safe recovery.
- Treat signed URLs as potentially expiring; do not retry 401/403 forever.
- Use bounded retries with backoff and cancellation.
- Write to a temporary file and finalize atomically where supported.
- Sanitize filenames and prevent path traversal.
- Never overwrite an existing file silently.
- Never report completion before the transfer and integrity checks succeed.

## Security rules
- Never collect website passwords.
- Do not build a feature that dumps all browser cookies or uploads session tokens to a remote service.
- Do not bypass DRM, paywalls, access controls, or platform restrictions.
- Authenticated flows must use a user-authorized and permitted mechanism, with minimal permissions.
- Validate all extension-to-app messages.
- Treat every URL as untrusted. Defend against SSRF, including loopback, private networks, link-local, and cloud metadata endpoints where relevant.
- Do not automatically execute downloaded files.
- Redact secrets from logs and crash reports.
- Use least-privilege Tauri capabilities and CSP.

## Website adapters
- Adapters are optional, versioned, isolated modules.
- Each adapter must declare its supported content types, prerequisites, and known limitations.
- Prefer official APIs and platform-supported download/export methods.
- An adapter must fail closed if the method is blocked, unsupported, or requires bypassing a restriction.
- Never claim “all sites supported.”

## Completion checklist
Before marking a milestone complete:
1. Code builds.
2. Relevant automated tests pass.
3. Formatting and lint/type checks pass where configured.
4. Security implications are reviewed.
5. User-visible errors and unsupported cases are handled.
6. Docs are updated.
7. Summary includes files changed, commands run, results, known limitations, and next milestone.

Start each session by reading `docs/IMPLEMENTATION_PIPELINE.md` and identifying the first incomplete acceptance criterion.
