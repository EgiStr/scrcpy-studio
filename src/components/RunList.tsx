// Runs, newest first. A run opens into its jobs (with the live feed of each) and, once it
// has finished, its findings.
import { useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import { ChevronRight } from "lucide-react";
import { errMessage, qaCancel, type QaJob, type QaStatus } from "../lib/ipc";
import { Button } from "./ui";
import { JobFeed, SessionScreen, StatusBadge, since } from "./qa";
import { RunFindings } from "./RunFindings";

const ORDER: QaStatus[] = ["running", "queued", "failed", "error", "blocked", "passed", "cancelled"];

function groups(jobs: QaJob[]) {
  const runs = new Map<string, QaJob[]>();
  for (const j of jobs) runs.set(j.runId, [...(runs.get(j.runId) ?? []), j]);
  return [...runs.entries()].sort((a, b) => b[1][0].queuedAt - a[1][0].queuedAt);
}

function Summary({ jobs }: { jobs: QaJob[] }) {
  return (
    <span className="flex flex-wrap gap-1">
      {ORDER.map((s) => {
        const n = jobs.filter((j) => j.status === s).length;
        return n ? (
          <span key={s} className="flex items-center gap-1">
            <StatusBadge status={s} />
            {n > 1 && <span className="text-xs text-zinc-500">×{n}</span>}
          </span>
        ) : null;
      })}
    </span>
  );
}

function JobRow({ job, onError }: { job: QaJob; onError: (e: string) => void }) {
  const [watch, setWatch] = useState(job.status === "running");
  return (
    <li className="border-t border-zinc-800 py-2 first:border-t-0">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm">
        <span className="font-mono text-xs text-zinc-500">J{job.id}</span>
        <StatusBadge status={job.status} title={job.note} />
        <span className="min-w-0 flex-1 truncate text-zinc-200" title={job.goal?.goal ?? job.scenario}>
          {job.scenario}
        </span>
        <span className="font-mono text-xs text-zinc-500">
          {job.kind === "web"
            ? "web · veronika.telkomsel.com"
            : job.serial
            ? `${job.serial}${job.display != null ? ` · hidden display ${job.display}` : ""}`
            : job.devices?.length
              ? `waiting for ${job.devices.join(" or ")}`
              : "waiting for a phone"}
        </span>
        <span className="w-16 text-right font-mono text-xs text-zinc-500">{since(job.startedAt, job.finishedAt)}</span>
        <span className="flex gap-1">
          {(job.evidenceDir || job.status === "running") && (
            <Button variant="ghost" onClick={() => setWatch(!watch)}>
              {watch ? "Hide steps" : "Steps"}
            </Button>
          )}
          {job.evidenceDir && (
            <Button variant="ghost" onClick={() => openPath(job.evidenceDir!).catch((e) => onError(errMessage(e)))}>
              Evidence
            </Button>
          )}
          {(job.status === "queued" || job.status === "running") && (
            <Button variant="danger" onClick={() => qaCancel(job.id).catch((e) => onError(errMessage(e)))}>
              Cancel
            </Button>
          )}
        </span>
      </div>
      {job.note && job.status !== "passed" && <p className="mt-1 text-xs text-zinc-500">{job.note}</p>}
      {(watch || job.display != null || (job.kind === "web" && job.status === "running")) && (
        <div className="mt-2 flex gap-3">
          <SessionScreen job={job} onError={onError} />
          {watch && (
            <div className="min-w-0 flex-1">
              <JobFeed job={job} />
            </div>
          )}
        </div>
      )}
    </li>
  );
}

export function RunList({ jobs, open, onOpen }: { jobs: QaJob[]; open?: string; onOpen: (runId?: string) => void }) {
  const [error, setError] = useState<string>();
  const runs = groups(jobs);

  if (runs.length === 0) {
    return <p className="text-sm text-zinc-500">No runs yet. Describe a test above or pick test sets to start one.</p>;
  }

  return (
    <div className="flex flex-col gap-2">
      {error && <p className="text-xs text-red-400">{error}</p>}
      {runs.map(([runId, runJobs]) => {
        const expanded = open === runId;
        const done = runJobs.every((j) => !["queued", "running"].includes(j.status));
        const at = new Date(runJobs[0].queuedAt * 1000);
        return (
          <section key={runId} className="rounded-lg border border-zinc-800 bg-zinc-950">
            <button
              type="button"
              aria-expanded={expanded}
              onClick={() => onOpen(expanded ? undefined : runId)}
              className="flex w-full items-center gap-3 px-3 py-2.5 text-left hover:bg-zinc-900"
            >
              <ChevronRight size={16} className={"shrink-0 text-zinc-500 transition-transform " + (expanded ? "rotate-90" : "")} />
              <span className="min-w-0 flex-1">
                <span className="block truncate text-sm text-zinc-100">
                  {runJobs.length === 1 ? runJobs[0].scenario : `${runJobs.length} scenarios`}
                </span>
                <span className="font-mono text-[11px] text-zinc-500">
                  {runId} · {at.toLocaleString()}
                </span>
              </span>
              <Summary jobs={runJobs} />
            </button>
            {expanded && (
              <div className="border-t border-zinc-800 px-3 pb-3">
                <ul>
                  {runJobs.map((j) => (
                    <JobRow key={j.id} job={j} onError={setError} />
                  ))}
                </ul>
                <div className="mt-3 border-t border-zinc-800 pt-3">
                  {done ? (
                    <RunFindings runId={runId} />
                  ) : (
                    <p className="text-xs text-zinc-500">Findings appear here when every job of this run has finished.</p>
                  )}
                </div>
              </div>
            )}
          </section>
        );
      })}
    </div>
  );
}
