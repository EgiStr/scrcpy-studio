// QA Runs: queue Telkomsel QA Skill test sets and watch them run in parallel, one device each.
import { useEffect, useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import {
  errMessage,
  qaCancel,
  qaConfigGet,
  qaConfigSet,
  qaLog,
  qaState,
  qaSubmit,
  qaTestsets,
  type QaConfig,
  type QaJob,
  type QaLease,
  type QaStatus,
} from "../lib/ipc";
import { useDevices } from "../state/devices";
import { Button, PanelTitle } from "../components/ui";

const BADGE: Record<QaStatus, string> = {
  queued: "bg-zinc-800 text-zinc-400",
  running: "bg-amber-500/20 text-amber-300",
  passed: "bg-emerald-500/20 text-emerald-300",
  failed: "bg-red-500/20 text-red-300",
  blocked: "bg-orange-500/20 text-orange-300",
  error: "bg-red-500/20 text-red-300",
  cancelled: "bg-zinc-700 text-zinc-300",
};

const INPUT =
  "min-w-0 rounded-md border border-zinc-700 bg-zinc-900 px-2 py-1.5 text-sm text-zinc-100 outline-none focus:border-emerald-500";

const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

function duration(job: QaJob, nowSecs: number) {
  if (!job.startedAt) return "";
  const secs = (job.finishedAt ?? nowSecs) - job.startedAt;
  return `${Math.floor(secs / 60)}m ${String(secs % 60).padStart(2, "0")}s`;
}

export function QaRunsPage() {
  const [config, setConfig] = useState<QaConfig>();
  const [testsets, setTestsets] = useState<string[]>([]);
  const [picked, setPicked] = useState<Record<string, boolean>>({});
  const [only, setOnly] = useState<Record<string, string>>({});
  const [jobs, setJobs] = useState<QaJob[]>([]);
  const [leases, setLeases] = useState<QaLease[]>([]);
  const [openJob, setOpenJob] = useState<number>();
  const [log, setLog] = useState("");
  const [error, setError] = useState<string>();
  // The dock keeps this list fresh; phones over USB or Wi-Fi and emulators all count.
  const devices = useDevices((s) => s.devices);

  const loadTestsets = () =>
    qaTestsets()
      .then((t) => {
        setTestsets(t);
        setError(undefined);
      })
      .catch((e) => {
        setTestsets([]);
        setError(errMessage(e));
      });

  useEffect(() => {
    qaConfigGet().then(setConfig);
    loadTestsets();
  }, []);

  useEffect(() => {
    const poll = () => {
      qaState().then((s) => {
        setJobs(s.jobs);
        setLeases(s.leases);
      });
      if (openJob !== undefined) qaLog(openJob).then(setLog).catch(() => undefined);
    };
    poll();
    const t = setInterval(poll, 1500);
    return () => clearInterval(t);
  }, [openJob]);

  async function saveConfig() {
    if (!config) return;
    try {
      await qaConfigSet(config);
      await loadTestsets();
    } catch (e) {
      setError(errMessage(e));
    }
  }

  async function submit() {
    const scenarios = testsets
      .filter((t) => picked[t])
      .map((t) => ({ testset: t, only: only[t]?.trim() || undefined }));
    try {
      await qaSubmit(scenarios);
      setPicked({});
      setError(undefined);
    } catch (e) {
      setError(errMessage(e));
    }
  }

  const count = (s: QaStatus) => jobs.filter((j) => j.status === s).length;
  const nowSecs = Math.floor(Date.now() / 1000);
  const pickedCount = testsets.filter((t) => picked[t]).length;

  return (
    <div className="h-full overflow-y-auto p-6">
      <PanelTitle
        title="QA Runs"
        desc={`${count("running")} running · ${count("queued")} queued · ${count("passed")} passed · ${count("failed")} failed · ${count("blocked")} blocked · ${count("error")} error · ${leases.length} devices leased`}
      />

      {config && (
        <div className="mb-4 flex flex-wrap items-end gap-2 text-xs text-zinc-400">
          <label className="flex min-w-[18rem] flex-1 flex-col gap-1">
            QA skill folder (telkomsel-qa-skills)
            <input
              className={INPUT}
              value={config.skillDir}
              onChange={(e) => setConfig({ ...config, skillDir: e.target.value })}
            />
          </label>
          <label className="flex w-28 flex-col gap-1">
            Python
            <input
              className={INPUT}
              value={config.python}
              onChange={(e) => setConfig({ ...config, python: e.target.value })}
            />
          </label>
          <label className="flex w-24 flex-col gap-1">
            Max parallel
            <input
              className={INPUT}
              type="number"
              min={1}
              value={config.maxParallel}
              onChange={(e) => setConfig({ ...config, maxParallel: Number(e.target.value) || 1 })}
            />
          </label>
          <label className="flex w-28 flex-col gap-1">
            Timeout (s)
            <input
              className={INPUT}
              type="number"
              min={60}
              value={config.timeoutSecs}
              onChange={(e) => setConfig({ ...config, timeoutSecs: Number(e.target.value) || 3600 })}
            />
          </label>
          <Button onClick={saveConfig}>Save</Button>
        </div>
      )}

      {error && <p className="mb-3 text-xs text-red-400">{error}</p>}

      <div className="mb-4 flex flex-wrap items-center gap-2 text-xs">
        <span className="text-zinc-500">Devices</span>
        {devices.length === 0 && (
          <span className="text-amber-400">
            none connected: plug in a phone with USB debugging on, or pair one over Wi-Fi from the dock
          </span>
        )}
        {devices.map((d) => {
          const lease = leases.find((l) => l.serial === d.serial);
          const usable = d.state === "device";
          return (
            <span
              key={d.serial}
              className={
                "rounded border px-2 py-1 font-mono " +
                (lease
                  ? "border-amber-500/40 text-amber-300"
                  : usable
                    ? "border-emerald-500/40 text-emerald-300"
                    : "border-red-500/40 text-red-300")
              }
            >
              {d.model ?? d.serial} · {d.serial} · {lease ? `busy J${lease.jobId}` : usable ? "free" : d.state}
            </span>
          );
        })}
      </div>

      {testsets.length > 0 && (
        <div className="mb-5 rounded-md border border-zinc-800 bg-zinc-950 p-3">
          <div className="mb-2 flex items-center justify-between">
            <span className="text-sm font-medium text-zinc-200">Scenarios</span>
            <Button variant="primary" disabled={pickedCount === 0} onClick={submit}>
              Run {pickedCount || ""} selected
            </Button>
          </div>
          <p className="mb-2 text-xs text-zinc-500">
            Each selected test set becomes one job on its own device. Devices that share one
            MyTelkomsel account can interfere with each other.
          </p>
          <div className="grid grid-cols-[repeat(auto-fill,minmax(22rem,1fr))] gap-x-4 gap-y-1">
            {testsets.map((t) => (
              <div key={t} className="flex items-center gap-2">
                <label className="flex min-w-0 flex-1 items-center gap-2 text-sm text-zinc-300">
                  <input
                    type="checkbox"
                    checked={!!picked[t]}
                    onChange={(e) => setPicked({ ...picked, [t]: e.target.checked })}
                  />
                  <span className="truncate" title={t}>
                    {fileName(t)}
                  </span>
                </label>
                <input
                  className={INPUT + " w-36 py-0.5 text-xs"}
                  placeholder="only: case ids"
                  value={only[t] ?? ""}
                  onChange={(e) => setOnly({ ...only, [t]: e.target.value })}
                />
              </div>
            ))}
          </div>
        </div>
      )}

      {jobs.length === 0 ? (
        <p className="text-sm text-zinc-500">No jobs yet.</p>
      ) : (
        <table className="w-full text-left text-sm">
          <thead className="text-xs text-zinc-500">
            <tr>
              <th className="py-1 pr-3 font-medium">Job</th>
              <th className="pr-3 font-medium">Run</th>
              <th className="pr-3 font-medium">Scenario</th>
              <th className="pr-3 font-medium">Device</th>
              <th className="pr-3 font-medium">Status</th>
              <th className="pr-3 font-medium">Time</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {[...jobs].reverse().map((j) => (
              <tr key={j.id} className="border-t border-zinc-800 text-zinc-300">
                <td className="py-1.5 pr-3 font-mono text-xs">J{j.id}</td>
                <td className="pr-3 font-mono text-xs text-zinc-500">{j.runId}</td>
                <td className="pr-3">{j.scenario}</td>
                <td className="pr-3 font-mono text-xs">{j.serial ?? "waiting for a device"}</td>
                <td className="pr-3">
                  <span
                    className={"rounded px-2 py-0.5 text-[11px] font-semibold uppercase " + BADGE[j.status]}
                    title={j.note ?? undefined}
                  >
                    {j.status}
                  </span>
                </td>
                <td className="pr-3 font-mono text-xs">{duration(j, nowSecs)}</td>
                <td className="flex justify-end gap-2 py-1">
                  {j.evidenceDir && (
                    <>
                      <Button variant="ghost" onClick={() => setOpenJob(openJob === j.id ? undefined : j.id)}>
                        {openJob === j.id ? "Hide log" : "Log"}
                      </Button>
                      <Button variant="ghost" onClick={() => openPath(j.evidenceDir!).catch((e) => setError(errMessage(e)))}>
                        Evidence
                      </Button>
                    </>
                  )}
                  {(j.status === "queued" || j.status === "running") && (
                    <Button variant="danger" onClick={() => qaCancel(j.id).catch((e) => setError(errMessage(e)))}>
                      Cancel
                    </Button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {openJob !== undefined && (
        <pre className="mt-3 max-h-80 overflow-auto rounded-md border border-zinc-800 bg-black p-3 font-mono text-xs text-zinc-300">
          {log || "(no output yet)"}
        </pre>
      )}
    </div>
  );
}
