import { createContext, useContext, type CSSProperties } from "react";
import type { Facets, JstackRelationView, Tag } from "./bindings";

/** Hue per harness, assigned by position in the facet list the backend sends, so a new harness gets a colour without a TS edit. */
const HueContext = createContext<(id: string) => number>(() => 210);

export function hueMap(facets: Facets): (id: string) => number {
  const ids = facets.harnesses.map((h) => h.id);
  return (id) => {
    const i = Math.max(0, ids.indexOf(id));
    return (i * 137 + 205) % 360;
  };
}

export const HueProvider = HueContext.Provider;

export function HarnessChip({ tag }: { tag: Tag }) {
  const hue = useContext(HueContext)(tag.id);
  return (
    <span className="chip" style={{ "--h": hue } as CSSProperties}>
      {tag.label}
    </span>
  );
}

export function Empty({ title, children }: { title: string; children?: React.ReactNode }) {
  return (
    <div className="empty">
      <p className="empty-title">{title}</p>
      {children && <p className="empty-body">{children}</p>}
    </div>
  );
}

export function plural(n: number, one: string, many = one + "s"): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** How a skill relates to the jstack repo, with its drift when it has any. The full sentence is the tooltip. */
export function JstackBadge({ j }: { j: JstackRelationView }) {
  return (
    <span className="jbadges" title={j.note}>
      <span className="jbadge">jstack {j.label}</span>
      {j.driftLabel && <span className="state warn">{j.driftLabel}</span>}
    </span>
  );
}
