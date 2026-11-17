// Describe what to test, then choose how it runs:
// - Scenario: the AI writes a test set (YAML) that run.py runs with no model in the loop;
//   you review and edit the YAML, it is saved and can be run again from "Pick test sets".
// - Agent: the AI proposes a plan from the existing test sets and new goal scenarios it
//   drives on the phone itself.
// Nothing runs until the tester approves it.
import { useEffect, useState } from "react";
import {
  aiPlan,
  aiScenario,
  errMessage,
  qaScenarioCheck,
  qaScenarioSave,
  qaSubmit,
  type Plan,
  type PlanItem,
  type QaScenario,
  type ScenarioCase,
} from "../lib/ipc";
import { remember, remembered } from "../lib/remember";
import { Button, Tabs } from "./ui";
import { forTarget, type Target } from "./DeviceTarget";

const INPUT =
  "min-w-0 rounded-md border border-zinc-700 bg-zinc-900 px-3 py-2 text-sm text-zinc-100 outline-none focus:border-emerald-500 placeholder:text-zinc-600";

type Mode = "scenario" | "agent";

const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

function title(it: PlanItem) {
  if (it.kind === "testset") return `${fileName(it.testset ?? "")}${it.only ? ` · ${it.only}` : " · all cases"}`;
  return `${it.id} · ${it.topic ?? ""}`;
}

export function AiPlanner({ target, onQueued }: { target: Target; onQueued: (runId: string) => void }) {
  const [mode, setMode] = useState<Mode>(() => remembered("planner.mode", ["scenario", "agent"] as Mode[], "scenario"));
  useEffect(() => {
    remember("planner.mode", mode);
  }, [mode]);
  const [objective, setObjective] = useState("");
  const [plan, setPlan] = useState<Plan>();
  const [keep, setKeep] = useState<boolean[]>([]);
  const [yaml, setYaml] = useState<string>();
  const [cost, setCost] = useState<number | null>();
  const [cases, setCases] = useState<ScenarioCase[]>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  async function write() {
    setBusy(true);
    setError(undefined);
    try {
      if (mode === "agent") {
        const p = await aiPlan(objective);
        setPlan(p);
        setKeep(p.items.map(() => true));
      } else {
        const d = await aiScenario(objective);
        setYaml(d.yaml);
        setCost(d.costUsd);
        setCases(undefined);
        check(d.yaml);
      }
    } catch (e) {
      setError(errMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function check(text: string) {
    try {
      setCases((await qaScenarioCheck(text)).cases);
      setError(undefined);
    } catch (e) {
      setCases(undefined);
      setError(errMessage(e));
    }
  }

  async function submit(scenarios: QaScenario[]) {
    onQueued(await qaSubmit(forTarget(scenarios, target)));
    setObjective("");
  }

  async function approvePlan() {
    if (!plan) return;
    const scenarios = plan.items
      .filter((_, i) => keep[i])
      .map((it) => (it.kind === "testset" ? { testset: it.testset ?? "", only: it.only ?? undefined } : { testset: "", goal: it }));
    try {
      await submit(scenarios);
      setPlan(undefined);
    } catch (e) {
      setError(errMessage(e));
    }
  }

  async function saveAndRun(run: boolean) {
    if (yaml === undefined) return;
    try {
      const path = await qaScenarioSave(yaml);
      if (run) await submit([{ testset: path }]);
      setYaml(undefined);
      setCases(undefined);
      if (!run) setError(undefined);
    } catch (e) {
      setError(errMessage(e));
    }
  }

  // ---- review a written scenario ----
  if (yaml !== undefined) {
    return (
      <div>
        <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
          <span className="text-sm text-zinc-300">
            Review the scenario: edit the YAML if needed
            {cost != null && <span className="text-zinc-500"> · writing cost ${cost.toFixed(3)}</span>}
          </span>
          <div className="flex gap-2">
            <Button variant="ghost" onClick={() => setYaml(undefined)}>
              Back
            </Button>
            <Button variant="ghost" onClick={() => check(yaml)}>
              Check
            </Button>
            <Button variant="ghost" disabled={!cases} onClick={() => saveAndRun(false)}>
              Save only
            </Button>
            <Button variant="primary" disabled={!cases} onClick={() => saveAndRun(true)}>
              Save and run {cases ? `${cases.length} ${cases.length === 1 ? "case" : "cases"}` : ""}
            </Button>
          </div>
        </div>
        <textarea
          aria-label="Scenario YAML"
          spellCheck={false}
          className={INPUT + " h-72 w-full resize-y font-mono text-xs"}
          value={yaml}
          onChange={(e) => {
            setYaml(e.target.value);
            setCases(undefined);
          }}
        />
        {error && <p className="mt-2 whitespace-pre-wrap text-xs text-red-400">{error}</p>}
        {cases && (
          <ul className="mt-2 flex flex-col gap-1 text-xs">
            {cases.map((c) => (
              <li key={c.id} className="flex gap-2 text-zinc-300">
                <span className="font-mono text-zinc-500">{c.id}</span>
                <span className="rounded bg-zinc-800 px-1.5 text-[10px] uppercase text-zinc-300">{c.surface}</span>
                <span className="truncate">{c.question || c.topic}</span>
              </li>
            ))}
          </ul>
        )}
        <p className="mt-2 text-xs text-zinc-500">
          Checked with the QA skill's own loader. Only read-only surfaces (Veronika, search, home) with ASCII questions
          are allowed; the runner taps nothing that buys, pays or sends. Saved scenarios also appear in Pick test sets.
        </p>
      </div>
    );
  }

  // ---- review an agent plan ----
  if (plan) {
    const kept = keep.filter(Boolean).length;
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
            <Button variant="primary" disabled={kept === 0} onClick={approvePlan}>
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

  // ---- describe ----
  return (
    <div>
      <div className="mb-2 flex flex-wrap items-center gap-3">
        <Tabs<Mode>
          tabs={[
            { id: "scenario", label: "Scenario (YAML)" },
            { id: "agent", label: "Agent" },
          ]}
          value={mode}
          onChange={setMode}
        />
        <span className="text-xs text-zinc-500">
          {mode === "scenario"
            ? "The AI writes a test set you review and save; the runner plays it with no model, the same every time."
            : "The AI picks existing test sets and drives the phone itself for anything new."}
        </span>
      </div>
      <textarea
        className={INPUT + " h-24 w-full resize-y"}
        placeholder="Example: Check that search finds 10 GB internet packages with their price, and that Veronika explains roaming packages for Japan."
        value={objective}
        onChange={(e) => setObjective(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && objective.trim() && !busy) write();
        }}
      />
      <div className="mt-2 flex items-center gap-3">
        <Button variant="primary" disabled={busy || !objective.trim()} onClick={write}>
          {busy ? (mode === "scenario" ? "Writing…" : "Planning…") : mode === "scenario" ? "Write a scenario" : "Make a plan"}
        </Button>
        <span className="text-xs text-zinc-500">
          {busy ? "The AI is reading the test sets; this can take a minute or two." : "Ctrl+Enter · you review it before anything runs"}
        </span>
      </div>
      {error && <p className="mt-3 whitespace-pre-wrap text-xs text-red-400">{error}</p>}
    </div>
  );
}
