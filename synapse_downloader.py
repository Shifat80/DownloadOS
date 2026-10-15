#!/usr/bin/env python3
"""Download non-DRM media from a Synapse LMS lecture page you can access.

The script opens a real browser so the user can sign in, observes the media
request made by the site's player, and hands that authorized request to
yt-dlp.  It intentionally refuses Widevine, PlayReady, FairPlay, and other
DRM-protected streams.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
import queue
import re
import sys
import threading
import time
import types
import urllib.error
import urllib.request
from concurrent.futures import Future, ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from urllib.parse import urlparse


DEFAULT_URL = (
    "https://lms.synapsepg.org/videos/6948f22647005260db1aa242/lecture"
)
MEDIA_EXTENSIONS = (".m3u8", ".mpd", ".mp4", ".webm", ".m4a")
MEDIA_CONTENT_TYPES = (
    "application/vnd.apple.mpegurl",
    "application/x-mpegurl",
    "audio/mpegurl",
    "audio/x-mpegurl",
    "application/dash+xml",
    "video/mp4",
    "video/webm",
    "audio/mp4",
)
DRM_URL_MARKERS = (
    "license-global.pallycon.com",
    "licensemanager",
    "fpskeymanager",
    "/drm-auth",
)
DRM_TEXT_MARKERS = (
    "com.widevine.alpha",
    "com.microsoft.playready",
    "com.apple.fps",
    "urn:uuid:edef8ba9",
    "urn:uuid:9a04f079",
    "skd://",
    "method=sample-aes",
    "pallycon",
)
TERMINAL_LOCK = threading.Lock()


@dataclass(frozen=True)
class MediaRequest:
    url: str
    headers: dict[str, str]


class Capture:
    """Collect media requests without ever printing authentication secrets."""

    def __init__(self) -> None:
        self.items: queue.Queue[MediaRequest] = queue.Queue()
        self.drm_seen = threading.Event()
        self.hls_key_headers: dict[str, str] = {}
        self.hls_key_url: str | None = None
        self._seen_urls: set[str] = set()
        self._lock = threading.Lock()

    def clear(self) -> None:
        while True:
            try:
                self.items.get_nowait()
            except queue.Empty:
                break
        with self._lock:
            self._seen_urls.clear()
            self.hls_key_headers.clear()
            self.hls_key_url = None
        self.drm_seen.clear()

    def observe_url(self, url: str) -> None:
        lowered = url.lower()
        if any(marker in lowered for marker in DRM_URL_MARKERS):
            self.drm_seen.set()

    def observe_request(self, url: str, headers: dict[str, str]) -> None:
        """Remember the signed headers used by Vidinfra's AES key endpoint."""

        self.observe_url(url)
        if urlparse(url).hostname == "kms.vidinfra.com":
            filtered = filter_headers(headers)
            if any(name.lower().startswith("x-auth-") for name in filtered):
                with self._lock:
                    self.hls_key_url = url
                    self.hls_key_headers.update(filtered)

    def get_hls_key_request(self) -> tuple[str | None, dict[str, str]]:
        with self._lock:
            return self.hls_key_url, dict(self.hls_key_headers)

    def add(self, url: str, headers: dict[str, str]) -> None:
        self.observe_url(url)
        clean_url = url.split("#", 1)[0]
        path = urlparse(clean_url).path.lower()
        if not any(ext in path for ext in MEDIA_EXTENSIONS):
            return
        with self._lock:
            if clean_url in self._seen_urls:
                return
            self._seen_urls.add(clean_url)
        self.items.put(MediaRequest(clean_url, filter_headers(headers)))


def filter_headers(headers: dict[str, str]) -> dict[str, str]:
    """Keep only headers normally needed to replay an authorized media request."""

    allowed = {
        "authorization",
        "cookie",
        "origin",
        "referer",
        "user-agent",
        "accept",
        "accept-language",
        "x-auth-nonce",
        "x-auth-signature",
        "x-auth-timestamp",
    }
    return {str(k): str(v) for k, v in headers.items() if k.lower() in allowed}


def safe_name(value: str) -> str:
    value = re.sub(r'[<>:"/\\|?*\x00-\x1f]', "_", value).strip(" .")
    value = re.sub(r"\s+", " ", value)
    return value[:160] or "lecture"


