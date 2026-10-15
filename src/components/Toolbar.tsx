import type { DownloadTask, TaskState } from "../types";

export type Filter = "all" | "active" | "paused" | "completed" | "failed";

const ACTIVE: TaskState[] = ["queued", "resolving", "downloading", "retrying", "verifying"];

export function matchesFilter(task: DownloadTask, filter: Filter): boolean {
  switch (filter) {
    case "all":
      return true;
    case "active":
      return ACTIVE.includes(task.state);
    case "paused":
      return task.state === "paused";
    case "completed":
      return task.state === "completed";
    case "failed":
      return task.state === "failed" || task.state === "cancelled";
  }
}

interface Props {
  tasks: DownloadTask[];
  filter: Filter;
  onChange: (f: Filter) => void;
}

const TABS: { key: Filter; label: string }[] = [
  { key: "all", label: "All" },
  { key: "active", label: "Active" },
  { key: "paused", label: "Paused" },
  { key: "completed", label: "Completed" },
  { key: "failed", label: "Failed" },
];

export function Toolbar({ tasks, filter, onChange }: Props) {
  return (
    <div className="toolbar" role="tablist" aria-label="Filter downloads">
      {TABS.map((tab) => {
        const count = tasks.filter((t) => matchesFilter(t, tab.key)).length;
        return (
          <button
            key={tab.key}
            role="tab"
            aria-selected={filter === tab.key}
            className={`tab ${filter === tab.key ? "tab-active" : ""}`}
            onClick={() => onChange(tab.key)}
          >
            {tab.label}
            <span className="tab-count">{count}</span>
          </button>
        );
      })}
    </div>
  );
}
