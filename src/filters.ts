import type { SkillRow } from "./bindings";

export type SortKey = "name" | "description" | "harnesses" | "scopes" | "owner" | "origin" | "uses" | "lastUsed";
export type Sort = { key: SortKey; dir: "asc" | "desc" };

export type Filters = {
  text: string;
  harnesses: string[];
  scopeKind: string;
  bundle: string;
  owner: string;
  origin: string;
  unused: boolean;
  problems: boolean;
};

export const NO_FILTERS: Filters = {
  text: "",
  harnesses: [],
  scopeKind: "",
  bundle: "",
  owner: "",
  origin: "",
  unused: false,
  problems: false,
};

export function activeFilterCount(f: Filters): number {
  return (
    (f.text ? 1 : 0) +
    (f.harnesses.length ? 1 : 0) +
    (f.scopeKind ? 1 : 0) +
    (f.bundle ? 1 : 0) +
    (f.owner ? 1 : 0) +
    (f.origin ? 1 : 0) +
    (f.unused ? 1 : 0) +
    (f.problems ? 1 : 0)
  );
}

function haystack(r: SkillRow): string {
  return [
    r.name,
    r.description ?? "",
    r.origin.label,
    r.origin.detail ?? "",
    ...r.harnesses.map((h) => h.label),
    ...r.scopes.map((s) => s.label),
    ...r.owners.map((o) => o.label),
  ]
    .join("\n")
    .toLowerCase();
}

export function matches(r: SkillRow, f: Filters): boolean {
  if (f.harnesses.length && !r.harnesses.some((h) => f.harnesses.includes(h.id))) return false;
  if (f.scopeKind && !r.scopes.some((s) => s.kind === f.scopeKind)) return false;
  if (f.bundle && !r.bundles.includes(f.bundle)) return false;
  if (f.owner && !r.owners.some((o) => o.id === f.owner)) return false;
  if (f.origin && r.origin.id !== f.origin) return false;
  if (f.unused && !r.usage.unused) return false;
  if (f.problems && r.flags.length === 0) return false;
  const terms = f.text.toLowerCase().split(/\s+/).filter(Boolean);
  if (terms.length) {
    const h = haystack(r);
    if (!terms.every((t) => h.includes(t))) return false;
  }
  return true;
}

const text = (a: string, b: string) => a.localeCompare(b, undefined, { sensitivity: "base", numeric: true });

function compare(a: SkillRow, b: SkillRow, key: SortKey): number {
  switch (key) {
    case "name":
      return text(a.name, b.name);
    case "description":
      return text(a.description ?? "", b.description ?? "");
    case "harnesses":
      return text(a.harnesses.map((h) => h.label).join(), b.harnesses.map((h) => h.label).join());
    case "scopes":
      return text(a.scopes.map((s) => s.label).join(), b.scopes.map((s) => s.label).join());
    case "owner":
      return text(a.owners.map((o) => o.label).join(), b.owners.map((o) => o.label).join());
    case "origin":
      return text(a.origin.label + (a.origin.detail ?? ""), b.origin.label + (b.origin.detail ?? ""));
    case "uses":
      return (a.usage.tracked ? a.usage.total : -1) - (b.usage.tracked ? b.usage.total : -1);
    case "lastUsed":
      return (a.usage.lastUsed ?? "").localeCompare(b.usage.lastUsed ?? "");
  }
}

export function filterAndSort(rows: SkillRow[], f: Filters, sort: Sort): SkillRow[] {
  const sign = sort.dir === "asc" ? 1 : -1;
  return rows
    .filter((r) => matches(r, f))
    .sort((a, b) => sign * compare(a, b, sort.key) || text(a.name, b.name) || a.id.localeCompare(b.id));
}
