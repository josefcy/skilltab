import type { ActionRequest, BundleRow, Problems, ShelfRow } from "./bindings";
import { ago } from "./format";
import { Empty, HarnessChip, plural } from "./ui";

type Manage = (request: ActionRequest) => void;

export function ShelfPanel({ shelf, onManage }: { shelf: ShelfRow[]; onManage: Manage }) {
  if (shelf.length === 0) {
    return <Empty title="The shelf is empty">Skills you disable, and links you remove, wait here until you restore them.</Empty>;
  }
  return (
    <div className="table-wrap">
      <table className="grid">
        <thead>
          <tr>
            <th className="static">Skill</th>
            <th className="static">Harness</th>
            <th className="static">Entry</th>
            <th className="static">State</th>
            <th className="static">Since</th>
            <th className="static" />
          </tr>
        </thead>
        <tbody>
          {shelf.map((s) => (
            <tr key={s.id} className="shelved" title={s.note}>
              <td>
                <span className="name">{s.name}</span>
              </td>
              <td>
                <HarnessChip tag={s.harness} />
              </td>
              <td className="mono">{s.entry}</td>
              <td>
                <span className="state warn">{s.reason}</span>
              </td>
              <td>{ago(s.at)}</td>
              <td>
                <button type="button" className="rowaction" onClick={() => onManage({ kind: "restore", shelf: s.id })}>
                  Restore…
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function BundlesPanel({ bundles, onPick, onManage }: { bundles: BundleRow[]; onPick: (id: string) => void; onManage: Manage }) {
  if (bundles.length === 0) {
    return <Empty title="No plugin bundles installed">Skills that ship inside a harness plugin are grouped here.</Empty>;
  }
  return (
    <div className="table-wrap">
      <table className="grid">
        <thead>
          <tr>
            <th className="static">Bundle</th>
            <th className="static">Harness</th>
            <th className="static">Version</th>
            <th className="static">Enabled</th>
            <th className="static num">Skills</th>
            <th className="static">Installed for</th>
            <th className="static" />
          </tr>
        </thead>
        <tbody>
          {bundles.map((b) => (
            <tr key={b.id} onClick={() => onPick(b.id)} title="Show these skills in the inventory">
              <td>
                <span className="name">{b.name}</span>
                <span className="sub">@{b.marketplace}</span>
              </td>
              <td>
                <HarnessChip tag={b.harness} />
              </td>
              <td className="mono">{b.version ?? <span className="sub">none</span>}</td>
              <td>{b.enabled ? "enabled" : <span className="state warn">disabled</span>}</td>
              <td className="num">{b.memberCount}</td>
              <td>
                <span className={`scope scope-${b.scope.kind}`}>{b.scope.label}</span>
              </td>
              <td>
                <button
                  type="button"
                  className="rowaction"
                  onClick={(e) => {
                    e.stopPropagation();
                    onManage(b.enabled ? { kind: "bundleDisable", bundle: b.id } : { kind: "bundleEnable", bundle: b.id });
                  }}
                >
                  {b.enabled ? "Disable…" : "Enable…"}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function ConflictsPanel({ problems, onPick }: { problems: Problems; onPick: (skillId: string) => void }) {
  if (problems.nameConflicts.length === 0) {
    return <Empty title="No name conflicts">Every installed skill has a name of its own.</Empty>;
  }
  return (
    <div className="panel-list">
      {problems.nameConflicts.map((c) => (
        <section key={c.name} className="conflict">
          <h3>
            {c.name}
            <span className="sub">
              {" "}
              {plural(c.members.length, "folder")}, {c.identicalContent ? "identical content" : "different content"}
            </span>
          </h3>
          <ul>
            {c.members.map((m) => (
              <li key={m.skill}>
                <button type="button" className="rowbtn" onClick={() => onPick(m.skill)}>
                  {m.harnesses.map((h) => (
                    <HarnessChip key={h.id} tag={h} />
                  ))}
                  <code className="path">{m.dir}</code>
                </button>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

export function BrokenPanel({ problems }: { problems: Problems }) {
  if (problems.brokenLinks.length === 0 && problems.other.length === 0) {
    return <Empty title="No broken links">Every link in a skills folder reaches a skill.</Empty>;
  }
  return (
    <div className="panel-list">
      {problems.brokenLinks.map((b) => (
        <section key={b.id} className="conflict">
          <h3>
            <HarnessChip tag={b.harness} />
          </h3>
          <p className="chainline">
            <code>{b.link}</code>
            <span className="sub"> points to a folder that is gone: </span>
            <code>{b.target}</code>
          </p>
        </section>
      ))}
      {problems.other.length > 0 && (
        <section className="conflict">
          <h3>Other scan problems</h3>
          <ul>
            {problems.other.map((o) => (
              <li key={o}>
                <code>{o}</code>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
