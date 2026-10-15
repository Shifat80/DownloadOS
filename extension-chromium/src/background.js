// DownloadOS Handoff — background service worker.
//
// Adds a "Send to DownloadOS" item to the link context menu and forwards the
// link to the desktop app's loopback bridge. Only ordinary http/https links
// are sent. No browsing data, cookies, or credentials are collected or sent.

import { getConfig, ping, sendHandoff } from "./bridge.js";

const MENU_ID = "downloados-send-link";

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: MENU_ID,
    title: "Send to DownloadOS",
    contexts: ["link"],
  });
});

chrome.contextMenus.onClicked.addListener(async (info) => {
  if (info.menuItemId !== MENU_ID) return;
  const url = info.linkUrl;
  if (!isHttpUrl(url)) {
    notify("Not a downloadable link", "Only http(s) links can be sent.");
    return;
  }

  const { port, token } = await getConfig();
  if (!token) {
    notify(
      "Not paired yet",
      "Open the extension options and paste the pairing code from DownloadOS.",
    );
    chrome.runtime.openOptionsPage();
    return;
  }

  if (!(await ping(port))) {
    notify("DownloadOS not running", "Start the DownloadOS desktop app and try again.");
    return;
  }

  try {
    const { ok, status, body } = await sendHandoff(port, token, url);
    if (ok) {
      notify("Sent to DownloadOS", body?.filename ? `Downloading ${body.filename}` : "Download started.");
    } else if (status === 401 || status === 403) {
      notify("Pairing failed", "The pairing code is wrong. Re-pair in options.");
    } else {
      notify("Could not send link", body?.message || `Bridge returned ${status}.`);
    }
  } catch {
    notify("DownloadOS not reachable", "The desktop app did not respond.");
  }
});

function isHttpUrl(url) {
  try {
    const u = new URL(url);
    return u.protocol === "http:" || u.protocol === "https:";
  } catch {
    return false;
  }
}

function notify(title, message) {
  // Notifications are best-effort; ignore failures (e.g. permission off).
  try {
    chrome.notifications.create({
      type: "basic",
      iconUrl: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
      title,
      message,
    });
  } catch {
    /* ignore */
  }
}
