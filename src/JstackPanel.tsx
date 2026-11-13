import { useState } from "react";
import { commands, type ActionRequest, type JstackView, type NotInstalledRow } from "./bindings";
import { ago } from "./format";
import { Empty, HarnessChip, JstackBadge, plural } from "./ui";

type Props = {
  /** undefined while loading, null when there is no clone. */
  jstack: JstackView | null | undefined;
  onManage: (request: ActionRequest) => void;
  onPick: (skillId: string) => void;
};

export function JstackPanel({ jstack, onManage, onPick }: Props) {
  if (jstack === undefined) return <Empty title="Reading the jstack clone…" />;
  if (jstack === null) {
    return <Empty title="No jstack clone found">Clone your jstack repo to ~\jstack, then press Rescan.</Empty>;
  }
  const drifted = jstack.relations.filter((r) => r.relation.drifted);
  const inSync = jstack.relations.filter((r) => !r.relation.drifted);
  const groups = groupByHarness(jstack.notInstalled);

  return (
    <div className="panel-list">
      <RepoHeader jstack={jstack} onManage={onManage} />

      <section className="conflict">
        <h3>
          Drifted <span className="sub">{drifted.length === 0 ? "nothing differs from the repo" : plural(drifted.length, "skill")}</span>
        </h3>
        {drifted.length > 0 && (
          <ul>
            {drifted.map((r) => (
              <li key={r.skillId}>
                <button type="button" className="rowbtn" onClick={() => onPick(r.skillId)} title="Show in the inventory">
                  <span className="name">{r.name}</span>
                  {r.harnesses.map((h) => (
                    <HarnessChip key={h.id} tag={h} />
                  ))}
                  <JstackBadge j={r.relation} />
                  <span className="sub grow">{r.relation.note}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
        {inSync.length > 0 && (
          <p className="sub jline">
            In sync: {inSync.map((r) => r.name).join(", ")}
          </p>
        )}
      </section>

      <section className="conflict">
        <h3>
          Not installed <span className="sub">{jstack.notInstalled.length === 0 ? "every harness has every jstack skill" : "repo skills a harness lacks"}</span>
        </h3>
        {groups.map(([harness, rows]) => (
          <div key={harness.id} className="jgroup">
            <p className="jgroup-head">
              <HarnessChip tag={harness} />
              <span className="sub">{plural(rows.length, "skill")} missing</span>
            </p>
            <ul>
              {rows.map((n) => (
                <li key={n.skill} className="jrow" title={n.note}>
                  <span className="name">{n.skill}</span>
                  <span className={n.reason === "nameTaken" ? "state warn" : "state"}>{n.reason === "nameTaken" ? "name taken" : "not linked"}</span>
                  <span className="sub grow">{n.note}</span>
                </li>
              ))}
            </ul>
          </div>
        ))}
      </section>

      <section className="conflict">
        <h3>
          Declared sources <span className="sub">skill-sources.txt</span>
        </h3>
        {jstack.declared.length === 0 && <p className="sub">The repo declares no outside sources.</p>}
        {jstack.declared.map((d) => (
          <div key={d.source} className="jgroup">
            <p className="jgroup-head">
              <code>{d.source}</code>
              <span className="grow" />
              {d.missing.length > 0 && (
                <button type="button" className="rowaction" onClick={() => onManage({ kind: "jstackInstallDeclared", source: d.source, skills: d.missing })}>
                  Install missing ({d.missing.length})…
                </button>
              )}
            </p>
            <p className="jskills">
              {d.skills.map((s) => (
                <span key={s.name} className={s.installed ? "jskill" : "jskill missing"}>
                  {s.name}
                  <span className="sub"> {s.installed ? "installed" : "missing"}</span>
                </span>
              ))}
            </p>
          </div>
        ))}
      </section>
    </div>
  );
}

function groupByHarness(rows: NotInstalledRow[]) {
  const groups: [NotInstalledRow["harness"], NotInstalledRow[]][] = [];
  for (const r of rows) {
    const g = groups.find(([h]) => h.id === r.harness.id);
    if (g) g[1].push(r);
    else groups.push([r.harness, [r]]);
  }
  return groups;
}

function RepoHeader({ jstack, onManage }: { jstack: JstackView; onManage: (request: ActionRequest) => void }) {
  const { repo } = jstack;
  const [fetching, setFetching] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const fetch = () => {
    setFetching(true);
    setError(null);
    commands
      .jstackFetch()
      .then((r) => r.status === "error" && setError(r.error.kind === "unknownId" ? "The clone is gone. Rescan." : r.error.message))
      .catch((e) => setError(String(e)))
      .finally(() => setFetching(false));
  };

  const up = repo.upstream;
  return (
    <section className="conflict jrepo">
      <h3>
        Your jstack clone
        <span className="sub"> read only here. skilltab never pushes.</span>
      </h3>
      <p className="jline">
        <code className="path">{repo.root}</code>
      </p>
      <dl className="jfacts">
        <div>
          <dt>Remote</dt>
          <dd>{repo.remote ? <code>{repo.remote}</code> : <span className="sub">none</span>}</dd>
        </div>
        <div>
          <dt>Branch</dt>
          <dd>
            <code>{repo.branch ?? "detached"}</code> at <code>{repo.head}</code>
          </dd>
        </div>
        <div>
          <dt>Upstream</dt>
          <dd>
            {up ? (
              <>
                <code>{up.name}</code> ahead {up.ahead},{" "}
                <span className={up.behind > 0 ? "state warn" : undefined}>behind {up.behind}</span>
              </>
            ) : (
              <span className="sub">none, so ahead and behind are unknown</span>
            )}
          </dd>
        </div>
        <div>
          <dt>Fetched</dt>
          <dd>
            {repo.fetchedAt ? ago(repo.fetchedAt) : <span className="sub">never</span>}
            <button type="button" className="rowaction" onClick={fetch} disabled={fetching}>
              {fetching ? "Fetching…" : "Fetch"}
            </button>
          </dd>
        </div>
        <div>
          <dt>Edits</dt>
          <dd>
            {repo.dirty.length === 0 ? (
              <span className="sub">none under skills/</span>
            ) : (
              <span className="state warn" title={repo.dirty.join("\n")}>
                {plural(repo.dirty.length, "file")} under skills/ differ from HEAD
              </span>
            )}
          </dd>
        </div>
      </dl>
      {error && <p className="error">Fetch failed: {error}</p>}
      <div className="jactions">
        <button type="button" className="manage" onClick={() => onManage({ kind: "jstackPull" })} title="git pull --ff-only in the clone">
          Pull…
        </button>
        <button
          type="button"
          className="manage"
          onClick={() => onManage({ kind: "jstackSync" })}
          title={repo.canSync ? "Run jstack's sync-skills.ps1" : "sync-skills.ps1 is not in the clone"}
        >
          Sync…
        </button>
        <span className="sub">Each one shows its plan first and changes nothing until you confirm.</span>
      </div>
    </section>
  );
}
