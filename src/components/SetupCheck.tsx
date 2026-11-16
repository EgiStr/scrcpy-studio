// Setup check: lists each dependency a QA or AI run needs, and what to do about a missing one.
import { useState } from "react";
import { errMessage, qaDoctor, type SetupCheck as Check } from "../lib/ipc";
import { Button } from "./ui";

export function SetupCheck() {
  const [checks, setChecks] = useState<Check[]>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  async function run() {
    setBusy(true);
    setError(undefined);
    try {
      setChecks(await qaDoctor());
    } catch (e) {
      setError(errMessage(e));
    } finally {
      setBusy(false);
    }
  }

  const failed = checks?.filter((c) => !c.ok).length ?? 0;

  return (
    <div className="mb-4">
      <div className="flex items-center gap-3">
        <Button variant="ghost" disabled={busy} onClick={run}>
          {busy ? "Checking…" : "Check setup"}
        </Button>
        {checks && (
          <span className={"text-xs " + (failed ? "text-amber-300" : "text-emerald-300")}>
            {failed ? `${failed} of ${checks.length} need attention` : "Everything a run needs is in place."}
          </span>
        )}
        {busy && <span className="text-xs text-zinc-500">This asks the AI model once, so it takes a few seconds.</span>}
      </div>
      {error && <p className="mt-2 text-xs text-red-400">{error}</p>}
      {checks && (
        <ul className="mt-2 flex flex-col gap-1 text-xs">
          {checks.map((c) => (
            <li key={c.name} className="flex gap-2">
              <span className={c.ok ? "text-emerald-400" : "text-red-400"}>{c.ok ? "OK" : "MISSING"}</span>
              <span className="w-56 shrink-0 text-zinc-200">{c.name}</span>
              <span className="min-w-0 break-all text-zinc-500">
                {c.detail}
                {c.fix && <span className="block text-amber-300">{c.fix}</span>}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
