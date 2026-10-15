// Mirrors the Rust `DownloadTask` (serde snake_case field names) and events.

export type TaskState =
  | "queued"
  | "resolving"
  | "downloading"
  | "paused"
  | "retrying"
  | "verifying"
  | "completed"
  | "failed"
  | "cancelled";

export type DownloadMode = "single" | "segmented";

export interface DownloadTask {
  id: string;
  source_url: string;
  display_url: string;
  filename: string;
  destination_path: string;
  state: TaskState;
  total_bytes: number | null;
  downloaded_bytes: number;
  mode: DownloadMode;
  etag: string | null;
  last_modified: string | null;
  retry_count: number;
  created_at: number;
  updated_at: number;
  last_error_code: string | null;
}

export interface ProgressEvent {
  id: string;
  downloaded: number;
  total: number | null;
}

/** Shape of a rejected command (`EngineError`). */
export interface EngineError {
  code: string;
  message: string;
}

export function isEngineError(e: unknown): e is EngineError {
  return (
    typeof e === "object" &&
    e !== null &&
    "message" in e &&
    typeof (e as Record<string, unknown>).message === "string"
  );
}

export function errorMessage(e: unknown): string {
  if (isEngineError(e)) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}
