import { useEffect, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { commands, type CoreError, type MountDetail, type SkillDetail, type UsageSummary } from "./bindings";
import { ago, date } from "./format";
import { HarnessChip } from "./ui";

type Load = { status: "loading" } | { status: "ok"; detail: SkillDetail } | { status: "error"; message: string };

/** Lets a long path wrap after a separator instead of mid-name. */
function breakable(path: string) {
  return path.split("\\").flatMap((part, i, all) => (i < all.length - 1 ? [part, "\\", <wbr key={i} />] : [part]));
}

function errorText(e: CoreError): string {
  return e.kind === "unknownId" ? "This skill is no longer installed." : e.message;
}

export function Detail({ id, revision }: { id: string; revision: number }) {
  const [load, setLoad] = useState<Load>({ status: "loading" });

  useEffect(() => {
    let live = true;
    commands
      .getSkill(id)
      .then((r) => {
        if (!live) return;
        setLoad(r.status === "ok" ? { status: "ok", detail: r.data } : { status: "error", message: errorText(r.error) });
      })
      .catch((e) => live && setLoad({ status: "error", message: String(e) }));
    return () => {
      live = false;
    };
  }, [id, revision]);

  if (load.status === "loading") return <aside className="detail" aria-busy="true" />;
  if (load.status === "error") {
    return (
      <aside className="detail">
        <p className="error">{load.message}</p>
      </aside>
    );
  }
  return <DetailBody d={load.detail} />;
}

function DetailBody({ d }: { d: SkillDetail }) {
  const { row } = d;
  return (
    <aside className="detail">
      <header className="detail-head">
        <div className="detail-title">
          <h2>{row.name}</h2>
          <button type="button" className="manage" disabled title="Deleting and disabling arrive in the next release">
            Manage…
          </button>
        </div>
        <p className="path" title={d.dir}>
          {d.dir}
        </p>
        <p className="oneline">
          <b>Origin.</b> {d.originNote}
        </p>
      </header>

      {d.problemNotes.length > 0 && (
        <section className="problems" aria-label="Problems">
          {d.problemNotes.map((n) => (
            <p key={n}>{n}</p>
          ))}
        </section>
      )}

      <section>
        <h3>Mounts</h3>
        {d.mounts.map((m) => (
          <Mount key={m.summary.id} m={m} />
        ))}
      </section>

      <section>
        <h3>Usage</h3>
        <UsageBlock u={row.usage} />
      </section>

      {d.frontmatter.length > 0 && (
        <section>
          <h3>Frontmatter</h3>
          <dl className="fields">
            {d.frontmatter.map((f) => (
              <div key={f.key}>
                <dt>{f.key}</dt>
                <dd>{f.value || <span className="sub">empty</span>}</dd>
              </div>
            ))}
          </dl>
        </section>
      )}

      <section>
        <h3>SKILL.md</h3>
        <div className="md">
          <Markdown remarkPlugins={[remarkGfm]}>{d.markdown}</Markdown>
        </div>
        {d.markdownTruncated && <p className="sub">Showing the first 512 KB.</p>}
      </section>
    </aside>
  );
}

function Mount({ m }: { m: MountDetail }) {
  const s = m.summary;
  const [revealError, setRevealError] = useState<string | null>(null);
  const reveal = (hop: number) =>
    commands
      .reveal(s.id, hop)
      .then((r) => setRevealError(r.status === "ok" ? null : errorText(r.error)))
      .catch((e) => setRevealError(String(e)));
  return (
    <div className="mount">
      <div className="mount-head">
        <HarnessChip tag={s.harness} />
        <span className={`scope scope-${s.scope.kind}`} title={s.scope.path ?? undefined}>
          {s.scope.label}
        </span>
        <span className="sub">{s.owner.label}</span>
        {s.state.kind === "shadowed" && <span className="state warn">shadowed</span>}
        {s.state.kind === "bundleDisabled" && <span className="state warn">bundle disabled</span>}
      </div>
      <p className="oneline">
        <b>Owner.</b> {m.ownerNote}
      </p>
      <ol className="chain">
        {m.chain.map((hop, i) => (
          <li key={hop.path} className={`hop hop-${hop.kind}`}>
            <span className="hop-node" aria-hidden />
            <div className="hop-body">
              <span className="hop-role">{i === 0 && hop.kind === "link" ? "entry (link)" : hop.kind === "folder" ? "folder" : "link"}</span>
              <code className="hop-path" title={hop.path}>
                {breakable(hop.path)}
              </code>
            </div>
            <button type="button" className="reveal" onClick={() => void reveal(i)}>
              Reveal
            </button>
          </li>
        ))}
      </ol>
      {revealError && <p className="error">Could not open Explorer: {revealError}</p>}
      <p className="keys sub">
        Answers to{" "}
        {m.keys.map((k) => (
          <code key={k}>{k}</code>
        ))}
      </p>
    </div>
  );
}

function UsageBlock({ u }: { u: UsageSummary }) {
  if (u.byHarness.every((h) => !h.tracked)) {
    return <p className="sub">No usage data yet. Counts appear here once transcripts are indexed.</p>;
  }
  return (
    <table className="usage">
      <thead>
        <tr>
          <th>Harness</th>
          <th className="num">All time</th>
          <th className="num">30 days</th>
          <th>Last used</th>
          <th>Counting since</th>
        </tr>
      </thead>
      <tbody>
        {u.byHarness.map((h) => (
          <tr key={h.harness.id}>
            <td>
              <HarnessChip tag={h.harness} />
            </td>
            {h.tracked ? (
              <>
                <td className="num">{h.total}</td>
                <td className="num">{h.last30d}</td>
                <td>{h.lastUsed ? ago(h.lastUsed) : <span className="sub">never</span>}</td>
                <td>{h.trackingSince ? date(h.trackingSince) : ""}</td>
              </>
            ) : (
              <td colSpan={4} className="nodata">
                no data yet
              </td>
            )}
          </tr>
        ))}
      </tbody>
    </table>
  );
}