def format_bytes(value: float | int | None) -> str:
    if value is None:
        return "?"
    size = float(value)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if abs(size) < 1024 or unit == "TiB":
            return f"{size:.1f} {unit}"
        size /= 1024
    return f"{size:.1f} TiB"


class TerminalProgress:
    """Render a stable one-line yt-dlp progress indicator."""

    def __init__(self, label: str = "") -> None:
        self.label = label
        self.last_length = 0
        self.last_update = 0.0

    def _write(self, message: str, *, finish: bool = False) -> None:
        if self.label:
            with TERMINAL_LOCK:
                print(f"[{self.label}] {message}", flush=True)
            return
        padding = " " * max(0, self.last_length - len(message))
        with TERMINAL_LOCK:
            print(f"\r{message}{padding}", end="\n" if finish else "", flush=True)
        self.last_length = 0 if finish else len(message)

    def __call__(self, status: dict[str, Any]) -> None:
        state = status.get("status")
        if state == "downloading":
            now = time.monotonic()
            minimum_interval = 1.0 if self.label else 0.15
            if now - self.last_update < minimum_interval:
                return
            self.last_update = now

            downloaded = status.get("downloaded_bytes") or 0
            total = status.get("total_bytes") or status.get("total_bytes_estimate")
            fragment_index = status.get("fragment_index")
            fragment_count = status.get("fragment_count")

            if total:
                percentage = min(100.0, downloaded * 100.0 / total)
                amount = f"{format_bytes(downloaded)} / {format_bytes(total)}"
            elif fragment_index and fragment_count:
                percentage = min(100.0, fragment_index * 100.0 / fragment_count)
                amount = f"fragment {fragment_index}/{fragment_count}"
            else:
                percentage = None
                amount = format_bytes(downloaded)

            speed = status.get("speed")
            eta = status.get("eta")
            percent_text = f"{percentage:6.2f}%" if percentage is not None else "   ---%"
            speed_text = f"{format_bytes(speed)}/s" if speed else "?"
            eta_text = f"{int(eta) // 60:02d}:{int(eta) % 60:02d}" if eta is not None else "--:--"
            self._write(
                f"Download {percent_text} | {amount} | {speed_text} | ETA {eta_text}"
            )
        elif state == "finished":
            downloaded = status.get("downloaded_bytes") or status.get("total_bytes")
            self._write(
                f"Download 100.00% | {format_bytes(downloaded)} | complete",
                finish=True,
            )
        elif state == "error":
            self._write("Download failed", finish=True)


def default_profile_dir() -> Path:
    base = Path(os.environ.get("LOCALAPPDATA", Path.cwd()))
    # v3 starts clean with Chromium's Windows sandbox enabled. Older profiles
    # are preserved beside it in case the user needs to recover their sessions.
    return base / "SynapseDownloader" / "browser-profile-v3"


def read_synapse_storage_from_edge() -> dict[str, str]:
    """Read only Synapse local-storage records from the last-used Edge profile.

    The session token is returned in memory and is never printed or written by
    this function. Other origins in the LevelDB are ignored.
    """

    edge_root = Path(os.environ.get("LOCALAPPDATA", "")) / "Microsoft" / "Edge" / "User Data"
    if not edge_root.exists():
        return {}

    profile_name = "Default"
    try:
        local_state = json.loads((edge_root / "Local State").read_text(encoding="utf-8"))
        profile_name = local_state.get("profile", {}).get("last_used") or "Default"
    except (OSError, ValueError, TypeError):
        pass
    leveldb = edge_root / profile_name / "Local Storage" / "leveldb"
    if not leveldb.exists():
        return {}

    # ccl_chromium_reader imports an optional zstd module for cache parsing;
    # local-storage parsing does not use it. Provide a harmless import shim on
    # Python versions for which the optional native zstd wheel is unavailable.
    if "zstd" not in sys.modules:
        zstd_shim = types.ModuleType("zstd")
        zstd_shim.Error = Exception  # type: ignore[attr-defined]
        zstd_shim.decompress = lambda data: data  # type: ignore[attr-defined]
        sys.modules["zstd"] = zstd_shim

    try:
        from ccl_chromium_reader import ccl_chromium_localstorage
    except ImportError:
        return {}

    wanted = {
        "sessionId",
        "student",
        "selectedCourse",
        "selectedSubject",
        "allLectures",
        "allChapters",
        "allQuestionSolves",
        "allSubjects",
        "allExams",
    }
    newest: dict[str, tuple[int, str]] = {}
    # The forensic reader can report decode warnings for unrelated origins.
    # Suppress those while selecting only the Synapse origin below.
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        try:
            with ccl_chromium_localstorage.LocalStoreDb(leveldb) as local_storage:
                for storage_key in local_storage.iter_storage_keys():
                    if str(storage_key) != "https://lms.synapsepg.org":
                        continue
                    for record in local_storage.iter_records_for_storage_key(storage_key):
                        key = str(record.script_key)
                        value = record.value
                        if key not in wanted or not isinstance(value, str):
                            continue
                        sequence = int(record.leveldb_seq_number)
                        if key not in newest or sequence > newest[key][0]:
                            newest[key] = (sequence, value)
        except (OSError, ValueError, KeyError):
            return {}
    result = {key: value for key, (_, value) in newest.items()}
    return result if result.get("sessionId") else {}


