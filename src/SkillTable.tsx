import { useEffect, useRef } from "react";
import type { ShelfRow, SkillRow } from "./bindings";
import type { Sort, SortKey } from "./filters";
import { ago } from "./format";
import { Empty, HarnessChip } from "./ui";

type Props = {
  rows: SkillRow[];
  /** Disabled rows, shown after the installed ones. */
  shelved: ShelfRow[];
  onRestore: (shelfId: string) => void;
  selected: string | null;
  onSelect: (id: string) => void;
  sort: Sort;
  onSort: (key: SortKey) => void;
};

const COLUMNS: { key: SortKey; label: string; hint?: string; className?: string }[] = [
  { key: "name", label: "Name", className: "c-name" },
  { key: "description", label: "Description", className: "c-desc" },
  { key: "harnesses", label: "Harnesses", className: "c-harness" },
  { key: "scopes", label: "Scope", className: "c-scope" },
  { key: "owner", label: "Owner", className: "c-owner" },
  { key: "origin", label: "Origin", className: "c-origin" },
  { key: "uses", label: "Uses", hint: "all time, last 30 days", className: "c-uses num" },
  { key: "lastUsed", label: "Last used", className: "c-last" },
];

function Uses({ r }: { r: SkillRow }) {
  if (!r.usage.tracked) return <span className="nodata">no data yet</span>;
  return (
    <>
      <span className={r.usage.unused ? "zero" : undefined}>{r.usage.total}</span>
      <span className="sub"> / {r.usage.last30d}</span>
    </>
  );
}

function Last({ r }: { r: SkillRow }) {
  if (!r.usage.tracked) return <span className="nodata">no data yet</span>;
  return r.usage.lastUsed ? <>{ago(r.usage.lastUsed)}</> : <span className="sub">never</span>;
}

export function SkillTable({ rows, shelved, onRestore, selected, onSelect, sort, onSort }: Props) {
  const selectedRef = useRef<HTMLTableRowElement | null>(null);

  useEffect(() => {
    selectedRef.current?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  if (rows.length === 0 && shelved.length === 0) {
    return (
      <Empty title="No skills match these filters">Clear a filter or change the search to see more.</Empty>
    );
  }

  return (
    <div className="table-wrap">
      <table className="grid">
        <thead>
          <tr>
            {COLUMNS.map((c) => (
              <th
                key={c.key}
                className={c.className}
                aria-sort={sort.key === c.key ? (sort.dir === "asc" ? "ascending" : "descending") : "none"}
              >
                <button type="button" onClick={() => onSort(c.key)} title={c.hint}>
                  {c.label}
                  <span className="arrow" aria-hidden>
                    {sort.key === c.key ? (sort.dir === "asc" ? "▲" : "▼") : ""}
                  </span>
                </button>
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => {
            const isSel = r.id === selected;
            return (
              <tr
                key={r.id}
                ref={isSel ? selectedRef : undefined}
                aria-selected={isSel}
                onClick={() => onSelect(r.id)}
              >
                <td className="c-name">
                  <span className="name">{r.name}</span>
                  {r.flags.length > 0 && (
                    <span className="flag" title={r.flags.join(", ")}>
                      !
                    </span>
                  )}
                </td>
                <td className="c-desc" title={r.description ?? undefined}>
                  {r.description ?? <span className="sub">none</span>}
                </td>
                <td className="c-harness">
                  {r.harnesses.map((h) => (
                    <HarnessChip key={h.id} tag={h} />
                  ))}
                </td>
                <td className="c-scope">
                  {r.scopes.map((s, i) => (
                    <span key={`${s.kind}:${s.label}:${i}`} className={`scope scope-${s.kind}`} title={s.path ?? undefined}>
                      {s.label}
                    </span>
                  ))}
                </td>
                <td className="c-owner">{r.owners.map((o) => o.label).join(", ")}</td>
                <td className="c-origin">
                  {r.origin.label}
                  {r.origin.detail && <span className="sub"> {r.origin.detail}</span>}
                </td>
                <td className="c-uses num">
                  <Uses r={r} />
                </td>
                <td className="c-last">
                  <Last r={r} />
                </td>
              </tr>
            );
          })}
          {shelved.map((s) => (
            <tr key={s.id} className="shelved" title={s.note}>
              <td className="c-name">
                <span className="name">{s.name}</span>
              </td>
              <td className="c-desc">
                <span className="state warn">{s.reason}</span>
                <button type="button" className="rowaction" onClick={() => onRestore(s.id)}>
                  Restore…
                </button>
              </td>
              <td className="c-harness">
                <HarnessChip tag={s.harness} />
              </td>
              <td className="c-scope" />
              <td className="c-owner">skilltab shelf</td>
              <td className="c-origin" />
              <td className="c-uses num" />
              <td className="c-last">{ago(s.at)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
