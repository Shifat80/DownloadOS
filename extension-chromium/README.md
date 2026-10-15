# DownloadOS Handoff (Chromium extension)

Sends ordinary `http(s)` download links from the browser to the DownloadOS
desktop app via a hardened loopback bridge.

## What it does / does not do

- Adds a **"Send to DownloadOS"** item to the right-click menu on links.
- Sends only the link URL you click (optionally a filename). It does **not**
  read page content, browsing history, cookies, or credentials, and sends
  nothing to any remote server — only to `127.0.0.1` (your own machine).
- Minimal permissions: `contextMenus`, `storage` (to remember the pairing
  code), `notifications`, and host access limited to `http://127.0.0.1/*`.

## Install (unpacked, for development)

1. Open `chrome://extensions` and enable **Developer mode**.
2. Click **Load unpacked** and select this `extension-chromium/` folder.

## Pair with the desktop app

1. Start the DownloadOS desktop app.
2. In the app, open **Settings → Browser pairing** and copy the **port** and
   **pairing code**.
3. Open the extension's **Options** page, paste the port and code, click
   **Save**, then **Test connection**.

## Security model

- The desktop app's bridge binds to loopback only (`127.0.0.1`).
- Every `/handoff` request must carry the pairing code (`X-DownloadOS-Token`)
  and originate from a `chrome-extension://` origin; web pages are rejected.
- Request bodies are size-limited, rate-limited, and schema-validated; only
  `http`/`https` URLs are accepted. See `docs/SECURITY_AND_POLICY.md` §6 and
  `docs/BRIDGE_SECURITY.md`.
