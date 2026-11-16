// Findings of one run: each case that did not pass with its failure category, how cases
// moved since earlier runs, a redacted report export, and AI hypotheses on request.
import { useEffect, useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import {
  errMessage,
  qaExport,
  qaFindings,
  qaInvestigate,
  type CaseChange,
  type DefectCategory,
  type Finding,
  type Investigation,
} from "../lib/ipc";
import { Button } from "./ui";

const LABEL: Record<DefectCategory, string> = {
  "ui-defect": "UI defect",
  "navigation-defect": "Navigation defect",
  "business-rule-defect": "Business-rule defect",
  "app-crash": "App crash",
  "automation-error": "Automation error",
  "infrastructure-issue": "Device / infrastructure",
  inconclusive: "Inconclusive",
};

const PRODUCT: DefectCategory[] = ["ui-defect", "navigation-defect", "business-rule-defect", "app-crash"];

const CHANGE_STYLE: Record<string, string> = {
  regressed: "text-red-300",
  fixed: "text-emerald-300",
};

/** Findings of a finished run; loaded when the run detail opens. */
export function RunFindings({ runId }: { runId: string }) {
  const [data, setData] = useState<{ findings: Finding[]; changes: CaseChange[] }>();
  const [busy, setBusy] = useState<string>();
  const [error, setError] = useState<string>();
  const [inv, setInv] = useState<Record<string, Investigation>>({});

  useEffect(() => {
    qaFindings(runId)
      .then(setData)
      .catch((e) => setError(errMessage(e)));
  }, [runId]);

  async function act<T>(label: string, f: () => Promise<T>) {
    setBusy(label);
    setError(undefined);
    try {
      return await f();
    } catch (e) {
      setError(errMessage(e));
    } finally {
      setBusy(undefined);
    }
  }

  async function investigate(f: Finding) {
    const r = await act(f.id, () => qaInvestigate(f.jobId, f.caseId));
    if (r) setInv((m) => ({ ...m, [f.id]: r }));
  }

  const moved = data?.changes.filter((c) => c.change === "regressed" || c.change === "fixed") ?? [];
  const product = data?.findings.filter((f) => PRODUCT.includes(f.category)).length ?? 0;

  return (
    <div>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h4 className="text-sm font-medium text-zinc-200">
          Findings
          {data && (
            <span className="ml-2 font-normal text-zinc-500">
              {data.findings.length === 0
                ? "none"
                : `${product} product ${product === 1 ? "defect" : "defects"} · ${data.findings.length - product} other`}
            </span>
          )}
        </h4>
        <Button
          variant="ghost"
          disabled={!!busy}
          onClick={() => act("export", () => qaExport(runId).then((p) => openPath(p)))}
        >
          {busy === "export" ? "Exporting…" : "Export defect report"}
        </Button>
      </div>
      {error && <p className="mt-2 text-xs text-red-400">{error}</p>}
      {!data && !error && <p className="mt-2 text-xs text-zinc-500">Loading…</p>}

      {data && (
        <div className="mt-2 flex flex-col gap-3 text-sm">
          {data.findings.length === 0 ? (
            <p className="text-xs text-emerald-300">Every graded case of this run passed.</p>
          ) : (
            data.findings.map((f) => (
              <div key={f.id} className="rounded-md border border-zinc-800 p-3">
                <div className="flex flex-wrap items-center gap-2">
                  <span
                    className={
                      "rounded px-2 py-0.5 text-[11px] font-semibold " +
                      (PRODUCT.includes(f.category) ? "bg-red-500/20 text-red-300" : "bg-zinc-700 text-zinc-300")
                    }
                    title={f.reason}
                  >
                    {LABEL[f.category]}
                  </span>
                  <span className="font-mono text-xs text-zinc-500">
                    J{f.jobId} {f.caseId} · {f.verdict}
                  </span>
                  <span className="text-zinc-200">{f.summary}</span>
                </div>
                {f.missed.length > 0 && (
                  <p className="mt-1 text-xs text-zinc-400">Missing: {f.missed.join(", ")}</p>
                )}
                {f.actual && <p className="mt-1 line-clamp-3 text-xs text-zinc-500">Seen: {f.actual}</p>}
                <div className="mt-2 flex gap-2">
                  <Button variant="ghost" disabled={!!busy} onClick={() => investigate(f)}>
                    {busy === f.id ? "Investigating… (about a minute)" : "Ask AI why"}
                  </Button>
                  {f.evidenceDir && (
                    <Button variant="ghost" onClick={() => openPath(f.evidenceDir!).catch((e) => setError(errMessage(e)))}>
                      Evidence
                    </Button>
                  )}
                </div>
                {inv[f.id] && <InvestigationView inv={inv[f.id]} />}
              </div>
            ))
          )}
          {moved.length > 0 && (
            <div>
              <p className="text-xs font-semibold text-zinc-300">Compared with earlier runs</p>
              <ul className="mt-1 font-mono text-xs">
                {moved.map((c) => (
                  <li key={c.jobId + c.key} className={CHANGE_STYLE[c.change]}>
                    {c.change} · J{c.jobId} {c.caseId}: {c.now} (was {c.before} in {c.beforeRun})
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function InvestigationView({ inv }: { inv: Investigation }) {
  return (
    <div className="mt-2 rounded border border-amber-500/30 bg-amber-500/5 p-2 text-xs">
      <p className="text-amber-300">{inv.note}</p>
      <ol className="mt-1 flex list-decimal flex-col gap-1 pl-4 text-zinc-300">
        {inv.hypotheses.map((h, i) => (
          <li key={i}>
            <span className="text-zinc-100">{h.cause}</span>{" "}
            <span className="text-zinc-500">({Math.round(h.confidence * 100)}% confidence)</span>
            {h.supporting.length > 0 && <p className="text-emerald-300/80">For: {h.supporting.join("; ")}</p>}
            {h.against.length > 0 && <p className="text-red-300/80">Against: {h.against.join("; ")}</p>}
          </li>
        ))}
      </ol>
      {inv.limitations.length > 0 && <p className="mt-1 text-zinc-500">Limits: {inv.limitations.join("; ")}</p>}
      {inv.nextSteps.length > 0 && <p className="text-zinc-400">Next: {inv.nextSteps.join("; ")}</p>}
      <p className="mt-1 text-zinc-600">
        {inv.model}
        {inv.costUsd != null && ` · $${inv.costUsd.toFixed(2)}`}
      </p>
    </div>
  );
}
