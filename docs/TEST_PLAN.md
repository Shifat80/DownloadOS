# DownloadOS Test Plan

## 1. Test layers

### Unit tests
- URL scheme validation.
- Filename sanitization.
- State-machine transitions.
- Retry classification and backoff bounds.
- Segment-range arithmetic.
- Error-code mapping.
- Redaction of sensitive URLs and headers.

### Integration tests
Use a controlled local HTTP test server that can simulate:
- Normal 200 response.
- Valid 206 range responses.
- Server ignoring Range and returning 200.
- Missing or malformed Content-Range.
- Incorrect content length.
- Connection interruption.
- Slow streaming.
- 401/403/404/429/500 responses.
- Redirect chains.
- ETag changes between requests.
- Filename headers with unsafe characters.
- Disk write failure where testable.

### UI tests
- Empty state.
- Active download progress.
- Pause/resume/cancel.
- Failed and retry states.
- Existing destination warning.
- Invalid URL error.
- Accessible keyboard operation.
- No fake progress in real engine flows.

### Extension/bridge tests
- Valid handoff.
- Desktop app unavailable.
- Malformed JSON/message.
- Oversized payload.
- Untrusted origin.
- Non-HTTP(S) URL.
- Loopback/private-network target rejection.
- Replayed or expired pairing token.
- No cookies/passwords in messages or logs.

## 2. Core acceptance scenarios

1. Download a known test file and compare byte-for-byte with the fixture.
2. Pause and resume a large transfer.
3. Interrupt the process and verify safe recovery.
4. Download from a server without Range support and confirm fallback.
5. Return malformed range responses and confirm safe failure/fallback.
6. Change ETag between segment requests and confirm the output is not corrupted.
7. Simulate disk full or write permission failure and show an actionable error.
8. Attempt path traversal through a server filename and verify the destination remains inside the selected folder.
9. Attempt `file:`, `javascript:`, loopback, private-network, and metadata-service URLs; verify policy enforcement.
10. Verify signed query parameters and authorization headers never appear in logs.
11. Confirm completed status is emitted only after validation.
12. Verify cancellation and retry do not create unbounded tasks.

## 3. Performance checks
- Use repeatable local test fixtures.
- Compare single-stream and segmented modes under the same server/network conditions.
- Record throughput, CPU, memory, recovery time, and error rate.
- Do not claim a fixed speed multiplier without repeatable evidence.
- Respect source-server limits; do not use unauthorized stress testing.

## 4. Release checklist
- [ ] Formatting passes.
- [ ] Lint/type checks pass.
- [ ] Rust unit tests pass.
- [ ] Integration tests pass.
- [ ] UI tests pass where configured.
- [ ] Extension tests pass where configured.
- [ ] Secret redaction verified.
- [ ] SSRF and redirect protections tested.
- [ ] Windows clean-install test completed.
- [ ] Supported features and limitations documented.
- [ ] No credentials or signing keys committed.
- [ ] Release notes and privacy policy reviewed.
