// Phones: every phone on a USB cable with its health, a live screenshot, and the job that
// holds it, showing the AI agent's calls as they happen or the scripted run's log.
import { useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import {
  errMessage,
  launch,
  monDevices,
  monScreen,
  qaCancel,
  qaState,
  type QaJob,
  type QaLease,
  type UsbDevice,
} from "../lib/ipc";
import { useConfig } from "../state/config";
import { useSessions } from "../state/sessions";
import { Button } from "../components/ui";
import { JobFeed, StatusBadge, since, usePoll } from "../components/qa";

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
              <StatusBadge status={job.status} title={job.note} />
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
          online && <p className="text-xs text-zinc-500">No job on this phone. Start one from Test.</p>
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
      <p className="mb-4 text-xs text-zinc-500">
        {`${devices?.length ?? 0} phones on USB · ${running} jobs running · ${queued} waiting for a phone`}
      </p>
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
