import { useEffect, useRef, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { commands, type ActionRequest, type BundleRow, type CoreError, type MountDetail, type SkillDetail, type UsageSummary } from "./bindings";
import { ago, date } from "./format";
import { HarnessChip } from "./ui";

type Load = { status: "loading" } | { status: "ok"; detail: SkillDetail } | { status: "error"; message: string };

/** Lets a long path wrap after a separator instead of mid-name. */
function breakable(path: string) {
  return path.split(/([\\/])/).flatMap((part, i) => (i % 2 ? [part, <wbr key={i} />] : [part]));
}

function errorText(e: CoreError): string {
  return e.kind === "unknownId" ? "This skill is no longer installed." : e.message;
}

type Manage = (request: ActionRequest) => void;

export function Detail({ id, revision, bundles, trashName, onManage }: { id: string; revision: number; bundles: BundleRow[]; trashName: string; onManage: Manage }) {
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
  return <DetailBody d={load.detail} bundles={bundles} trashName={trashName} onManage={onManage} />;
}

function DetailBody({ d, bundles, trashName, onManage }: { d: SkillDetail; bundles: BundleRow[]; trashName: string; onManage: Manage }) {
  const { row } = d;
  return (
    <aside className="detail">
      <header className="detail-head">
        <div className="detail-title">
          <h2>{row.name}</h2>
          <ManageMenu d={d} bundles={bundles} trashName={trashName} onManage={onManage} />
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

type MenuItem = { label: string; hint: string; request: ActionRequest; danger?: boolean };

/** Every action that applies to this skill's mounts, body and bundles. The planner decides whether each is allowed. */
function ManageMenu({ d, bundles, trashName, onManage }: { d: SkillDetail; bundles: BundleRow[]; trashName: string; onManage: Manage }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [open]);

  const items: MenuItem[] = [];
  for (const m of d.mounts) {
    const where = `${m.summary.harness.label}, ${m.summary.scope.label}`;
    items.push({ label: `Disable in ${where}`, hint: m.summary.entry, request: { kind: "disable", mount: m.summary.id } });
    items.push({ label: `Remove from ${where}`, hint: m.summary.entry, request: { kind: "removeMount", mount: m.summary.id } });
  }
  for (const b of bundles.filter((b) => d.row.bundles.includes(b.id))) {
    const hint = `${b.memberCount} skills, ${b.harness.label}`;
    items.push(
      b.enabled
        ? { label: `Disable plugin ${b.name}`, hint, request: { kind: "bundleDisable", bundle: b.id } }
        : { label: `Enable plugin ${b.name}`, hint, request: { kind: "bundleEnable", bundle: b.id } },
    );
  }
  items.push({ label: "Delete everywhere", hint: `Every link, then the folder to the ${trashName}`, request: { kind: "deleteBody", skill: d.row.id }, danger: true });

  return (
    <div className="menu-anchor" ref={ref}>
      <button type="button" className="manage" aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        Manage…
      </button>
      {open && (
        <div className="menu" role="menu">
          {items.map((it) => (
            <button
              key={it.label + it.hint}
              type="button"
              role="menuitem"
              className={it.danger ? "danger" : undefined}
              onClick={() => {
                setOpen(false);
                onManage(it.request);
              }}
            >
              <span>{it.label}</span>
              <span className="sub">{it.hint}</span>
            </button>
          ))}
        </div>
      )}
    </div>
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
      {revealError && <p className="error">Could not open the folder: {revealError}</p>}
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
