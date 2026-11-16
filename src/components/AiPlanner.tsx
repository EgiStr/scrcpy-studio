// Describe what to test; the AI proposes a plan from the QA skill's test sets and new goal
// scenarios. Nothing runs until the tester approves it.
import { useState } from "react";
import { aiPlan, errMessage, qaSubmit, type Plan, type PlanItem } from "../lib/ipc";
import { Button } from "./ui";

const INPUT =
  "min-w-0 rounded-md border border-zinc-700 bg-zinc-900 px-3 py-2 text-sm text-zinc-100 outline-none focus:border-emerald-500 placeholder:text-zinc-600";

const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

function title(it: PlanItem) {
  if (it.kind === "testset") return `${fileName(it.testset ?? "")}${it.only ? ` · ${it.only}` : " · all cases"}`;
  return `${it.id} · ${it.topic ?? ""}`;
}

export function AiPlanner({ onQueued }: { onQueued: (runId: string) => void }) {
  const [objective, setObjective] = useState("");
  const [plan, setPlan] = useState<Plan>();
  const [keep, setKeep] = useState<boolean[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  async function makePlan() {
    setBusy(true);
    setError(undefined);
    try {
      const p = await aiPlan(objective);
      setPlan(p);
      setKeep(p.items.map(() => true));
    } catch (e) {
      setError(errMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function approve() {
    if (!plan) return;
    const scenarios = plan.items
      .filter((_, i) => keep[i])
      .map((it) =>
        it.kind === "testset" ? { testset: it.testset ?? "", only: it.only ?? undefined } : { testset: "", goal: it },
      );
    try {
      onQueued(await qaSubmit(scenarios));
      setPlan(undefined);
      setObjective("");
    } catch (e) {
      setError(errMessage(e));
    }
  }

  const kept = keep.filter(Boolean).length;

  if (plan) {
    return (
      <div>
        <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
          <span className="text-sm text-zinc-300">
            Review the plan: {plan.items.length} scenarios
            {plan.costUsd != null && <span className="text-zinc-500"> · planning cost ${plan.costUsd.toFixed(3)}</span>}
          </span>
          <div className="flex gap-2">
            <Button variant="ghost" onClick={() => setPlan(undefined)}>
              Back
            </Button>
            <Button variant="primary" disabled={kept === 0} onClick={approve}>
              Run {kept} {kept === 1 ? "scenario" : "scenarios"}
            </Button>
          </div>
        </div>
        {error && <p className="mb-3 whitespace-pre-wrap text-xs text-red-400">{error}</p>}
        <ul className="flex flex-col gap-2">
          {plan.items.map((it, i) => (
            <li key={i}>
              <label className="flex cursor-pointer gap-3 rounded-md border border-zinc-800 p-3 hover:border-zinc-700">
                <input
                  type="checkbox"
                  className="mt-1"
                  checked={keep[i] ?? false}
                  onChange={(e) => setKeep(keep.map((k, j) => (j === i ? e.target.checked : k)))}
                />
                <div className="min-w-0 text-sm">
                  <div className="flex items-center gap-2">
                    <span
                      className={
                        "rounded px-1.5 py-0.5 text-[10px] font-semibold uppercase " +
                        (it.kind === "goal" ? "bg-sky-500/20 text-sky-300" : "bg-zinc-800 text-zinc-300")
                      }
                    >
                      {it.kind === "goal" ? "AI drives" : "scripted"}
                    </span>
                    <span className="text-zinc-200">{title(it)}</span>
                  </div>
                  {it.goal && <p className="mt-1 text-zinc-300">{it.goal}</p>}
                  {it.keypoints.length > 0 && (
                    <p className="mt-1 font-mono text-xs text-zinc-500">must show: {it.keypoints.join(" · ")}</p>
                  )}
                  {it.why && <p className="mt-1 text-xs text-zinc-500">{it.why}</p>}
                </div>
              </label>
            </li>
          ))}
        </ul>
        {plan.rejected.length > 0 && (
          <div className="mt-3 text-xs text-amber-300">
            Left out of the plan:
            <ul className="ml-4 list-disc">
              {plan.rejected.map((r) => (
                <li key={r}>{r}</li>
              ))}
            </ul>
          </div>
        )}
        <p className="mt-3 text-xs text-zinc-500">
          Goal scenarios drive a real phone with the QA skill's screen, input and record tools only. Buying, paying,
          sending, typing secrets and coordinate taps are refused, and the verdict comes from the recorded evidence.
        </p>
      </div>
    );
  }

  return (
    <div>
      <textarea
        className={INPUT + " h-24 w-full resize-y"}
        placeholder="Example: Check that search finds 10 GB internet packages with their price, and that the Roaming menu lists packages for Japan."
        value={objective}
        onChange={(e) => setObjective(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && objective.trim() && !busy) makePlan();
        }}
      />
      <div className="mt-2 flex items-center gap-3">
        <Button variant="primary" disabled={busy || !objective.trim()} onClick={makePlan}>
          {busy ? "Planning…" : "Make a plan"}
        </Button>
        <span className="text-xs text-zinc-500">
          {busy ? "The AI is reading the test sets; this can take a minute or two." : "Ctrl+Enter · you review the plan before anything runs"}
        </span>
      </div>
      {error && <p className="mt-3 whitespace-pre-wrap text-xs text-red-400">{error}</p>}
    </div>
  );
}
