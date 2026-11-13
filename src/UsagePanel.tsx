import { useState } from "react";
import { commands, type AliasPick, type AliasSuggestionView, type AliasView, type CoreError, type RefreshReport } from "./bindings";
import { Empty, HarnessChip, plural } from "./ui";

type Props = {
  /** undefined while loading. */
  suggestions: AliasSuggestionView[] | undefined;
  confirmed: AliasView[];
  onPick: (skillId: string) => void;
};

function errorText(e: CoreError): string {
  return e.kind === "unknownId" ? "That key or skill is no longer there. Rescan and try again." : e.message;
}

const pick = (s: AliasSuggestionView): AliasPick => ({ harness: s.harness.id, from: s.from, to: s.to });

/**
 * Usage under names that no installed skill answers to any more, such as a plugin's old prefix.
 * Counting one under an installed skill is a click each, and nothing here links anything on its own.
 */
export function UsagePanel({ suggestions, confirmed, onPick }: Props) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // The backend refreshes and the new revision reloads the lists, so success needs no state change here.
  const run = (key: string, call: () => Promise<{ status: "ok"; data: RefreshReport } | { status: "error"; error: CoreError }>) => {
    setBusy(key);
    setError(null);
    call()
      .then((r) => r.status === "error" && setError(errorText(r.error)))
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(null));
  };

  if (suggestions === undefined) return <Empty title="Reading usage history…" />;

  const sorted = [...suggestions].sort((a, b) => b.events - a.events || a.fromLabel.localeCompare(b.fromLabel));
  const total = sorted.reduce((n, s) => n + s.events, 0);

  return (
    <div className="panel-list">
      <section className="conflict">
        <h3>Usage history under old names</h3>
        <p className="sub jline">
          Your transcripts mention skills by names that nothing installed answers to now. Each suggestion below matches exactly one installed skill. Confirming
          counts that history under the skill. Undo splits it off again. Nothing is linked until you click.
        </p>
        <div className="jactions">
          <button
            type="button"
            className="manage"
            disabled={sorted.length === 0 || busy !== null}
            onClick={() => run("all", () => commands.confirmAliases(sorted.map(pick)))}
          >
            {busy === "all" ? "Confirming…" : `Confirm all (${plural(total, "event")})`}
          </button>
          <span className="sub">{sorted.length === 0 ? "No suggestions." : `${plural(sorted.length, "suggestion")} in the list.`}</span>
        </div>
        {error && <p className="error">{error}</p>}
      </section>

      {sorted.length === 0 && confirmed.length === 0 && (
        <Empty title="Nothing to match">Every skill name in your transcripts is either installed or has no single installed match.</Empty>
      )}

      {sorted.length > 0 && (
        <section className="conflict">
          <h3>
            Suggestions <span className="sub">most events first</span>
          </h3>
          <ul className="alist">
            {sorted.map((s) => (
              <li key={s.harness.id + s.from} className="arow">
                <span className="num aevents">
                  {s.events}
                  <span className="sub"> {s.events === 1 ? "event" : "events"}</span>
                </span>
                <span className="akeys">
                  <code>{s.fromLabel}</code>
                  <span className="ato" aria-label="counts under">
                    →
                  </span>
                  <button type="button" className="textlink" onClick={() => onPick(s.targetBody)} title="Show this skill in the inventory">
                    {s.toLabel}
                  </button>
                </span>
                <HarnessChip tag={s.harness} />
                <span className="sub areason" title={s.rule}>
                  {s.reason}
                </span>
                <button type="button" className="rowaction" disabled={busy !== null} onClick={() => run(s.harness.id + s.from, () => commands.confirmAlias(s.harness.id, s.from, s.to))}>
                  {busy === s.harness.id + s.from ? "Confirming…" : "Confirm"}
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}

      {confirmed.length > 0 && (
        <section className="conflict">
          <h3>
            Confirmed <span className="sub">{plural(confirmed.length, "link")}</span>
          </h3>
          <ul className="alist">
            {confirmed.map((a) => (
              <li key={a.harness.id + a.from} className="arow confirmed">
                <span />
                <span className="akeys">
                  <code>{a.fromLabel}</code>
                  <span className="ato" aria-label="counts under">
                    →
                  </span>
                  {a.targetBody ? (
                    <button type="button" className="textlink" onClick={() => onPick(a.targetBody!)} title="Show this skill in the inventory">
                      {a.toLabel}
                    </button>
                  ) : (
                    <>
                      <code>{a.toLabel}</code>
                      <span className="state warn">no longer installed</span>
                    </>
                  )}
                </span>
                <HarnessChip tag={a.harness} />
                <span />
                <button type="button" className="rowaction" disabled={busy !== null} onClick={() => run("undo" + a.harness.id + a.from, () => commands.removeAlias(a.harness.id, a.from))}>
                  {busy === "undo" + a.harness.id + a.from ? "Undoing…" : "Undo"}
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
