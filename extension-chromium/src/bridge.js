// Shared helpers for talking to the DownloadOS local bridge.
//
// The bridge listens only on loopback (127.0.0.1). The extension authenticates
// with a pairing token the user copies from the desktop app's Settings.

export const DEFAULT_PORT = 14900;

export async function getConfig() {
  const { port, token } = await chrome.storage.local.get(["port", "token"]);
  return { port: port || DEFAULT_PORT, token: token || "" };
}

export function baseUrl(port) {
  return `http://127.0.0.1:${port}`;
}

/** Check whether the desktop app's bridge is reachable. */
export async function ping(port) {
  try {
    const res = await fetch(`${baseUrl(port)}/ping`, { method: "GET" });
    return res.ok;
  } catch {
    return false;
  }
}

/** Send a link to the desktop app. Returns {ok, status, body}. */
export async function sendHandoff(port, token, url, filename) {
  const payload = { url };
  if (filename) payload.filename = filename;
  const res = await fetch(`${baseUrl(port)}/handoff`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "X-DownloadOS-Token": token,
    },
    body: JSON.stringify(payload),
  });
  let body = null;
  try {
    body = await res.json();
  } catch {
    /* no/invalid JSON body */
  }
  return { ok: res.ok, status: res.status, body };
}
