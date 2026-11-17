// Test: start a run (describe it to the AI, or pick test sets) and follow every run from
// queue to findings on one page.
import { useEffect, useState } from "react";
import { remember, remembered } from "../lib/remember";
import { qaConfigGet, qaState, type QaJob, type QaLease } from "../lib/ipc";
import { useDevices } from "../state/devices";
import { Button, Tabs } from "../components/ui";
import { usePoll } from "../components/qa";
import { AiPlanner } from "../components/AiPlanner";
import { TestsetPicker } from "../components/TestsetPicker";
import { RunList } from "../components/RunList";
import { DeviceTarget, type Target } from "../components/DeviceTarget";

type Mode = "ai" | "sets";

export function TestPage({ onDevices, onSettings }: { onDevices: () => void; onSettings: () => void }) {
  const [mode, setMode] = useState<Mode>(() => remembered("test.mode", ["ai", "sets"] as Mode[], "ai"));
  useEffect(() => {
    remember("test.mode", mode);
  }, [mode]);
  const [jobs, setJobs] = useState<QaJob[]>([]);
  const [leases, setLeases] = useState<QaLease[]>([]);
  const [open, setOpen] = useState<string>();
  const devices = useDevices((s) => s.devices);
  const [target, setTarget] = useState<Target>({ serials: [], spread: "share" });
  const [maxParallel, setMaxParallel] = useState(2);
  useEffect(() => {
    qaConfigGet()
      .then((c) => setMaxParallel(c.maxParallel))
      .catch(() => undefined);
  }, []);

  usePoll(
    async () => {
      const s = await qaState();
      setJobs(s.jobs);
      setLeases(s.leases);
    },
    1500,
    true,
    [],
  );

  const ready = devices.filter((d) => d.state === "device");
  const free = ready.filter((d) => !leases.some((l) => l.serial === d.serial)).length;
  const running = jobs.filter((j) => j.status === "running").length;
  const queued = jobs.filter((j) => j.status === "queued").length;

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto flex max-w-5xl flex-col gap-6 p-6">
        <header className="flex flex-wrap items-end justify-between gap-3">
          <div>
            <h1 className="text-xl font-semibold text-zinc-100">Test</h1>
            <p className="mt-0.5 text-xs text-zinc-500">
              {running} running · {queued} waiting for a phone
            </p>
          </div>
          <button
            type="button"
            onClick={onDevices}
            className={
              "rounded-full border px-3 py-1 text-xs transition-colors hover:bg-zinc-800 " +
              (ready.length === 0 ? "border-amber-500/40 text-amber-300" : "border-zinc-700 text-zinc-300")
            }
          >
            {ready.length === 0 ? "No phone connected" : `${ready.length} ${ready.length === 1 ? "phone" : "phones"} · ${free} free`}
          </button>
        </header>

        <section className="rounded-lg border border-zinc-800 bg-zinc-950 p-4">
          <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
            <h2 className="text-sm font-medium text-zinc-200">New run</h2>
            <Tabs<Mode>
              tabs={[
                { id: "ai", label: "Describe it" },
                { id: "sets", label: "Pick test sets" },
              ]}
              value={mode}
              onChange={setMode}
            />
          </div>
          <DeviceTarget devices={devices} leases={leases} maxParallel={maxParallel} value={target} onChange={setTarget} />
          {mode === "ai" ? (
            <AiPlanner target={target} onQueued={setOpen} />
          ) : (
            <TestsetPicker target={target} onQueued={setOpen} onSettings={onSettings} />
          )}
          {ready.length === 0 && (
            <p className="mt-3 flex flex-wrap items-center gap-2 text-xs text-amber-300">
              Runs wait in the queue until a phone is connected.
              <Button variant="ghost" onClick={onDevices}>
                Connect a phone
              </Button>
            </p>
          )}
        </section>

        <section>
          <h2 className="mb-2 text-sm font-medium text-zinc-200">Runs</h2>
          <RunList jobs={jobs} open={open} onOpen={setOpen} />
        </section>
      </div>
    </div>
  );
}
