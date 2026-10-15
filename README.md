# DownloadOS — Engineering Documentation

This documentation pack is the implementation source of truth for Claude Code. It defines the architecture, scope, pipeline, security boundaries, testing strategy, and milestone workflow for DownloadOS.

## Read in this order

1. [`SYSTEM_DESIGN.md`](docs/SYSTEM_DESIGN.md) — architecture and design decisions
2. [`IMPLEMENTATION_PIPELINE.md`](docs/IMPLEMENTATION_PIPELINE.md) — staged build plan
3. [`CLAUDE_CODE_WORKFLOW.md`](docs/CLAUDE_CODE_WORKFLOW.md) — how to work with Claude Code
4. [`SECURITY_AND_POLICY.md`](docs/SECURITY_AND_POLICY.md) — security and supported-use boundaries
5. [`TEST_PLAN.md`](docs/TEST_PLAN.md) — verification requirements
6. [`PRODUCT_REQUIREMENTS.md`](docs/PRODUCT_REQUIREMENTS.md) — MVP requirements and acceptance criteria
7. [`CLAUDE.md`](CLAUDE.md) — persistent repository instructions for Claude Code

## Product statement

DownloadOS is a local-first desktop download manager for Windows and macOS. It manages ordinary HTTP/HTTPS downloads, queues, retries, resumable transfers, and browser handoff. It may support selected media websites through isolated adapters when a technically supported and permitted method exists.

Do not promise compatibility with every website. Authentication does not imply permission to bypass DRM, access controls, platform restrictions, or private-content protections.

## Guiding principles

- Local-first: downloaded files and task history stay on the user's device by default.
- Direct transfer: files travel from the source server to the user's device, not through a DownloadOS server.
- Test-driven increments: every milestone must compile and pass relevant tests before moving forward.
- Secure by default: least privilege, no password collection, no cookie-dump workflow, and strict URL/IPC validation.
- Honest compatibility: unsupported websites and streams fail clearly rather than pretending to work.
