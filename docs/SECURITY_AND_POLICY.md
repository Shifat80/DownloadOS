# Security and Supported-Use Boundaries

## 1. Supported product purpose
DownloadOS is intended to download files and media the user is authorized to save, using supported technical methods. The product must not promise that all websites or private content are downloadable.

## 2. Authentication
- Never ask for or store a user's website password.
- Prefer official APIs, supported export features, or a narrowly scoped user-authorized browser flow.
- Do not export or upload a complete cookie jar.
- Avoid persisting session tokens. If a site-supported flow requires short-lived authorization data, keep it in memory where feasible, minimize scope/lifetime, and redact it from logs.
- Treat 401/403 responses as an authentication/authorization state, not as an invitation to bypass restrictions.
- A user being logged in does not itself grant permission to bypass technical protections.

## 3. Media and platform limits
- Do not bypass DRM, paywalls, access controls, or private-content protections.
- Do not evade rate limits or platform enforcement.
- Use official APIs and platform-supported download/export methods where available.
- An adapter must report unsupported cases honestly.
- Review the relevant platform terms and applicable law before shipping each integration.

## 4. URL and SSRF defense
- Accept HTTP/HTTPS only by default.
- Reject embedded credentials unless explicitly justified and safely handled.
- Resolve and validate destination addresses; reject loopback, private, link-local, multicast, and metadata-service targets as appropriate.
- Revalidate every redirect destination.
- Consider DNS rebinding, IPv4/IPv6 representations, alternate encodings, and redirect chains.
- Set timeouts, response-size limits where appropriate, and redirect limits.
- Never let a webpage trigger arbitrary local file access or arbitrary local process execution.

## 5. Filesystem
- Sanitize server-provided filenames.
- Prevent path traversal and destination escape.
- Require user approval for destination changes.
- Never silently overwrite an existing file.
- Use temporary files and safe finalization.
- Handle disk-full and permission errors.
- Do not execute downloads automatically.

## 6. Browser bridge
- Prefer browser native messaging where practical.
- If using loopback HTTP/WebSocket, bind to loopback only and use an explicit pairing process, scoped short-lived tokens, Origin/schema checks, rate limits, and size limits.
- Reject malformed requests and untrusted origins.
- Do not expose an unauthenticated local server to the LAN.
- Do not permit arbitrary bridge commands.
- Keep bridge logs free of secrets.

## 7. Tauri and supply chain
- Use minimal capabilities.
- Apply a restrictive CSP.
- Avoid broad filesystem or shell permissions.
- Pin and audit dependencies where practical.
- Keep signing keys and secrets out of the repository.
- Review dependency licenses before distribution.

## 8. Privacy defaults
- No mandatory account or telemetry for the local MVP.
- Do not upload download URLs or file lists.
- Explain any optional diagnostics and obtain informed consent.
- Provide a way to clear task history.
- Redact query tokens, Authorization headers, cookies, and credentials from logs.

## 9. Release gate
Before a public release, perform a threat-model review, dependency review, URL/redirect tests, path-traversal tests, bridge-origin tests, secret-redaction tests, and a review of each media adapter's permitted use.
