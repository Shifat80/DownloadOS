import type { DownloadTask, TaskState } from "../types";
import { formatBytes, formatEta, formatSpeed, percent } from "../format";

export interface LiveProgress {
  downloaded: number;
  total: number | null;
  speed: number | null;
}

interface Props {
  task: DownloadTask;
  live?: LiveProgress;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onCancel: (id: string) => void;
  onRetry: (id: string) => void;
  onRemove: (id: string) => void;
}

const STATE_LABEL: Record<TaskState, string> = {
  queued: "Queued",
  resolving: "Resolving",
  downloading: "Downloading",
  paused: "Paused",
  retrying: "Retrying",
  verifying: "Verifying",
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled",
};

const ACTIVE_STATES: TaskState[] = ["queued", "resolving", "downloading", "retrying", "verifying"];

export function TaskCard({ task, live, onPause, onResume, onCancel, onRetry, onRemove }: Props) {
  const downloaded = live?.downloaded ?? task.downloaded_bytes;
  const total = task.total_bytes ?? live?.total ?? null;
  const pct = percent(downloaded, total);
  const speed = task.state === "downloading" ? (live?.speed ?? null) : null;
  const eta = speed && total ? (total - downloaded) / speed : null;

  const indeterminate = task.state === "downloading" && pct == null;

  return (
    <article className="card" aria-label={`Download ${task.filename}`}>
      <div className="card-top">
        <div className="card-ident">
          <span className="filename" title={task.filename}>
            {task.filename}
          </span>
          <span className="url" title={task.display_url}>
            {task.display_url}
          </span>
        </div>
        <span className={`badge badge-${task.state}`}>{STATE_LABEL[task.state]}</span>
      </div>

      <div className={`progress ${indeterminate ? "progress-indeterminate" : ""}`}>
        <div
          className="progress-fill"
          style={pct != null ? { width: `${pct}%` } : undefined}
          role="progressbar"
          aria-valuenow={pct ?? undefined}
          aria-valuemin={0}
          aria-valuemax={100}
        />
      </div>

      <div className="card-meta">
        <span>
          {formatBytes(downloaded)}
          {total != null ? ` of ${formatBytes(total)}` : ""}
          {pct != null ? ` · ${pct.toFixed(0)}%` : ""}
        </span>
        <span className="meta-right">
          {task.state === "downloading" && (
            <>
              {formatSpeed(speed)} · ETA {formatEta(eta)}
            </>
          )}
          {task.state === "failed" && task.last_error_code && (
            <span className="error-code">{task.last_error_code}</span>
          )}
        </span>
      </div>

      <div className="card-actions">
        {task.state === "downloading" && (
          <button className="btn" onClick={() => onPause(task.id)}>
            Pause
          </button>
        )}
        {task.state === "paused" && (
          <button className="btn btn-primary" onClick={() => onResume(task.id)}>
            Resume
          </button>
        )}
        {ACTIVE_STATES.includes(task.state) || task.state === "paused" ? (
          <button className="btn" onClick={() => onCancel(task.id)}>
            Cancel
          </button>
        ) : null}
        {task.state === "failed" && (
          <button className="btn btn-primary" onClick={() => onRetry(task.id)}>
            Retry
          </button>
        )}
        {!ACTIVE_STATES.includes(task.state) && (
          <button className="btn btn-ghost" onClick={() => onRemove(task.id)}>
            Remove
          </button>
        )}
      </div>
    </article>
  );
}
