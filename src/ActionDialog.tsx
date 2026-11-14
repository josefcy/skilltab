import { useEffect, useRef, useState } from "react";
import { commands, type ActionRequest, type ApplyReport, type CoreError, type PlanView } from "./bindings";
import { HarnessChip } from "./ui";

type Phase =
  | { kind: "planning"; notice?: string[] }
  | { kind: "plan"; plan: PlanView; notice?: string[] }
  | { kind: "applying"; plan: PlanView }
  | { kind: "done"; plan: PlanView; report: ApplyReport }
  | { kind: "error"; message: string };

function errorText(e: CoreError): string {
  return e.kind === "unknownId" ? "That item is no longer installed. Rescan and try again." : e.message;
}

const RESULT_LABEL = { done: "done", skipped: "already done", failed: "failed", notRun: "not run" } as const;

/** The one confirm flow for every change. It plans, shows what will happen, and is the only caller of applyPlan. */
export function ActionDialog({ request, onClose, onApplied }: { request: ActionRequest; onClose: () => void; onApplied?: () => void }) {
  const [phase, setPhase] = useState<Phase>({ kind: "planning" });
  const confirmRef = useRef<HTMLButtonElement | null>(null);

  const plan = (notice?: string[]) => {
    setPhase({ kind: "planning", notice });
    commands
      .planAction(request)
      .then((r) => setPhase(r.status === "ok" ? { kind: "plan", plan: r.data, notice } : { kind: "error", message: errorText(r.error) }))
      .catch((e) => setPhase({ kind: "error", message: String(e) }));
  };

  useEffect(() => plan(), [request]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && phase.kind !== "applying") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [phase.kind, onClose]);

  useEffect(() => {
    if (phase.kind === "plan" && phase.plan.ready) confirmRef.current?.focus();
  }, [phase]);

  const apply = (p: PlanView) => {
    setPhase({ kind: "applying", plan: p });
    commands
      .applyPlan(p.id)
      .then((r) => {
        if (r.status === "error") return setPhase({ kind: "error", message: errorText(r.error) });
        if (r.data.outcome === "stale") return plan(r.data.reasons);
        if (r.data.outcome === "applied") onApplied?.();
        setPhase({ kind: "done", plan: p, report: r.data });
      })
      .catch((e) => setPhase({ kind: "error", message: String(e) }));
  };

  const shown = phase.kind === "plan" || phase.kind === "applying" || phase.kind === "done" ? phase.plan : null;

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && phase.kind !== "applying" && onClose()}>
      <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="action-title">
        <header>
          <h2 id="action-title">{shown?.title ?? "Planning…"}</h2>
        </header>

        {(phase.kind === "planning" || phase.kind === "plan") && phase.notice && (
          <div className="notice">
            <p>The disk changed since that plan was made, so nothing ran and it was planned again.</p>
            {phase.notice.map((n) => (
              <p key={n} className="sub">
                {n}
              </p>
            ))}
          </div>
        )}
        {phase.kind === "error" && <p className="error">{phase.message}</p>}

        {shown && phase.kind !== "done" && <PlanBody plan={shown} />}
        {phase.kind === "done" && <ReportBody report={phase.report} />}

        <footer>
          {phase.kind === "done" || phase.kind === "error" ? (
            <button type="button" className="primary" onClick={onClose}>
              Close
            </button>
          ) : (
            <>
              <button type="button" onClick={onClose} disabled={phase.kind === "applying"}>
                Cancel
              </button>
              <button
                type="button"
                ref={confirmRef}
                className={shown?.blockers.length === 0 && shown.steps.some((s) => s.kind === "recycle") ? "primary danger" : "primary"}
                disabled={phase.kind !== "plan" || !phase.plan.ready}
                onClick={() => phase.kind === "plan" && apply(phase.plan)}
                title={shown && !shown.ready ? (shown.blockers.length > 0 ? "Blocked. See the reasons above." : "Nothing to change.") : undefined}
              >
                {phase.kind === "applying" ? "Working…" : (shown?.confirm ?? "Confirm")}
              </button>
            </>
          )}
        </footer>
      </div>
    </div>
  );
}

function PlanBody({ plan }: { plan: PlanView }) {
  return (
    <div className="dialog-body">
      {plan.blockers.length > 0 && (
        <section className="blockers" aria-label="Blocked">
          <h3>Blocked</h3>
          {plan.blockers.map((b) => (
            <div key={b.kind + b.reason} className="blocker">
              <p>{b.reason}</p>
              <p className="remedy">
                <b>Instead.</b> {b.remedy}
              </p>
            </div>
          ))}
        </section>
      )}

      {plan.skipped.length > 0 && (
        <section className="blockers" aria-label="Skipped">
          <h3>Skipped, can't be deleted here</h3>
          {plan.skipped.map((s, i) => (
            <div key={i} className="blocker">
              <p>
                <b>{s.name}.</b> {s.reason}
              </p>
              <p className="remedy">
                <b>Instead.</b> {s.remedy}
              </p>
            </div>
          ))}
        </section>
      )}

      {plan.steps.length === 0 && plan.blockers.length === 0 && plan.skipped.length === 0 && <p className="sub">Nothing to change. It is already in that state.</p>}

      {plan.steps.length > 0 && (
        <section>
          <h3>{plan.blockers.length > 0 ? "Would do" : "Will do"}</h3>
          <ol className="steps">
            {plan.steps.map((s, i) => (
              <li key={i}>
                <span>{s.text}</span>
                <code>{s.detail}</code>
              </li>
            ))}
          </ol>
        </section>
      )}

      {plan.effects.length > 0 && (
        <section>
          <h3>What changes</h3>
          <ul className="effects">
            {plan.effects.map((e, i) => (
              <li key={i}>
                {e.harness && <HarnessChip tag={e.harness} />}
                <span>{e.text}</span>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}

function ReportBody({ report }: { report: ApplyReport }) {
  const headline =
    report.outcome === "applied" ? "Done." : report.outcome === "failed" ? "A step failed. The steps after it did not run." : "Refused. Nothing ran.";
  return (
    <div className="dialog-body">
      <p className={report.outcome === "applied" ? "ok" : "error"}>{headline}</p>
      <ol className="steps">
        {report.steps.map((s, i) => (
          <li key={i} className={`result-${s.result}`}>
            <span>
              {s.text} <span className="result">{RESULT_LABEL[s.result]}</span>
            </span>
            <code>{s.detail}</code>
            {s.message && s.result === "failed" && <span className="error">{s.message}</span>}
          </li>
        ))}
      </ol>
      {report.output.filter((o) => o).map((o, i) => (
        <pre key={i} className="output">
          {o}
        </pre>
      ))}
    </div>
  );
}