def install_synapse_storage(context: Any, storage: dict[str, str]) -> None:
    """Install imported values before the LMS JavaScript reads localStorage."""

    payload = json.dumps(storage, ensure_ascii=True).replace("</", "<\\/")
    context.add_init_script(
        script=f"""
        (() => {{
            if (location.hostname !== 'lms.synapsepg.org') return;
            const values = {payload};
            for (const [key, value] of Object.entries(values)) {{
                localStorage.setItem(key, value);
            }}
        }})();
        """
    )


def import_dependencies() -> tuple[Any, Any]:
    try:
        from playwright.sync_api import Error as PlaywrightError
        from playwright.sync_api import sync_playwright
    except ImportError:
        print(
            "Missing Playwright. Run:\n"
            "  python -m pip install -r requirements.txt",
            file=sys.stderr,
        )
        raise SystemExit(2)

    try:
        import yt_dlp
    except ImportError:
        print(
            "Missing yt-dlp. Run:\n"
            "  python -m pip install -r requirements.txt",
            file=sys.stderr,
        )
        raise SystemExit(2)
    return (sync_playwright, (PlaywrightError, yt_dlp))


def manifest_has_drm(media: MediaRequest) -> tuple[bool, str]:
    """Inspect a small manifest and identify common commercial DRM markers."""

    path = urlparse(media.url).path.lower()
    if not (".m3u8" in path or ".mpd" in path):
        return (False, "")

    request = urllib.request.Request(media.url, headers=media.headers)
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            body = response.read(2_000_000).decode("utf-8", "replace").lower()
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        # The browser observation below is an independent DRM check. If this
        # fetch fails, yt-dlp can still try the same authorized request.
        return (False, f"Manifest inspection was unavailable: {exc}")

    for marker in DRM_TEXT_MARKERS:
        if marker in body:
            return (True, f"manifest contains DRM marker {marker!r}")

    # DASH ContentProtection can also use identifiers not listed above.
    if ".mpd" in path and "<contentprotection" in body:
        return (True, "DASH manifest contains ContentProtection")
    return (False, "")


def fetch_hls_key(url: str, headers: dict[str, str]) -> bytes:
    """Consume Vidinfra's one-time signed request and return the AES key."""

    request = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            key = response.read(64)
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        raise RuntimeError(f"The signed AES key request failed: {exc}") from exc
    if len(key) not in (16, 24, 32):
        raise RuntimeError(f"The AES key server returned an invalid {len(key)}-byte key.")
    return key


