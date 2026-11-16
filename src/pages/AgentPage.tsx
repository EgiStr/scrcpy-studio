// AI Agent: a natural-language objective becomes a test plan the user reviews. Approved
// scenarios run as QA jobs: existing test sets run scripted, goals are driven by the AI.
import { useEffect, useState } from "react";
import {
  aiPlan,
  errMessage,
  qaConfigGet,
  qaConfigSet,
  qaSubmit,
  type Plan,
  type PlanItem,
  type QaConfig,
} from "../lib/ipc";
import { Button, PanelTitle } from "../components/ui";

const INPUT =
  "min-w-0 rounded-md border border-zinc-700 bg-zinc-900 px-2 py-1.5 text-sm text-zinc-100 outline-none focus:border-emerald-500";

const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

function title(it: PlanItem) {
  if (it.kind === "testset") return `${fileName(it.testset ?? "")}${it.only ? ` · ${it.only}` : " · all cases"}`;
  return `${it.id} · ${it.topic ?? ""}`;
}

export function AgentPage({ onMonitor }: { onMonitor: () => void }) {
  const [config, setConfig] = useState<QaConfig>();
  const [objective, setObjective] = useState("");
  const [plan, setPlan] = useState<Plan>();
  const [keep, setKeep] = useState<boolean[]>([]);
  const [busy, setBusy] = useState(false);
  const [queued, setQueued] = useState<string>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    qaConfigGet().then(setConfig);
  }, []);

  async function makePlan() {
    setBusy(true);
    setError(undefined);
    setQueued(undefined);
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
        it.kind === "testset"
          ? { testset: it.testset ?? "", only: it.only ?? undefined }
          : { testset: "", goal: it },
      );
    try {
      setQueued(await qaSubmit(scenarios));
      setPlan(undefined);
    } catch (e) {
      setError(errMessage(e));
    }
  }

  const kept = keep.filter(Boolean).length;

  return (
    <div className="h-full overflow-y-auto p-6">
      <PanelTitle
        title="AI Agent"
        desc="Describe what to test. The AI proposes a plan from the QA skill's test sets and new goal scenarios; nothing runs until you approve it."
      />

      {config && (
        <div className="mb-4 flex flex-wrap items-end gap-2 text-xs text-zinc-400">
          <label className="flex min-w-[14rem] flex-1 flex-col gap-1">
            AI command (Claude Code CLI)
            <input className={INPUT} value={config.aiCommand} onChange={(e) => setConfig({ ...config, aiCommand: e.target.value })} />
          </label>
          <label className="flex w-48 flex-col gap-1">
            Model
            <input className={INPUT} value={config.aiModel} onChange={(e) => setConfig({ ...config, aiModel: e.target.value })} />
          </label>
          <label className="flex w-28 flex-col gap-1">
            Max actions
            <input
              className={INPUT}
              type="number"
              min={5}
              value={config.aiMaxActions}
              onChange={(e) => setConfig({ ...config, aiMaxActions: Number(e.target.value) || 80 })}
            />
          </label>
          <Button onClick={() => qaConfigSet(config).catch((e) => setError(errMessage(e)))}>Save</Button>
        </div>
      )}

      <textarea
        className={INPUT + " mb-2 h-28 w-full resize-y"}
        placeholder="Example: Check that search finds 10 GB internet packages with their price, and that the Roaming menu lists packages for Japan."
        value={objective}
        onChange={(e) => setObjective(e.target.value)}
      />
      <div className="mb-4 flex items-center gap-3">
        <Button variant="primary" disabled={busy || !objective.trim()} onClick={makePlan}>
          {busy ? "Planning…" : "Generate plan"}
        </Button>
        {busy && <span className="text-xs text-zinc-500">The AI is reading the test sets; this can take a minute or two.</span>}
      </div>

      {error && <p className="mb-3 whitespace-pre-wrap text-xs text-red-400">{error}</p>}
      {queued && (
        <p className="mb-3 flex items-center gap-3 text-sm text-emerald-300">
          Queued as {queued}.
          <Button variant="ghost" onClick={onMonitor}>
            Watch it in Monitor
          </Button>
        </p>
      )}

      {plan && (
        <div className="rounded-md border border-zinc-800 bg-zinc-950 p-3">
          <div className="mb-2 flex items-center justify-between">
            <span className="text-sm font-medium text-zinc-200">
              Plan · {plan.items.length} scenarios{plan.costUsd != null ? ` · planning cost $${plan.costUsd.toFixed(3)}` : ""}
            </span>
            <Button variant="primary" disabled={kept === 0} onClick={approve}>
              Approve and run {kept}
            </Button>
          </div>
          <ul className="flex flex-col gap-2">
            {plan.items.map((it, i) => (
              <li key={i} className="flex gap-3 rounded border border-zinc-800 p-2">
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
            Goal scenarios drive a real phone. The agent can only use the QA skill's screen, input and record tools on its
            leased phone; buying, paying, sending, typing secrets and coordinate taps are refused, and the verdict comes from
            the evidence it records.
          </p>
        </div>
      )}
    </div>
  );
}
