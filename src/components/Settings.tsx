import { useState } from "react";

interface Props {
  destDir: string;
  onSave: (dir: string) => void;
  onClose: () => void;
}

export function Settings({ destDir, onSave, onClose }: Props) {
  const [dir, setDir] = useState(destDir);

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
          <span className="field-hint">
            Files are saved here. Type or paste a full folder path.
          </span>
        </label>

        <div className="modal-actions">
          <button className="btn btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button
            className="btn btn-primary"
            onClick={() => {
              onSave(dir.trim());
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
