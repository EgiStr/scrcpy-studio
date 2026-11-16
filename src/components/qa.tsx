// Pieces every QA view shares: the status badge, polling, run time and a job's live feed.
import { useEffect, useRef, useState } from "react";
import { qaLog, qaTrace, type QaJob, type QaStatus, type TraceEvent } from "../lib/ipc";

export const STATUS_BADGE: Record<QaStatus, string> = {
  queued: "bg-zinc-800 text-zinc-400",
  running: "bg-amber-500/20 text-amber-300",
  passed: "bg-emerald-500/20 text-emerald-300",
  failed: "bg-red-500/20 text-red-300",
  blocked: "bg-orange-500/20 text-orange-300",
  error: "bg-red-500/20 text-red-300",
  cancelled: "bg-zinc-700 text-zinc-300",
};

export function StatusBadge({ status, title }: { status: QaStatus; title?: string | null }) {
  return (
    <span
      className={"rounded px-2 py-0.5 text-[11px] font-semibold uppercase " + STATUS_BADGE[status]}
      title={title ?? undefined}
    >
      {status}
    </span>
  );
}

export function since(secs?: number | null, until?: number | null) {
  if (!secs) return "";
  const d = Math.max(0, (until ?? Math.floor(Date.now() / 1000)) - secs);
  return `${Math.floor(d / 60)}m ${String(d % 60).padStart(2, "0")}s`;
}

/** Poll `fn` every `ms` while `on`, never overlapping two calls. */
export function usePoll(fn: () => Promise<void>, ms: number, on: boolean, deps: unknown[]) {
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

const TRACE: Record<TraceEvent["kind"], string> = {
  say: "text-zinc-300",
  tool: "text-sky-300",
  result: "text-zinc-500",
  done: "text-emerald-300",
};

/** The AI agent's calls as they happen, or a scripted run's log. */
export function JobFeed({ job }: { job: QaJob }) {
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