def download_media(
    media: MediaRequest,
    title: str,
    output_dir: Path,
    fragments: int,
    yt_dlp: Any,
    hls_key: bytes | None = None,
    progress_label: str = "",
) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    output_template = str(output_dir / f"{safe_name(title)}.%(ext)s")
    terminal_progress = TerminalProgress(progress_label)
    options: dict[str, Any] = {
        "outtmpl": output_template,
        "http_headers": media.headers,
        "continuedl": True,
        "concurrent_fragment_downloads": fragments,
        "fragment_retries": 20,
        "retries": 20,
        "file_access_retries": 10,
        # Prefer one already-muxed stream so ffmpeg is not required. Synapse's
        # ordinary HLS variants normally contain both video and audio.
        "format": "b",
        "merge_output_format": "mp4",
        "noplaylist": True,
        "overwrites": False,
        "part": True,
        "noprogress": True,
        "quiet": True,
        "progress_hooks": [terminal_progress],
        # The Vidinfra manifest is VOD, but yt-dlp's Generic extractor can
        # misclassify its CRLF media-sequence line and print a live-HLS warning.
        "no_warnings": True,
    }
    if hls_key is not None:
        # Supplying the already-authorized key prevents parallel workers from
        # racing to reuse Vidinfra's single-use signed nonce.
        options["extractor_args"] = {
            "generic": {"hls_key": [hls_key.hex()]},
        }
    with TERMINAL_LOCK:
        prefix = f"[{progress_label}] " if progress_label else ""
        print(f"\n{prefix}Downloading to: {output_dir.resolve()}")
        print(f"{prefix}Parallel fragment connections: {fragments}")
    with yt_dlp.YoutubeDL(options) as downloader:
        downloader.download([media.url])


def parse_video_selection(value: str, count: int) -> list[int]:
    """Parse selections such as ``1,3-5`` into zero-based card indices."""

    value = value.strip().lower()
    if value == "all":
        return list(range(count))

    selected: list[int] = []
    seen: set[int] = set()
    for part in value.split(","):
        part = part.strip()
        if not part:
            raise ValueError("empty selection")
        if "-" in part:
            pieces = part.split("-", 1)
            if not all(piece.strip().isdigit() for piece in pieces):
                raise ValueError(f"invalid range {part!r}")
            start, end = (int(piece.strip()) for piece in pieces)
            if start > end:
                raise ValueError(f"range {part!r} is backwards")
            numbers = range(start, end + 1)
        else:
            if not part.isdigit():
                raise ValueError(f"invalid number {part!r}")
            numbers = (int(part),)

        for number in numbers:
            if not 1 <= number <= count:
                raise ValueError(f"video {number} is outside 1-{count}")
            index = number - 1
            if index not in seen:
                seen.add(index)
                selected.append(index)
    if not selected:
        raise ValueError("no videos selected")
    return selected


def choose_videos(
    page: Any,
    requested_index: int | None,
    requested_videos: str | None,
    download_all: bool,
) -> list[tuple[int, str]]:
    cards = page.locator(".student-video-card")
    count = cards.count()
    if count == 0:
        raise RuntimeError(
            "No lecture cards were found. Make sure you are signed in, the "
            "lecture page has loaded, and your account can play this course."
        )

    labels: list[str] = []
    print("\nVideos on this page:")
    for index in range(count):
        text = " ".join(cards.nth(index).inner_text().split())
        label = text or f"Video {index + 1}"
        labels.append(label)
        print(f"  {index + 1}. {label}")

    if download_all:
        selected_indices = list(range(count))
    elif requested_videos is not None:
        try:
            selected_indices = parse_video_selection(requested_videos, count)
        except ValueError as exc:
            raise RuntimeError(f"Invalid --videos selection: {exc}") from exc
    elif requested_index is not None:
        if not 1 <= requested_index <= count:
            raise RuntimeError(f"--index must be between 1 and {count}.")
        selected_indices = [requested_index - 1]
    else:
        while True:
            raw = input(
                f"Choose videos [1-{count}] (examples: 1, 1,3-5, or all): "
            ).strip()
            try:
                selected_indices = parse_video_selection(raw, count)
            except ValueError:
                print("Enter one number, a comma/range list such as 1,3-5, or all.")
                continue
            break

    return [(index, labels[index]) for index in selected_indices]


