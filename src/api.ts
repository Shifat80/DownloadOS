// Thin typed wrappers over Tauri commands and events.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { DownloadTask, ProgressEvent } from "./types";

export interface Settings {
  download_dir: string;
  connections: number;
}

export interface BridgeInfo {
  port: number;
  token: string;
}

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (downloadDir?: string, connections?: number) =>
    invoke<void>("set_settings", {
      downloadDir: downloadDir ?? null,
      connections: connections ?? null,
    }),
  bridgeInfo: () => invoke<BridgeInfo>("bridge_info"),

  list: () => invoke<DownloadTask[]>("list_downloads"),
  details: (id: string) => invoke<DownloadTask | null>("get_download_details", { id }),
  add: (url: string, filename?: string) =>
    invoke<DownloadTask>("add_download", {
      url,
      filename: filename && filename.trim() ? filename.trim() : null,
    }),
  pause: (id: string) => invoke<void>("pause_download", { id }),
  resume: (id: string) => invoke<void>("resume_download", { id }),
  cancel: (id: string) => invoke<void>("cancel_download", { id }),
  retry: (id: string) => invoke<void>("retry_download", { id }),
  remove: (id: string) => invoke<boolean>("remove_download", { id }),
};

/** Subscribe to the "list changed" signal. */
export function onDownloadsChanged(cb: () => void): Promise<UnlistenFn> {
  return listen("downloads:changed", () => cb());
}

/** Subscribe to throttled byte-progress events. */
export function onProgress(cb: (p: ProgressEvent) => void): Promise<UnlistenFn> {
  return listen<ProgressEvent>("download:progress", (e) => cb(e.payload));
}
