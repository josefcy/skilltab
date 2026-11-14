import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ActionDialog } from "./ActionDialog";
import type { ActionRequest } from "./bindings";
import { Detail } from "./Detail";
import { BrokenPanel, BundlesPanel, ConflictsPanel, ShelfPanel } from "./Panels";
import { BulkBar } from "./BulkBar";
import { SkillTable } from "./SkillTable";
import { Toolbar } from "./Toolbar";
import { UsagePanel } from "./UsagePanel";
import { filterAndSort, NO_FILTERS, type Filters, type Sort, type SortKey } from "./filters";
import { hueMap, HueProvider } from "./ui";
import { useInventory } from "./useInventory";
import { useAliasSuggestions } from "./useSideData";

/** Checkboxes are not text fields, so keys keep working after a tick. */
function isTextField(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target instanceof HTMLInputElement) return !["checkbox", "radio"].includes(target.type);
  return target.tagName === "SELECT" || target.tagName === "TEXTAREA";
}

type Tab = "skills" | "usage" | "shelf" | "bundles" | "conflicts" | "broken";

export default function App() {
  const { inventory: inv, error, firstLoadMs, refresh, refreshing } = useInventory();
  const [tab, setTab] = useState<Tab>("skills");
  const [filters, setFilters] = useState<Filters>(NO_FILTERS);
  const [sort, setSort] = useState<Sort>({ key: "name", dir: "asc" });
  const [selected, setSelected] = useState<string | null>(null);
  const [checked, setChecked] = useState<ReadonlySet<string>>(new Set());
  const [action, setAction] = useState<ActionRequest | null>(null);
  const closeAction = useCallback(() => setAction(null), []);
  const searchRef = useRef<HTMLInputElement | null>(null);
  const suggestions = useAliasSuggestions(inv?.revision ?? 0);

  const rows = useMemo(() => (inv ? filterAndSort(inv.rows, filters, sort) : []), [inv, filters, sort]);
  // Shelved rows have no usage, owner or origin, so they only show under the name and harness filters.
  const shelved = useMemo(() => {
    if (!inv) return [];
    const onlyNameAndHarness = JSON.stringify({ ...filters, text: "", harnesses: [] }) === JSON.stringify(NO_FILTERS);
    if (!onlyNameAndHarness) return [];
    const text = filters.text.toLowerCase();
    return inv.shelf.filter(
      (s) => s.name.toLowerCase().includes(text) && (filters.harnesses.length === 0 || filters.harnesses.includes(s.harness.id)),
    );
  }, [inv, filters]);
  const hue = useMemo(() => (inv ? hueMap(inv.facets) : () => 210), [inv]);

  // A rescan can drop skills, and a tick must never outlive its row.
  useEffect(() => {
    if (!inv) return;
    const ids = new Set(inv.rows.map((r) => r.id));
    setChecked((prev) => {
      const kept = [...prev].filter((id) => ids.has(id));
      return kept.length === prev.size ? prev : new Set(kept);
    });
  }, [inv]);

  const check = useCallback(
    (ids: string[], on: boolean) =>
      setChecked((prev) => {
        const next = new Set(prev);
        for (const id of ids) {
          if (on) next.add(id);
          else next.delete(id);
        }
        return next;
      }),
    [],
  );
  const clearChecked = useCallback(() => setChecked(new Set()), []);

  const onSort = (key: SortKey) =>
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: key === "uses" || key === "lastUsed" ? "desc" : "asc" }));

  const showBundle = (bundle: string) => {
    setFilters({ ...NO_FILTERS, bundle });
    setTab("skills");
  };
  const showSkill = (id: string) => {
    setFilters(NO_FILTERS);
    setSelected(id);
    setTab("skills");
  };

  const step = useCallback(
    (delta: number) => {
      if (rows.length === 0) return;
      const at = rows.findIndex((r) => r.id === selected);
      const next = at < 0 ? (delta > 0 ? 0 : rows.length - 1) : Math.min(rows.length - 1, Math.max(0, at + delta));
      setSelected(rows[next].id);
    },
    [rows, selected],
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (action) return;
      const typing = isTextField(e.target);
      if (e.key === "/" && !typing) {
        e.preventDefault();
        setTab("skills");
        searchRef.current?.focus();
        searchRef.current?.select();
      } else if (e.key === "Escape") {
        if (typing && filters.text) setFilters((f) => ({ ...f, text: "" }));
        else {
          if (e.target instanceof HTMLElement) e.target.blur();
          if (!typing && tab === "skills") clearChecked();
        }
      } else if (tab === "skills" && !typing && (e.key === "ArrowDown" || e.key === "j")) {
        e.preventDefault();
        step(1);
      } else if (tab === "skills" && !typing && (e.key === "ArrowUp" || e.key === "k")) {
        e.preventDefault();
        step(-1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [tab, step, filters.text, action, clearChecked]);

  if (!inv) {
    return <main className="boot">{error ? <p className="error">Could not read the inventory: {error}</p> : <p className="sub">Scanning skills…</p>}</main>;
  }

  const stillThere = selected && inv.rows.some((r) => r.id === selected) ? selected : null;
  const tabs: { id: Tab; label: string; n: number }[] = [
    { id: "skills", label: "Skills", n: inv.rows.length },
    { id: "usage", label: "Usage history", n: suggestions?.length ?? 0 },
    { id: "shelf", label: "Shelf", n: inv.shelf.length },
    { id: "bundles", label: "Bundles", n: inv.bundles.length },
    { id: "conflicts", label: "Name conflicts", n: inv.problems.nameConflicts.length },
    { id: "broken", label: "Broken links", n: inv.problems.brokenLinks.length + inv.problems.other.length },
  ];

  return (
    <HueProvider value={hue}>
      <div className="app">
        <nav className="tabs" role="tablist">
          {tabs.map((t) => (
            <button
              key={t.id}
              type="button"
              role="tab"
              aria-selected={tab === t.id}
              className={t.n > 0 && (t.id === "broken" || t.id === "conflicts") ? "attn" : undefined}
              onClick={() => setTab(t.id)}
            >
              {t.label}
              <span className="count">{t.n}</span>
            </button>
          ))}
          <span className="spacer" />
          <button type="button" className="link" onClick={refresh} disabled={refreshing}>
            {refreshing ? "Rescanning…" : "Rescan"}
          </button>
        </nav>

        {tab === "skills" && (
          <>
            <Toolbar inv={inv} filters={filters} onChange={(patch) => setFilters((f) => ({ ...f, ...patch }))} shown={rows.length} searchRef={searchRef} />
            <div className="split">
              <div className="table-pane">
                {checked.size > 0 && (
                  <BulkBar
                    selected={checked.size}
                    hidden={checked.size - rows.filter((r) => checked.has(r.id)).length}
                    onDelete={() => setAction({ kind: "deleteBodies", skills: inv.rows.filter((r) => checked.has(r.id)).map((r) => r.id) })}
                    onClear={clearChecked}
                  />
                )}
                <SkillTable
                  rows={rows}
                  shelved={shelved}
                  onRestore={(shelf) => setAction({ kind: "restore", shelf })}
                  selected={stillThere}
                  onSelect={setSelected}
                  checked={checked}
                  onCheck={check}
                  sort={sort}
                  onSort={onSort}
                />
              </div>
              {stillThere ? (
                <Detail id={stillThere} revision={inv.revision} bundles={inv.bundles} onManage={setAction} />
              ) : (
                <aside className="detail detail-empty">
                  <p>Select a skill to see where it is mounted, who owns it, and what it says.</p>
                  <p className="sub">Arrow keys move the selection. Press / to search.</p>
                </aside>
              )}
            </div>
          </>
        )}
        {tab === "usage" && <UsagePanel suggestions={suggestions} confirmed={inv.aliases} onPick={showSkill} />}
        {tab === "shelf" && <ShelfPanel shelf={inv.shelf} onManage={setAction} />}
        {tab === "bundles" && <BundlesPanel bundles={inv.bundles} onPick={showBundle} onManage={setAction} />}
        {action && <ActionDialog request={action} onClose={closeAction} onApplied={action.kind === "deleteBodies" ? clearChecked : undefined} />}
        {tab === "conflicts" && <ConflictsPanel problems={inv.problems} onPick={showSkill} />}
        {tab === "broken" && <BrokenPanel problems={inv.problems} />}

        <footer className="status">
          <span>
            {inv.rows.length} skills in {inv.bundles.length} bundles
          </span>
          <span>
            scanned in {inv.scanMs} ms{firstLoadMs !== null && `, first load ${firstLoadMs} ms`}
          </span>
          <span>
            {inv.trackingSince.length === 0
              ? "usage: no data yet"
              : `usage tracked for ${inv.trackingSince.map((t) => t.harness.label).join(", ")}`}
          </span>
          {error && <span className="error">{error}</span>}
        </footer>
      </div>
    </HueProvider>
  );
}
