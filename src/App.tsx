import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import "./App.css";

import { api, onDownloadsChanged, onProgress, type BridgeInfo } from "./api";
import { errorMessage, type DownloadTask } from "./types";
import { AddBar } from "./components/AddBar";
import { Settings } from "./components/Settings";
import { Toolbar, matchesFilter, type Filter } from "./components/Toolbar";
import { TaskCard, type LiveProgress } from "./components/TaskCard";

export default function App() {
  const [tasks, setTasks] = useState<DownloadTask[]>([]);
  const [destDir, setDestDir] = useState("");
  const [connections, setConnections] = useState(4);
  const [bridge, setBridge] = useState<BridgeInfo | null>(null);
  const [filter, setFilter] = useState<Filter>("all");
  const [live, setLive] = useState<Record<string, LiveProgress>>({});
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [showSettings, setShowSettings] = useState(false);

  const samples = useRef<Record<string, { downloaded: number; t: number; speed: number }>>({});

  const refresh = useCallback(async () => {
    try {
      setTasks(await api.list());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  const flash = useCallback((msg: string) => setError(msg), []);

  // Initial load: settings, pairing info, and existing tasks.
  useEffect(() => {
    (async () => {
      try {
        const s = await api.getSettings();
        setDestDir(s.download_dir);
        setConnections(s.connections);
      } catch (e) {
        flash(errorMessage(e));
      }
      try {
        setBridge(await api.bridgeInfo());
      } catch {
        /* bridge optional */
      }
      await refresh();
    })();
  }, [refresh, flash]);

  // Event subscriptions.
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    let active = true;

    onDownloadsChanged(() => refresh()).then((u) => (active ? unlisteners.push(u) : u()));

    onProgress((p) => {
      const now = Date.now();
      const prev = samples.current[p.id];
      let speed = prev?.speed ?? 0;
      if (prev && now > prev.t) {
        const inst = ((p.downloaded - prev.downloaded) * 1000) / (now - prev.t);
        speed = prev.speed > 0 ? prev.speed * 0.6 + inst * 0.4 : inst;
      }
      samples.current[p.id] = { downloaded: p.downloaded, t: now, speed };
      setLive((cur) => ({ ...cur, [p.id]: { downloaded: p.downloaded, total: p.total, speed } }));
    }).then((u) => (active ? unlisteners.push(u) : u()));

    return () => {
      active = false;
      unlisteners.forEach((u) => u());
    };
  }, [refresh]);

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
        await api.add(url, filename);
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

  const saveSettings = useCallback(
    async (dir: string, conns: number) => {
      try {
        await api.setSettings(dir, conns);
        const s = await api.getSettings();
        setDestDir(s.download_dir);
        setConnections(s.connections);
      } catch (e) {
        flash(errorMessage(e));
      }
    },
    [flash],
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
          downloadDir={destDir}
          connections={connections}
          bridge={bridge}
          onSave={saveSettings}
          onClose={() => setShowSettings(false)}
        />
      )}
    </div>
  );
}
