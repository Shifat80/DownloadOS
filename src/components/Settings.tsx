import { useState } from "react";
import type { BridgeInfo } from "../api";

interface Props {
  downloadDir: string;
  connections: number;
  bridge: BridgeInfo | null;
  onSave: (downloadDir: string, connections: number) => void;
  onClose: () => void;
}

export function Settings({ downloadDir, connections, bridge, onSave, onClose }: Props) {
  const [dir, setDir] = useState(downloadDir);
  const [conns, setConns] = useState(connections);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-label="Settings"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="modal-title">Settings</h2>

        <label className="field">
          <span className="field-label">Download folder</span>
          <input
            className="input"
            type="text"
            value={dir}
            onChange={(e) => setDir(e.target.value)}
            spellCheck={false}
          />
          <span className="field-hint">Type or paste a full folder path.</span>
        </label>

        <label className="field" style={{ marginTop: 16 }}>
          <span className="field-label">Connections per download: {conns}</span>
          <input
            type="range"
            min={1}
            max={16}
            value={conns}
            onChange={(e) => setConns(parseInt(e.target.value, 10))}
          />
          <span className="field-hint">
            More connections can speed up large downloads from servers that support ranges.
          </span>
        </label>

        <div className="field" style={{ marginTop: 16 }}>
          <span className="field-label">Browser pairing</span>
          {bridge && bridge.port > 0 ? (
            <>
              <div className="pair-row">
                <span className="pair-key">Port</span>
                <code className="pair-val">{bridge.port}</code>
              </div>
              <div className="pair-row">
                <span className="pair-key">Pairing code</span>
                <code className="pair-val">{bridge.token}</code>
                <button
                  type="button"
                  className="btn btn-ghost"
                  onClick={() => navigator.clipboard?.writeText(bridge.token)}
                >
                  Copy
                </button>
              </div>
              <span className="field-hint">
                Enter these in the DownloadOS browser extension's options to pair it.
              </span>
            </>
          ) : (
            <span className="field-hint">Browser bridge unavailable.</span>
          )}
        </div>

        <div className="modal-actions">
          <button className="btn btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button
            className="btn btn-primary"
            onClick={() => {
              onSave(dir.trim(), conns);
              onClose();
            }}
            disabled={!dir.trim()}
          >
            Save
          </button>
        </div>
      </div>
    </div>
  );
}
