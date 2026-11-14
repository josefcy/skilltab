import { useState } from "react";
import type { ActionRequest } from "./bindings";

const FOLDER_KEY = "skilltab.setupFolder";

function remembered(): string {
  try {
    return localStorage.getItem(FOLDER_KEY) ?? "";
  } catch {
    return "";
  }
}

function remember(folder: string) {
  try {
    localStorage.setItem(FOLDER_KEY, folder);
  } catch {
    // Private windows and blocked site data throw. The field then starts empty next time.
  }
}

export function SetupPanel({ onManage }: { onManage: (request: ActionRequest) => void }) {
  const [folder, setFolder] = useState(remembered);
  const trimmed = folder.trim();

  return (
    <div className="panel-list">
      <section className="conflict setup">
        <h3>Setup folder</h3>
        <p className="sub">A folder you keep in git. Export fills it on this machine. Restore reads it on another one.</p>
        <div className="setup-row">
          <input
            className="search setup-folder"
            type="text"
            aria-label="Setup folder"
            placeholder="~/skill-setup"
            spellCheck={false}
            value={folder}
            onChange={(e) => {
              setFolder(e.target.value);
              remember(e.target.value);
            }}
          />
          <button type="button" className="manage" disabled={!trimmed} onClick={() => onManage({ kind: "exportSetup", folder: trimmed })}>
            Export…
          </button>
          <button type="button" className="manage" disabled={!trimmed} onClick={() => onManage({ kind: "restoreSetup", folder: trimmed })}>
            Restore…
          </button>
        </div>
        <ul className="setup-notes">
          <li>
            <b>Export</b> writes <code>manifest.json</code> and <code>config/</code> into that folder and stops. It never commits or pushes.
          </li>
          <li>
            <b>Restore</b> shows a plan first and runs it only after you confirm. It never overwrites a folder or a config file. A config file that differs is
            written beside the original as <code>&lt;name&gt;.from-backup</code>.
          </li>
          <li>
            Never carried: <code>settings.json</code>, <code>config.toml</code>, <code>hooks.json</code>, auth files and sessions.
          </li>
        </ul>
      </section>
    </div>
  );
}
