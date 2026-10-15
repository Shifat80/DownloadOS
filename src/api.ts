// Thin typed wrappers over Tauri commands and events.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { DownloadTask, ProgressEvent } from "./types";

export const api = {
  defaultDir: () => invoke<string>("default_download_dir"),
  list: () => invoke<DownloadTask[]>("list_downloads"),
  details: (id: string) => invoke<DownloadTask | null>("get_download_details", { id }),
  add: (url: string, destDir: string, filename?: string) =>
    invoke<DownloadTask>("add_download", {
      url,
      destDir,
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
