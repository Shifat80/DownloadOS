# Claude Code Workflow

## 1. First session prompt

Paste this into Claude Code after opening the repository:

> Read `CLAUDE.md`, `README.md`, `docs/SYSTEM_DESIGN.md`, `docs/IMPLEMENTATION_PIPELINE.md`, `docs/SECURITY_AND_POLICY.md`, and `docs/TEST_PLAN.md`. Inspect the current repository and toolchain. Do not implement the whole application. Identify the first incomplete milestone, list the current files and baseline commands, run the existing checks where safe, and propose the smallest next implementation step. Follow the documented architecture and security rules.

## 2. Start a milestone

> Implement Phase [N] from `docs/IMPLEMENTATION_PIPELINE.md` only. First summarize the planned changes and tests. Inspect the existing code before editing. Keep changes minimal and maintainable. Add tests for new behavior. Run formatting, lint/type checks, build, and relevant tests. Fix failures before reporting completion. Do not claim tests passed unless you actually ran them. Update the docs if behavior or commands changed. Stop after this milestone and report remaining limitations.

## 3. Ask for a code review

> Review the current diff against `CLAUDE.md`, `docs/SYSTEM_DESIGN.md`, and `docs/SECURITY_AND_POLICY.md`. Do not change code immediately. Identify correctness bugs, race conditions, unsafe filesystem behavior, SSRF risks, secret leakage, missing tests, and architecture drift. Rank findings by severity and cite file/line locations. Then propose the smallest fix plan.

## 4. Ask for a focused test pass

> Run the test suite relevant to the latest changes. Add missing regression tests for each bug found. Do not weaken assertions just to make tests pass. Report exact commands and results. If a test needs network access, use a controlled local test server rather than relying on a third-party site.

## 5. Ask for a security review

> Perform a security review of URL validation, redirects, filesystem destinations, filename handling, extension-to-app messages, Tauri commands/capabilities, logging, and authenticated-download handling. Verify that no passwords are collected, no broad cookie dump is implemented, and no access-control or DRM bypass exists. Provide findings with severity and remediation. Do not claim the app is secure solely because the checklist exists.

## 6. Recover after context compaction or a new session

> Read `CLAUDE.md` and `docs/IMPLEMENTATION_PIPELINE.md`. Inspect Git status, recent diff, and the current tests. Determine which milestone is complete and the first incomplete acceptance criterion. Summarize the state and continue only with that criterion. Do not redo completed work unless you find a specific defect.

## 7. Before committing

> Review `git diff` and `git status`. Confirm no secrets, tokens, cookies, generated binaries, local databases, or unrelated files are included. Run the documented checks. Suggest a concise commit message. Do not commit unless I explicitly ask you to.

## 8. Rules for web-site adapters

> Before implementing this adapter, document the allowed access method, permissions, expected inputs/outputs, supported content types, and known failure modes. Prefer official APIs and supported export/download methods. Do not bypass DRM, paywalls, access controls, or platform restrictions. Do not export the user's full browser cookie jar. If no permitted and maintainable path is available, implement a clear `UNSUPPORTED_MEDIA` result and document the limitation.

## 9. Good prompting habits
- Ask for one phase, one bug, or one review at a time.
- Require a plan before broad refactors.
- Keep Git checkpoints.
- Ask Claude to show the actual test output summary.
- Do not paste real passwords, cookies, access tokens, or signing secrets into prompts.
- Use synthetic test data and local fixtures.
