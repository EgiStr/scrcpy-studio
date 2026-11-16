// Monitor: every phone on a USB cable with its health, a live screenshot, and the job that
// holds it, showing the AI agent's calls as they happen or the scripted run's log.
import { useEffect, useRef, useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import {
  errMessage,
  launch,
  monDevices,
  monScreen,
  qaCancel,
  qaLog,
  qaState,
  qaTrace,
  type QaJob,
  type QaLease,
  type TraceEvent,
  type UsbDevice,
} from "../lib/ipc";
import { useConfig } from "../state/config";
import { useSessions } from "../state/sessions";
import { Button, PanelTitle } from "../components/ui";

const STATUS: Record<string, string> = {
  running: "bg-amber-500/20 text-amber-300",
  passed: "bg-emerald-500/20 text-emerald-300",
  failed: "bg-red-500/20 text-red-300",
  blocked: "bg-orange-500/20 text-orange-300",
  error: "bg-red-500/20 text-red-300",
  cancelled: "bg-zinc-700 text-zinc-300",
  queued: "bg-zinc-800 text-zinc-400",
};

const TRACE: Record<TraceEvent["kind"], string> = {
  say: "text-zinc-300",
  tool: "text-sky-300",
  result: "text-zinc-500",
  done: "text-emerald-300",
};

function since(secs?: number | null, until?: number | null) {
  if (!secs) return "";
  const d = Math.max(0, (until ?? Math.floor(Date.now() / 1000)) - secs);
  return `${Math.floor(d / 60)}m ${String(d % 60).padStart(2, "0")}s`;
}

/** Poll `fn` every `ms` while `on`, never overlapping two calls. */
function usePoll(fn: () => Promise<void>, ms: number, on: boolean, deps: unknown[]) {
  useEffect(() => {
    if (!on) return;
    let alive = true;
    let timer: ReturnType<typeof setTimeout>;
    const tick = async () => {
      await fn().catch(() => undefined);
      if (alive) timer = setTimeout(tick, ms);
    };
    tick();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [on, ms, ...deps]);
}

function JobFeed({ job }: { job: QaJob }) {
  const [trace, setTrace] = useState<TraceEvent[]>([]);
  const [log, setLog] = useState("");
  const end = useRef<HTMLDivElement>(null);
  const live = job.status === "running";
  usePoll(
    async () => {
      if (job.kind === "agent") setTrace(await qaTrace(job.id));
      else setLog(await qaLog(job.id));
    },
    1500,
    true,
    [job.id, live],
  );
  // A block body: scrollIntoView may return a value, and React would call it as a cleanup.
  useEffect(() => {
    end.current?.scrollIntoView({ block: "nearest" });
  }, [trace.length, log]);

  return (
    <div className="h-56 overflow-y-auto rounded border border-zinc-800 bg-black p-2 font-mono text-[11px] leading-snug">
      {job.kind === "agent" ? (
        trace.length === 0 ? (
          <span className="text-zinc-500">waiting for the agent…</span>
        ) : (
          trace.slice(-60).map((e, i) => (
            <div key={i} className={(e.error ? "text-red-400" : TRACE[e.kind]) + " whitespace-pre-wrap break-words"}>
              {e.kind === "tool" ? `→ ${e.name} ${e.detail}` : e.kind === "result" ? `  ${e.detail}` : e.detail}
            </div>
          ))
        )
      ) : (
        <pre className="whitespace-pre-wrap text-zinc-300">{log.split("\n").slice(-40).join("\n") || "(no output yet)"}</pre>
      )}
      <div ref={end} />
    </div>
  );
}

function DeviceCard({
  d,
  job,
  leased,
  liveScreen,
  onError,
}: {
  d: UsbDevice;
  job?: QaJob;
  leased: boolean;
  liveScreen: boolean;
  onError: (e: string) => void;
}) {
  const [shot, setShot] = useState<string>();
  const args = useConfig((s) => s.args);
  const addSession = useSessions((s) => s.add);
  const online = d.state === "device";
  usePoll(async () => setShot(await monScreen(d.serial)), 2500, liveScreen && online, [d.serial]);

  return (
    <div className="flex gap-3 rounded-md border border-zinc-800 bg-zinc-950 p-3">
      <div className="flex w-44 shrink-0 items-center justify-center overflow-hidden rounded bg-black">
        {online && shot ? (
          <img src={shot} alt={`screen of ${d.serial}`} className="max-h-96 w-full object-contain" />
        ) : (
          <span className="p-4 text-center text-xs text-zinc-500">{online ? (liveScreen ? "loading…" : "live screen off") : d.state}</span>
        )}
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <div className="flex items-start justify-between gap-2">
          <div className="min-w-0">
            <div className="text-sm font-medium text-zinc-100">{d.model ?? d.serial}</div>
            <div className="font-mono text-xs text-zinc-500">{d.serial}</div>
          </div>
          <span
            className={
              "rounded px-2 py-0.5 text-[11px] font-semibold uppercase " +
              (!online ? "bg-red-500/20 text-red-300" : leased ? "bg-amber-500/20 text-amber-300" : "bg-emerald-500/20 text-emerald-300")
            }
          >
            {!online ? d.state : leased && job ? `busy J${job.id}` : "free"}
          </span>
        </div>
        {d.state === "unauthorized" && (
          <p className="text-xs text-amber-300">Unlock the phone and allow USB debugging for this computer.</p>
        )}
        {online && (
          <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-zinc-400">
            {d.android && <span>Android {d.android}</span>}
            {d.battery != null && <span className={d.battery < 20 ? "text-red-400" : ""}>battery {d.battery}%</span>}
            {d.temperatureC != null && (
              <span className={d.temperatureC >= 42 ? "text-red-400" : ""}>{d.temperatureC.toFixed(1)} °C</span>
            )}
            {d.foreground && (
              <span className="truncate font-mono" title={d.foreground}>
                front: {d.foreground.split("/")[0]}
              </span>
            )}
          </div>
        )}
        {job ? (
          <>
            <div className="flex flex-wrap items-center gap-2 text-xs">
              <span className={"rounded px-2 py-0.5 font-semibold uppercase " + (STATUS[job.status] ?? "")}>{job.status}</span>
              <span className="text-zinc-200">{job.scenario}</span>
              <span className="text-zinc-500">
                {job.kind === "agent" ? `AI · ${job.actions} actions · ` : "scripted · "}
                {since(job.startedAt, job.finishedAt)}
              </span>
            </div>
            {job.goal?.goal && <p className="text-xs text-zinc-400">{job.goal.goal}</p>}
            <JobFeed job={job} />
          </>
        ) : (
          online && <p className="text-xs text-zinc-500">No job on this phone. Queue one from AI Agent or QA Runs.</p>
        )}
        <div className="flex gap-2">
          {online && (
            <Button
              variant="ghost"
              onClick={() =>
                launch({ ...args, connect: { serial: d.serial } })
                  .then(addSession)
                  .catch((e) => onError(errMessage(e)))
              }
            >
              Mirror
            </Button>
          )}
          {job?.evidenceDir && (
            <Button variant="ghost" onClick={() => openPath(job.evidenceDir!).catch((e) => onError(errMessage(e)))}>
              Evidence
            </Button>
          )}
          {job && job.status === "running" && (
            <Button variant="danger" onClick={() => qaCancel(job.id).catch((e) => onError(errMessage(e)))}>
              Stop job
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}

export function MonitorPage() {
  const [devices, setDevices] = useState<UsbDevice[]>();
  const [jobs, setJobs] = useState<QaJob[]>([]);
  const [leases, setLeases] = useState<QaLease[]>([]);
  const [liveScreen, setLiveScreen] = useState(true);
  const [error, setError] = useState<string>();

  usePoll(
    async () => {
      try {
        setDevices(await monDevices());
      } catch (e) {
        setError(errMessage(e));
      }
    },
    3000,
    true,
    [],
  );
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

  // The job on a phone: the one holding its lease, else the last one that ran there.
  const jobFor = (serial: string) => {
    const lease = leases.find((l) => l.serial === serial);
    if (lease) return jobs.find((j) => j.id === lease.jobId);
    return [...jobs].reverse().find((j) => j.serial === serial);
  };
  const queued = jobs.filter((j) => j.status === "queued").length;
  const running = jobs.filter((j) => j.status === "running").length;

  return (
    <div className="h-full overflow-y-auto p-6">
      <PanelTitle
        title="Monitor"
        desc={`${devices?.length ?? 0} phones on USB · ${running} jobs running · ${queued} waiting for a phone`}
      />
      <label className="mb-4 flex items-center gap-2 text-xs text-zinc-400">
        <input type="checkbox" checked={liveScreen} onChange={(e) => setLiveScreen(e.target.checked)} />
        Live screen (a screenshot every few seconds; use Mirror for full video)
      </label>
      {error && <p className="mb-3 text-xs text-red-400">{error}</p>}
      {devices && devices.length === 0 && (
        <p className="text-sm text-amber-300">
          No phone on a USB cable. Plug one in, turn on USB debugging and allow this computer.
        </p>
      )}
      <div className="grid grid-cols-[repeat(auto-fill,minmax(34rem,1fr))] gap-3">
        {devices?.map((d) => (
          <DeviceCard
            key={d.serial}
            d={d}
            job={jobFor(d.serial)}
            leased={leases.some((l) => l.serial === d.serial)}
            liveScreen={liveScreen}
            onError={setError}
          />
        ))}
      </div>
    </div>
  );
}
