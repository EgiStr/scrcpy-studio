// Which phones a new run may use. None picked: any free phone. Several picked: the
// scenarios are shared out and run in parallel, or every scenario runs on each phone.
import type { QaLease, QaScenario } from "../lib/ipc";
import type { Device } from "../lib/types";

export type Spread = "share" | "each";
export interface Target {
  serials: string[];
  spread: Spread;
}

/** The scenarios to submit for a target: pinned to the picked phones, once per phone for "each". */
export function forTarget(scenarios: QaScenario[], t: Target): QaScenario[] {
  if (t.serials.length === 0) return scenarios;
  if (t.spread === "each") return t.serials.flatMap((s) => scenarios.map((sc) => ({ ...sc, devices: [s] })));
  return scenarios.map((sc) => ({ ...sc, devices: t.serials }));
}

const CHIP = "rounded-full border px-3 py-1 text-xs transition-colors ";

export function DeviceTarget({
  devices,
  leases,
  maxParallel,
  value,
  onChange,
}: {
  devices: Device[];
  leases: QaLease[];
  maxParallel: number;
  value: Target;
  onChange: (t: Target) => void;
}) {
  const ready = devices.filter((d) => d.state === "device");
  // Picked phones that went away stay listed, so the tester sees why a run waits.
  const gone = value.serials.filter((s) => !ready.some((d) => d.serial === s));
  const toggle = (s: string) =>
    onChange({ ...value, serials: value.serials.includes(s) ? value.serials.filter((x) => x !== s) : [...value.serials, s] });
  const n = value.serials.length;

  return (
    <div className="mb-3 flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-xs text-zinc-500">Run on</span>
        <button
          type="button"
          aria-pressed={n === 0}
          onClick={() => onChange({ ...value, serials: [] })}
          className={CHIP + (n === 0 ? "border-emerald-500/60 bg-emerald-600/15 text-emerald-300" : "border-zinc-700 text-zinc-400 hover:bg-zinc-800")}
        >
          Any free phone
        </button>
        {ready.map((d) => {
          const on = value.serials.includes(d.serial);
          const busy = leases.some((l) => l.serial === d.serial);
          return (
            <button
              key={d.serial}
              type="button"
              aria-pressed={on}
              title={d.serial}
              onClick={() => toggle(d.serial)}
              className={CHIP + (on ? "border-emerald-500/60 bg-emerald-600/15 text-emerald-300" : "border-zinc-700 text-zinc-300 hover:bg-zinc-800")}
            >
              {d.model ?? d.serial}
              {d.serial.startsWith("emulator-") && <span className="ml-1 text-zinc-500">emulator</span>}
              {busy && <span className="ml-1 text-amber-400">busy</span>}
            </button>
          );
        })}
        {gone.map((s) => (
          <button
            key={s}
            type="button"
            title="Not connected: jobs for it wait until it is back. Click to remove."
            onClick={() => toggle(s)}
            className={CHIP + "border-amber-500/40 text-amber-300"}
          >
            {s} · not connected
          </button>
        ))}
      </div>
      {n > 1 && (
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <label className="flex cursor-pointer items-center gap-1.5 text-zinc-300">
            <input type="radio" checked={value.spread === "share"} onChange={() => onChange({ ...value, spread: "share" })} />
            Share the scenarios across {n} phones
          </label>
          <label className="flex cursor-pointer items-center gap-1.5 text-zinc-300">
            <input type="radio" checked={value.spread === "each"} onChange={() => onChange({ ...value, spread: "each" })} />
            Run every scenario on each phone
          </label>
          {n > maxParallel && (
            <span className="text-amber-300">At most {maxParallel} run at once (parallel limit in Settings).</span>
          )}
        </div>
      )}
    </div>
  );
}
