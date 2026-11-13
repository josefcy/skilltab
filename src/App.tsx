import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Detail } from "./Detail";
import { BrokenPanel, BundlesPanel, ConflictsPanel } from "./Panels";
import { SkillTable } from "./SkillTable";
import { Toolbar } from "./Toolbar";
import { filterAndSort, NO_FILTERS, type Filters, type Sort, type SortKey } from "./filters";
import { hueMap, HueProvider } from "./ui";
import { useInventory } from "./useInventory";

type Tab = "skills" | "bundles" | "conflicts" | "broken";

export default function App() {
  const { inventory: inv, error, firstLoadMs, refresh, refreshing } = useInventory();
  const [tab, setTab] = useState<Tab>("skills");
  const [filters, setFilters] = useState<Filters>(NO_FILTERS);
  const [sort, setSort] = useState<Sort>({ key: "name", dir: "asc" });
  const [selected, setSelected] = useState<string | null>(null);
  const searchRef = useRef<HTMLInputElement | null>(null);

  const rows = useMemo(() => (inv ? filterAndSort(inv.rows, filters, sort) : []), [inv, filters, sort]);
  const hue = useMemo(() => (inv ? hueMap(inv.facets) : () => 210), [inv]);

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
      const typing = e.target instanceof HTMLElement && ["INPUT", "SELECT", "TEXTAREA"].includes(e.target.tagName);
      if (e.key === "/" && !typing) {
        e.preventDefault();
        setTab("skills");
        searchRef.current?.focus();
        searchRef.current?.select();
      } else if (e.key === "Escape") {
        if (typing && filters.text) setFilters((f) => ({ ...f, text: "" }));
        else if (e.target instanceof HTMLElement) e.target.blur();
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
  }, [tab, step, filters.text]);

  if (!inv) {
    return <main className="boot">{error ? <p className="error">Could not read the inventory: {error}</p> : <p className="sub">Scanning skills…</p>}</main>;
  }

  const stillThere = selected && inv.rows.some((r) => r.id === selected) ? selected : null;
  const tabs: { id: Tab; label: string; n: number }[] = [
    { id: "skills", label: "Skills", n: inv.rows.length },
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
              <SkillTable rows={rows} selected={stillThere} onSelect={setSelected} sort={sort} onSort={onSort} />
              {stillThere ? (
                <Detail id={stillThere} revision={inv.revision} />
              ) : (
                <aside className="detail detail-empty">
                  <p>Select a skill to see where it is mounted, who owns it, and what it says.</p>
                  <p className="sub">Arrow keys move the selection. Press / to search.</p>
                </aside>
              )}
            </div>
          </>
        )}
        {tab === "bundles" && <BundlesPanel bundles={inv.bundles} onPick={showBundle} />}
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
