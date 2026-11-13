import type { RefObject } from "react";
import type { FacetValue, Inventory } from "./bindings";
import { activeFilterCount, NO_FILTERS, type Filters } from "./filters";

type Props = {
  inv: Inventory;
  filters: Filters;
  /** Merges into the current filters, so two quick changes both land. */
  onChange: (patch: Partial<Filters>) => void;
  shown: number;
  searchRef: RefObject<HTMLInputElement | null>;
};

function Select({
  label,
  value,
  values,
  onChange,
}: {
  label: string;
  value: string;
  values: FacetValue[];
  onChange: (v: string) => void;
}) {
  return (
    <label className="select">
      <span className="select-label">{label}</span>
      <select value={value} onChange={(e) => onChange(e.target.value)} data-active={value !== ""}>
        <option value="">any</option>
        {values.map((v) => (
          <option key={v.id} value={v.id}>
            {v.label} ({v.count})
          </option>
        ))}
      </select>
    </label>
  );
}

export function Toolbar({ inv, filters, onChange, shown, searchRef }: Props) {
  const set = onChange;
  const toggleHarness = (id: string) =>
    set({ harnesses: filters.harnesses.includes(id) ? filters.harnesses.filter((h) => h !== id) : [...filters.harnesses, id] });

  const unused = inv.rows.filter((r) => r.usage.unused).length;
  const problems = inv.rows.filter((r) => r.flags.length > 0).length;
  const noUsage = inv.trackingSince.length === 0;
  const active = activeFilterCount(filters);

  return (
    <div className="toolbar">
      <div className="toolbar-row">
        <input
          ref={searchRef}
          className="search"
          type="search"
          placeholder="Search name, description, scope, origin   ( / )"
          value={filters.text}
          onChange={(e) => set({ text: e.target.value })}
          spellCheck={false}
        />
        <div className="seg" role="group" aria-label="Harness">
          {inv.facets.harnesses.map((h) => (
            <button
              key={h.id}
              type="button"
              className="seg-btn"
              aria-pressed={filters.harnesses.includes(h.id)}
              disabled={!h.present}
              title={h.present ? undefined : `${h.label} has nothing installed on this machine`}
              onClick={() => toggleHarness(h.id)}
            >
              {h.label}
              <span className="count">{h.count}</span>
            </button>
          ))}
        </div>
        <button
          type="button"
          className="toggle"
          aria-pressed={filters.unused}
          disabled={noUsage}
          title={noUsage ? "No harness has usage data yet, so nothing can be called unused" : "No uses since tracking began"}
          onClick={() => set({ unused: !filters.unused })}
        >
          Unused<span className="count">{noUsage ? "–" : unused}</span>
        </button>
        <button
          type="button"
          className="toggle"
          aria-pressed={filters.problems}
          title="Shadowed, name conflict, or manifest issue"
          onClick={() => set({ problems: !filters.problems })}
        >
          Problems<span className="count">{problems}</span>
        </button>
      </div>
      <div className="toolbar-row">
        <Select label="Scope" value={filters.scopeKind} values={inv.facets.scopeKinds} onChange={(v) => set({ scopeKind: v })} />
        <Select label="Bundle" value={filters.bundle} values={inv.facets.bundles} onChange={(v) => set({ bundle: v })} />
        <Select label="Owner" value={filters.owner} values={inv.facets.owners} onChange={(v) => set({ owner: v })} />
        <Select label="Origin" value={filters.origin} values={inv.facets.origins} onChange={(v) => set({ origin: v })} />
        <span className="spacer" />
        <span className="shown" aria-live="polite">
          {shown === inv.rows.length ? `${shown} skills` : `${shown} of ${inv.rows.length} skills`}
        </span>
        {active > 0 && (
          <button type="button" className="link" onClick={() => onChange(NO_FILTERS)}>
            Clear filters
          </button>
        )}
      </div>
    </div>
  );
}
