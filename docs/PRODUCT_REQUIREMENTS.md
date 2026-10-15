# DownloadOS Product Requirements

## Product vision
A reliable, local-first download manager that combines robust file transfer management with convenient browser handoff and carefully scoped media integrations.

## Primary users
- Users downloading files from direct links.
- Users who want pause/resume, queues, and download history.
- Users who prefer browser-to-desktop download handoff.
- Users who need selected supported media workflows, within site permissions and technical limits.

## MVP features
### Must have
- Direct HTTP/HTTPS downloads.
- Queue and task states.
- Actual progress and byte counts.
- Pause/resume/cancel/retry.
- Persistent task history.
- Safe filenames and destination selection.
- Error handling and logs with secrets redacted.
- Windows development build.
- Automated tests for core transfer behavior.

### Should have
- Segmented downloads when verified supported.
- Browser extension handoff for ordinary links.
- Search/filter task history.
- Download directory and connection-limit settings.
- Clear unsupported/authentication errors.

### Later
- macOS release validation and distribution.
- Selected site/media adapters.
- Optional update service.
- Optional licensing and paid Pro features.
- Optional telemetry with informed consent.

## User stories
1. As a user, I can paste a direct URL and choose where to save it.
2. As a user, I can see real progress, downloaded bytes, and transfer status.
3. As a user, I can pause and resume without restarting when the server and saved state allow it.
4. As a user, I can restart the app and recover interrupted tasks safely.
5. As a user, I can send a normal link from my browser to DownloadOS.
6. As a user, I get a clear message when authentication expires or a site is unsupported.
7. As a user, my passwords and browser session cookies are not uploaded to DownloadOS servers.

## Definition of done
A feature is done only when implemented, tested, integrated, documented, and reviewed for security. A screenshot or UI mock is not proof of functional behavior.