def discover_media_in_frames(page: Any) -> MediaRequest | None:
    """Find media already loaded by an embedded cross-origin player.

    Some headed Chromium sessions do not deliver every iframe request through
    a page listener. Performance entries inside each frame provide an
    independent way to discover the same playlist.
    """

    try:
        user_agent = page.evaluate("navigator.userAgent")
    except Exception:
        user_agent = ""

    candidates: list[tuple[str, str]] = []
    for frame in page.frames:
        try:
            urls = frame.evaluate(
                """
                () => {
                    const resources = performance.getEntriesByType('resource')
                        .map(entry => entry.name);
                    const elements = [...document.querySelectorAll('video, audio, source')]
                        .flatMap(element => [element.currentSrc, element.src])
                        .filter(Boolean);
                    return [...resources, ...elements];
                }
                """
            )
        except Exception:
            continue
        for url in urls:
            if not isinstance(url, str) or url.startswith("blob:"):
                continue
            path = urlparse(url).path.lower()
            if any(ext in path for ext in MEDIA_EXTENSIONS):
                candidates.append((url, frame.url))

    if not candidates:
        return None

    # Prefer the quality-specific VOD playlist. It includes EXT-X-ENDLIST,
    # which prevents yt-dlp from misclassifying the master as a live stream.
    candidates.sort(
        key=lambda item: (
            0 if urlparse(item[0]).path.lower().endswith("/media.m3u8") else 1,
            0 if ".m3u8" in urlparse(item[0]).path.lower() else 1,
        )
    )
    url, referer = candidates[0]
    headers = {"Referer": referer or page.url}
    if user_agent:
        headers["User-Agent"] = user_agent
    return MediaRequest(url.split("#", 1)[0], headers)


def wait_for_media(page: Any, capture: Capture, timeout_seconds: int) -> MediaRequest:
    """Wait while continuing to pump Playwright's synchronous event loop.

    Blocking on Queue.get() here prevents sync Playwright from dispatching the
    request/response callbacks that populate the queue. Short Playwright waits
    keep browser events flowing while we poll for the captured media request.
    """

    deadline = time.monotonic() + timeout_seconds
    next_frame_scan = 0.0
    settle_deadline: float | None = None
    candidates: dict[str, MediaRequest] = {}
    while time.monotonic() < deadline:
        while True:
            try:
                item = capture.items.get_nowait()
                candidates[item.url] = item
                if settle_deadline is None:
                    # The master arrives first; the child VOD playlist follows.
                    settle_deadline = time.monotonic() + 2.0
            except queue.Empty:
                break
        page.wait_for_timeout(200)
        now = time.monotonic()
        if now >= next_frame_scan:
            discovered = discover_media_in_frames(page)
            if discovered is not None:
                candidates[discovered.url] = discovered
                if settle_deadline is None:
                    settle_deadline = now + 2.0
            next_frame_scan = now + 1.0
        if settle_deadline is not None and now >= settle_deadline:
            ranked = sorted(
                candidates.values(),
                key=lambda item: (
                    0
                    if urlparse(item.url).path.lower().endswith("/media.m3u8")
                    else 1,
                    0 if ".m3u8" in urlparse(item.url).path.lower() else 1,
                ),
            )
            return ranked[0]
    raise queue.Empty


def launch_context(playwright: Any, profile: Path, browser_name: str) -> Any:
    profile.mkdir(parents=True, exist_ok=True)
    choices = ("chrome", "msedge", "chromium") if browser_name == "auto" else (browser_name,)
    errors: list[str] = []
    for choice in choices:
        kwargs: dict[str, Any] = {
            "user_data_dir": str(profile),
            "headless": False,
            # Playwright otherwise launches Chromium with --no-sandbox, which
            # produces a security warning and can change the LMS login's device
            # fingerprint. Edge's normal Windows sandbox works here.
            "chromium_sandbox": True,
            "viewport": {"width": 1280, "height": 850},
        }
        if choice != "chromium":
            kwargs["channel"] = choice
        try:
            print(f"Trying browser: {choice}")
            return playwright.chromium.launch_persistent_context(**kwargs)
        except Exception as exc:
            errors.append(f"{choice}: {exc}")
    raise RuntimeError("No supported browser could be started:\n" + "\n".join(errors))


