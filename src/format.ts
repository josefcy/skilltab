const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

export function ago(iso: string, now = Date.now()): string {
  const d = now - Date.parse(iso);
  if (d < HOUR) return `${Math.max(1, Math.round(d / MINUTE))}m ago`;
  if (d < DAY) return `${Math.round(d / HOUR)}h ago`;
  if (d < 60 * DAY) return `${Math.round(d / DAY)}d ago`;
  if (d < 730 * DAY) return `${Math.round(d / (30 * DAY))}mo ago`;
  return `${Math.round(d / (365 * DAY))}y ago`;
}

export function date(iso: string): string {
  return new Date(iso).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}
