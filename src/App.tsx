import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import "./App.css";

import { api, onDownloadsChanged, onProgress } from "./api";
import { errorMessage, type DownloadTask } from "./types";
import { AddBar } from "./components/AddBar";
import { Settings } from "./components/Settings";
import { Toolbar, matchesFilter, type Filter } from "./components/Toolbar";
import { TaskCard, type LiveProgress } from "./components/TaskCard";

const DEST_KEY = "downloados.destDir";

function loadStoredDir(): string {
  try {
    return localStorage.getItem(DEST_KEY) ?? "";
  } catch {
    return "";
  }
}

function storeDir(dir: string) {
  try {
    localStorage.setItem(DEST_KEY, dir);
  } catch {
    /* ignore: private mode / blocked storage */
  }
}

export default function App() {
  const [tasks, setTasks] = useState<DownloadTask[]>([]);
  const [destDir, setDestDir] = useState<string>(loadStoredDir());
  const [filter, setFilter] = useState<Filter>("all");
  const [live, setLive] = useState<Record<string, LiveProgress>>({});
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [showSettings, setShowSettings] = useState(false);

  // Previous progress samples for speed estimation.
  const samples = useRef<Record<string, { downloaded: number; t: number; speed: number }>>({});

  const refresh = useCallback(async () => {
    try {
      setTasks(await api.list());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  const flash = useCallback((msg: string) => {
    setError(msg);
  }, []);

  // Initial load: default destination + existing tasks.
  useEffect(() => {
    (async () => {
      if (!loadStoredDir()) {
        try {
          const dir = await api.defaultDir();
          setDestDir(dir);
          storeDir(dir);
        } catch {
          /* leave empty; user sets it in Settings */
        }
      }
      await refresh();
    })();
  }, [refresh]);

  // Event subscriptions.
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    let active = true;

    onDownloadsChanged(() => refresh()).then((u) => {
      if (active) unlisteners.push(u);
      else u();
    });

    onProgress((p) => {
      const now = Date.now();
      const prev = samples.current[p.id];
      let speed = prev?.speed ?? 0;
      if (prev && now > prev.t) {
        const inst = ((p.downloaded - prev.downloaded) * 1000) / (now - prev.t);
        // Exponential smoothing keeps the number readable.
        speed = prev.speed > 0 ? prev.speed * 0.6 + inst * 0.4 : inst;
      }
      samples.current[p.id] = { downloaded: p.downloaded, t: now, speed };
      setLive((cur) => ({
        ...cur,
        [p.id]: { downloaded: p.downloaded, total: p.total, speed },
      }));
    }).then((u) => {
      if (active) unlisteners.push(u);
      else u();
    });

    return () => {
      active = false;
      unlisteners.forEach((u) => u());
    };
  }, [refresh]);

  // Auto-dismiss errors.
  useEffect(() => {
    if (!error) return;
    const t = setTimeout(() => setError(null), 6000);
    return () => clearTimeout(t);
  }, [error]);

  const handleAdd = useCallback(
    async (url: string, filename: string) => {
      if (!destDir) {
        flash("Choose a download folder in Settings first.");
        setShowSettings(true);
        return;
      }
      setBusy(true);
      try {
        await api.add(url, destDir, filename);
        await refresh();
      } catch (e) {
        flash(errorMessage(e));
      } finally {
        setBusy(false);
      }
    },
    [destDir, flash, refresh],
  );

  const action = useCallback(
    (fn: (id: string) => Promise<unknown>) => async (id: string) => {
      try {
        await fn(id);
        await refresh();
      } catch (e) {
        flash(errorMessage(e));
      }
    },
    [flash, refresh],
  );

  const visible = useMemo(() => tasks.filter((t) => matchesFilter(t, filter)), [tasks, filter]);

  return (
    <div className="app">
      <header className="app-header">
        <div className="brand">
          <span className="logo-dot" aria-hidden />
          <h1>DownloadOS</h1>
        </div>
        <button className="btn btn-ghost" onClick={() => setShowSettings(true)}>
          Settings
        </button>
      </header>

      <AddBar
        destDir={destDir}
        busy={busy}
        onAdd={handleAdd}
        onOpenSettings={() => setShowSettings(true)}
      />

      <Toolbar tasks={tasks} filter={filter} onChange={setFilter} />

      {error && (
        <div className="banner banner-error" role="alert">
          {error}
          <button className="banner-close" onClick={() => setError(null)} aria-label="Dismiss">
            ×
          </button>
        </div>
      )}

      <main className="list">
        {visible.length === 0 ? (
          <div className="empty">
            <p className="empty-title">
              {tasks.length === 0 ? "No downloads yet" : "Nothing here"}
            </p>
            <p className="empty-sub">
              {tasks.length === 0
                ? "Paste a link above to start your first download."
                : "No downloads match this filter."}
            </p>
          </div>
        ) : (
          visible.map((task) => (
            <TaskCard
              key={task.id}
              task={task}
              live={live[task.id]}
              onPause={action(api.pause)}
              onResume={action(api.resume)}
              onCancel={action(api.cancel)}
              onRetry={action(api.retry)}
              onRemove={action(api.remove)}
            />
          ))
        )}
      </main>

      {showSettings && (
        <Settings
          destDir={destDir}
          onSave={(dir) => {
            setDestDir(dir);
            storeDir(dir);
          }}
          onClose={() => setShowSettings(false)}
        />
      )}
    </div>
  );
}
