import { useState } from "react";

interface Props {
  destDir: string;
  busy: boolean;
  onAdd: (url: string, filename: string) => void;
  onOpenSettings: () => void;
}

export function AddBar({ destDir, busy, onAdd, onOpenSettings }: Props) {
  const [url, setUrl] = useState("");
  const [filename, setFilename] = useState("");

  function submit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = url.trim();
    if (!trimmed) return;
    onAdd(trimmed, filename);
    setUrl("");
    setFilename("");
  }

  return (
    <form className="addbar" onSubmit={submit}>
      <div className="addbar-row">
        <input
          className="input url-input"
          type="url"
          inputMode="url"
          placeholder="Paste an https:// link to download"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          aria-label="Download URL"
          autoFocus
        />
        <button className="btn btn-primary" type="submit" disabled={busy || !url.trim()}>
          {busy ? "Adding…" : "Add"}
        </button>
      </div>
      <div className="addbar-row secondary">
        <input
          className="input"
          type="text"
          placeholder="Optional filename"
          value={filename}
          onChange={(e) => setFilename(e.target.value)}
          aria-label="Optional filename"
        />
        <button type="button" className="btn btn-ghost dest-btn" onClick={onOpenSettings}>
          <span className="dest-label">Save to</span>
          <span className="dest-path" title={destDir}>
            {destDir || "…"}
          </span>
        </button>
      </div>
    </form>
  );
}
