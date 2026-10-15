# Browser Bridge Security Review

This documents the security design of the browser-to-app handoff (Phase 5) and
satisfies the "bridge security review is documented" acceptance criterion.

## Mechanism chosen: hardened loopback bridge

Native messaging was considered (preferred where practical per
`docs/SECURITY_AND_POLICY.md` §6) but requires registering a native-host
manifest in the OS registry tied to a fixed extension ID, which is impractical
to install and test in this environment. We therefore use a **loopback HTTP
bridge**, hardened as the policy requires.

## Threat model

| Threat | Mitigation |
|--------|-----------|
| A malicious web page POSTs to the local bridge (CSRF to localhost) | `Origin` must start with `chrome-extension://` / `moz-extension://`; web origins and missing Origin are rejected (403). |
| Another local user / process calls the bridge | A per-install **pairing token** (`X-DownloadOS-Token`) is required; compared in constant time. An empty configured token never authorizes. |
| LAN exposure | The server binds `127.0.0.1` only — never `0.0.0.0`. |
| Resource exhaustion via large bodies | Declared `Content-Length > 8 KiB` is rejected (413), and the body read is capped regardless of the declared length. |
| Request flooding | Fixed-window rate limit (20 handoffs / 10 s), returning 429. |
| Arbitrary command execution | The bridge exposes only `GET /ping` and `POST /handoff`; there is no generic command surface. |
| Hostile URLs / SSRF | Only `http`/`https` URLs are accepted at the bridge; full SSRF resolution (loopback/private/link-local/metadata blocking, per-redirect re-validation) runs in the download engine before any bytes are fetched. |
| Malformed input | Non-JSON bodies and bad shapes are rejected (400). |
| Data exfiltration by the extension | The extension sends only the clicked link URL (and optional filename) to `127.0.0.1`. It has no host permissions beyond `http://127.0.0.1/*`, does not read page content, cookies, history, or credentials, and contacts no remote server. |

## What is NOT collected or sent

- No website passwords.
- No cookie jar or session tokens.
- No browsing history or page content.
- Nothing is sent to any remote server — only to the user's own loopback.

## Pairing

On first run the app generates a random pairing token, persisted locally under
the app data directory. The user copies the port and token from **Settings →
Browser pairing** into the extension's options. The token is the app's own
local pairing secret (not a website credential), so persisting it locally is
acceptable; a future enhancement can add token rotation / expiry.

## Residual items / follow-ups

- Token rotation and short expiry (currently a stable per-install token).
- Optional native-messaging transport as an alternative to loopback.
- These do not weaken the above controls; they are hardening refinements.

## Tests

`src-tauri/src/bridge/mod.rs` unit tests cover origin/token authorization
(including the empty-token case), rate limiting, and constant-time comparison.
`src-tauri/tests/bridge_handoff.rs` exercises the live server end to end: valid
handoff, bad/missing token, web origin, oversized body, malformed JSON, and
non-http URL rejection.