def capture_selected_video(
    page: Any,
    capture: Capture,
    selected_index: int,
    title: str,
    capture_timeout: int,
    PlaywrightError: Any,
) -> tuple[MediaRequest, bytes | None]:
    card = page.locator(".student-video-card").nth(selected_index)
    capture.clear()
    card.scroll_into_view_if_needed()
    try:
        card.dispatch_event("click")
    except PlaywrightError:
        page.locator(".student-video-card").nth(selected_index).dispatch_event("click")
    print(f"Waiting for the player stream for: {title}")

    try:
        media = wait_for_media(page, capture, capture_timeout)
    except queue.Empty as exc:
        raise RuntimeError(
            f"No media stream was detected for {title!r}. Confirm that the "
            "video starts playing in the browser, then try again."
        ) from exc

    # Give the player a moment to request either its ordinary AES key or a
    # commercial DRM licence.
    end = time.monotonic() + 4
    while time.monotonic() < end and not capture.drm_seen.is_set():
        page.wait_for_timeout(200)

    has_drm, detail = manifest_has_drm(media)
    if capture.drm_seen.is_set() or has_drm:
        reason = detail or "the player requested a commercial DRM licence"
        raise RuntimeError(
            f"{title!r} is DRM-protected and cannot be downloaded by this "
            f"script ({reason})."
        )
    if detail:
        print(f"Note: {detail}")

    hls_key: bytes | None = None
    key_url, key_headers = capture.get_hls_key_request()
    if key_url and key_headers:
        hls_key = fetch_hls_key(key_url, key_headers)
        print("Captured and verified the authorized AES key.")

    print(f"Captured authorized media: {urlparse(media.url).path}")
    try:
        page.evaluate(
            "document.querySelectorAll('video, audio').forEach(media => media.pause())"
        )
    except PlaywrightError:
        pass
    return media, hls_key


