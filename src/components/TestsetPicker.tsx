// Pick existing test sets and saved AI scenarios (optionally only some case ids); each
// becomes one job on its own phone.
import { useEffect, useState } from "react";
import { errMessage, qaSubmit, qaTestsets } from "../lib/ipc";
import { Button } from "./ui";
import { forTarget, type Target } from "./DeviceTarget";

const isSaved = (path: string) => /[\\/]scenarios[\\/]/.test(path);

const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

export function TestsetPicker({
  target,
  onQueued,
  onSettings,
}: {
  target: Target;
  onQueued: (runId: string) => void;
  onSettings: () => void;
}) {
  const [testsets, setTestsets] = useState<string[]>();
  const [picked, setPicked] = useState<Record<string, boolean>>({});
  const [only, setOnly] = useState<Record<string, string>>({});
  const [filter, setFilter] = useState("");
  const [error, setError] = useState<string>();

  useEffect(() => {
    qaTestsets()
      .then(setTestsets)
      .catch((e) => {
        setTestsets([]);
        setError(errMessage(e));
      });
  }, []);

  async function submit() {
    const scenarios = (testsets ?? [])
      .filter((t) => picked[t])
      .map((t) => ({ testset: t, only: only[t]?.trim() || undefined }));
    try {
      onQueued(await qaSubmit(forTarget(scenarios, target)));
      setPicked({});
      setOnly({});
      setError(undefined);
    } catch (e) {
      setError(errMessage(e));
    }
  }

  if (testsets === undefined) return <p className="text-sm text-zinc-500">Loading test sets…</p>;
  if (testsets.length === 0) {
    return (
      <div className="text-sm">
        <p className="text-amber-300">{error ?? "No test sets found."}</p>
        <Button variant="ghost" onClick={onSettings}>
          Set the QA skill folder in Settings
        </Button>
      </div>
    );
  }

  const shown = testsets.filter((t) => fileName(t).toLowerCase().includes(filter.trim().toLowerCase()));
  const pickedCount = testsets.filter((t) => picked[t]).length;

  return (
    <div>
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <input
          className="min-w-0 flex-1 rounded-md border border-zinc-700 bg-zinc-900 px-3 py-1.5 text-sm text-zinc-100 outline-none focus:border-emerald-500 placeholder:text-zinc-600"
          placeholder={`Filter ${testsets.length} test sets`}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        <Button variant="primary" disabled={pickedCount === 0} onClick={submit}>
          Run {pickedCount || ""} selected
        </Button>
      </div>
      {error && <p className="mb-2 text-xs text-red-400">{error}</p>}
      <div className="grid max-h-72 grid-cols-[repeat(auto-fill,minmax(20rem,1fr))] gap-x-4 gap-y-1 overflow-y-auto">
        {shown.map((t) => (
          <div key={t} className="flex items-center gap-2 rounded px-1 py-0.5 hover:bg-zinc-900">
            <label className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 text-sm text-zinc-300">
              <input type="checkbox" checked={!!picked[t]} onChange={(e) => setPicked({ ...picked, [t]: e.target.checked })} />
              <span className="truncate" title={t}>
                {fileName(t)}
              </span>
              {isSaved(t) && <span className="shrink-0 rounded bg-sky-500/20 px-1 text-[10px] text-sky-300">AI scenario</span>}
            </label>
            {picked[t] && (
              <input
                className="w-32 rounded border border-zinc-700 bg-zinc-900 px-2 py-0.5 text-xs text-zinc-100 outline-none focus:border-emerald-500 placeholder:text-zinc-600"
                placeholder="only: case ids"
                value={only[t] ?? ""}
                onChange={(e) => setOnly({ ...only, [t]: e.target.value })}
              />
            )}
          </div>
        ))}
      </div>
      <p className="mt-2 text-xs text-zinc-500">
        Each test set runs as one job on its own phone, in parallel when several phones are free. Phones that share one
        MyTelkomsel account can interfere with each other.
      </p>
    </div>
  );
}
