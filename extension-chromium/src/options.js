import { DEFAULT_PORT, getConfig, ping } from "./bridge.js";

const portEl = document.getElementById("port");
const tokenEl = document.getElementById("token");
const statusEl = document.getElementById("status");

async function load() {
  const { port, token } = await getConfig();
  portEl.value = port || DEFAULT_PORT;
  tokenEl.value = token || "";
}

function setStatus(msg, ok) {
  statusEl.textContent = msg;
  statusEl.style.color = ok ? "#16a34a" : "#dc2626";
}

document.getElementById("save").addEventListener("click", async () => {
  const port = parseInt(portEl.value, 10) || DEFAULT_PORT;
  const token = tokenEl.value.trim();
  await chrome.storage.local.set({ port, token });
  setStatus("Saved.", true);
});

document.getElementById("test").addEventListener("click", async () => {
  const port = parseInt(portEl.value, 10) || DEFAULT_PORT;
  setStatus("Testing…", true);
  const ok = await ping(port);
  setStatus(
    ok ? "DownloadOS is reachable." : "Could not reach DownloadOS on that port.",
    ok,
  );
});

load();