def capture_and_download(args: argparse.Namespace) -> None:
    sync_playwright, (PlaywrightError, yt_dlp) = import_dependencies()
    capture = Capture()
    profile = Path(args.profile_dir).expanduser().resolve()

    with sync_playwright() as playwright:
        try:
            context = launch_context(playwright, profile, args.browser)
        except PlaywrightError as exc:
            hint = (
                "Could not start the browser. Try --browser msedge, or run:\n"
                "  python -m playwright install chromium"
            )
            raise RuntimeError(f"{hint}\n\nBrowser error: {exc}") from exc

        page = context.pages[0] if context.pages else context.new_page()
        edge_storage = read_synapse_storage_from_edge()
        if edge_storage:
            install_synapse_storage(context, edge_storage)
            print(
                "Imported the existing Synapse session from regular Edge "
                "(the token is not displayed or copied to a file)."
            )

        def intercept_hls_key(route: Any, request: Any) -> None:
            # Vidinfra signs each AES-key request with a single-use nonce. Save
            # the browser-generated request but abort it so the nonce remains
            # unused for our authorized downloader request.
            try:
                headers = request.all_headers()
            except Exception:
                headers = request.headers
            capture.observe_request(request.url, headers)
            route.abort()

        context.route("https://kms.vidinfra.com/**", intercept_hls_key)

        def on_request(request: Any) -> None:
            try:
                # request.all_headers() performs an extra browser protocol call.
                # Inside a synchronous event callback that can fail for requests
                # made by Synapse's cross-origin Vidinfra iframe. The immediately
                # available headers property contains the replay headers we need
                # and, importantly, never drops the media URL from the queue.
                capture.observe_request(request.url, request.headers)
                capture.add(request.url, request.headers)
            except Exception:
                # A browser may dispose a request while a page is navigating.
                pass

        def on_response(response: Any) -> None:
            capture.observe_url(response.url)
            content_type = response.headers.get("content-type", "").lower()
            if any(kind in content_type for kind in MEDIA_CONTENT_TYPES):
                try:
                    capture.add(response.url, response.request.headers)
                except Exception:
                    pass

        # Context-level listeners cover the main LMS page, cross-origin
        # iframes, and any popup used by an embedded video provider.
        context.on("request", on_request)
        context.on("response", on_response)

        print(f"Opening {args.url}")
        try:
            page.goto(args.url, wait_until="domcontentloaded", timeout=90_000)
        except PlaywrightError:
            print("The page is still loading; continuing in the open browser.")

        session_ready = False
        if edge_storage:
            try:
                page.locator(".student-video-card").first.wait_for(
                    state="visible", timeout=60_000
                )
                session_ready = True
                print("Regular Edge session accepted; the lecture page is ready.")
            except PlaywrightError:
                print(
                    "The Edge session was found but the LMS did not accept it. "
                    "Refresh the LMS in regular Edge and try again."
                )

        if not session_ready:
            print(
                "\nSign in if needed and wait until the video cards appear.\n"
                "Your login is kept only in the dedicated browser profile:\n"
                f"  {profile}"
            )
            input("Press Enter here when the lecture page is ready... ")

        selections = choose_videos(
            page,
            args.index,
            args.videos,
            args.download_all,
        )
        total = len(selections)
        parallel_videos = min(args.parallel_videos, total)
        fragments_per_video = max(1, args.fragments // parallel_videos)
        print(
            f"\nSelected {total} video(s); downloading up to {parallel_videos} "
            f"at once with {fragments_per_video} fragment connections each."
        )

        futures: list[Future[None]] = []
        with ThreadPoolExecutor(max_workers=parallel_videos) as pool:
            for order, (selected_index, title) in enumerate(selections, start=1):
                if order > 1:
                    print(f"\nReloading the lecture page for capture {order}/{total}...")
                    page.goto(args.url, wait_until="domcontentloaded", timeout=90_000)
                    page.locator(".student-video-card").first.wait_for(
                        state="visible", timeout=90_000
                    )

                print(f"\nCapturing video {order}/{total}: {title}")
                media, hls_key = capture_selected_video(
                    page,
                    capture,
                    selected_index,
                    title,
                    args.capture_timeout,
                    PlaywrightError,
                )

                # Unload the iframe so it cannot keep streaming while the
                # independent download runs. The browser remains available to
                # capture the next selected video.
                page.goto("about:blank", wait_until="load")
                label = f"{order}/{total}" if total > 1 else ""
                futures.append(
                    pool.submit(
                        download_media,
                        media,
                        title,
                        Path(args.output_dir),
                        fragments_per_video,
                        yt_dlp,
                        hls_key,
                        label,
                    )
                )

            context.close()
            print("\nAll streams captured; the browser is closed.")

            failures: list[str] = []
            for future in as_completed(futures):
                try:
                    future.result()
                except Exception as exc:
                    failures.append(str(exc))
            if failures:
                raise RuntimeError(
                    f"{len(failures)} batch download(s) failed:\n- "
                    + "\n- ".join(failures)
                )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Capture and download a non-DRM stream from an authorized Synapse "
            "LMS browser session."
        )
    )
    parser.add_argument("url", nargs="?", default=DEFAULT_URL)
    parser.add_argument(
        "-o", "--output-dir", default="downloads", help="download folder"
    )
    parser.add_argument(
        "--profile-dir",
        default=str(default_profile_dir()),
        help="persistent browser profile used for the LMS login",
    )
    parser.add_argument(
        "--browser",
        choices=("auto", "chrome", "msedge", "chromium"),
        default="auto",
        help="browser Playwright should open (default: auto)",
    )
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument(
        "--index", type=int, help="download one 1-based video number"
    )
    selection.add_argument(
        "--videos",
        help="download a selection such as 1,3-5",
    )
    selection.add_argument(
        "--all",
        action="store_true",
        dest="download_all",
        help="download every video listed on the page",
    )
    parser.add_argument(
        "--parallel-videos",
        type=int,
        default=2,
        help="simultaneous video downloads in a batch (default: 2)",
    )
    parser.add_argument(
        "--fragments",
        type=int,
        default=16,
        help="total fragment connection budget shared by downloads (default: 16)",
    )
    parser.add_argument(
        "--capture-timeout",
        type=int,
        default=45,
        help="seconds to wait for the player request (default: 45)",
    )
    args = parser.parse_args()
    if not 1 <= args.fragments <= 32:
        parser.error("--fragments must be between 1 and 32")
    if not 1 <= args.parallel_videos <= 4:
        parser.error("--parallel-videos must be between 1 and 4")
    if args.capture_timeout < 5:
        parser.error("--capture-timeout must be at least 5 seconds")
    return args


def main() -> int:
    try:
        capture_and_download(parse_args())
        print("\nDownload complete.")
        return 0
    except KeyboardInterrupt:
        print("\nCancelled.", file=sys.stderr)
        return 130
    except Exception as exc:
        print(f"\nError: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
