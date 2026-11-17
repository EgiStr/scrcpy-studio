// Which phones a new run may use. None picked: any free phone. Several picked: the
// scenarios are shared out and run in parallel, or every scenario runs on each phone.
import type { QaLease, QaMode, QaScenario } from "../lib/ipc";
import type { Device } from "../lib/types";

export type Spread = "share" | "each";
export interface Target {
  serials: string[];
  spread: Spread;
  mode: QaMode;
}

/** The scenarios to submit for a target: pinned to the picked phones, once per phone for "each". */
export function forTarget(scenarios: QaScenario[], t: Target): QaScenario[] {
  scenarios = scenarios.map((sc) => ({ ...sc, mode: t.mode }));
  if (t.serials.length === 0) return scenarios;
  if (t.spread === "each") return t.serials.flatMap((s) => scenarios.map((sc) => ({ ...sc, devices: [s] })));
  return scenarios.map((sc) => ({ ...sc, devices: t.serials }));
}

const CHIP = "rounded-full border px-3 py-1 text-xs transition-colors ";

export interface PhoneEntry {
  /** The phone's id (`ro.serialno`), shared by its USB and wireless serials. */
  id: string;
  /** The serial a pick names: USB when it is plugged in. */
  serial: string;
  serials: string[];
  model?: string;
}

const wireless = (s: string) => s.includes(":") || s.includes("._adb");

/** Online devices, one entry per phone: a phone on USB and wireless at once is one phone. */
export function phonesOf(devices: Device[]): PhoneEntry[] {
  const out: PhoneEntry[] = [];
  for (const d of devices.filter((x) => x.state === "device")) {
    const id = d.phone ?? d.serial;
    const p = out.find((x) => x.id === id);
    if (!p) out.push({ id, serial: d.serial, serials: [d.serial], model: d.model });
    else {
      p.serials.push(d.serial);
      if (wireless(p.serial) && !wireless(d.serial)) p.serial = d.serial;
    }
  }
  return out;
}

export const phoneBusy = (p: PhoneEntry, leases: QaLease[]) =>
  leases.some((l) => l.phone === p.id || p.serials.includes(l.serial));

const MODES: { id: QaMode; label: string; hint: string }[] = [
  {
    id: "auto",
    label: "Auto",
    hint: "Physical phones first, each running tests on hidden displays (one Veronika test per phone at a time), then emulators. A whole test set is split across the free places and merged into one result.",
  },
  { id: "screen", label: "Phone screen", hint: "One test per phone on its own screen, so you can watch or mirror it." },
  { id: "hidden", label: "Hidden sessions", hint: "Hidden displays on physical phones only; emulators are not used." },
];

export function DeviceTarget({
  devices,
  leases,
  maxParallel,
  sessions,
  value,
  onChange,
}: {
  devices: Device[];
  leases: QaLease[];
  maxParallel: number;
  /** Tests per phone on hidden displays (Settings); 0 turns hidden sessions off. */
  sessions: number;
  value: Target;
  onChange: (t: Target) => void;
}) {
  const ready = phonesOf(devices);
  // Picked phones that went away stay listed, so the tester sees why a run waits.
  const gone = value.serials.filter((s) => !ready.some((d) => d.serials.includes(s)));
  const toggle = (s: string) =>
    onChange({ ...value, serials: value.serials.includes(s) ? value.serials.filter((x) => x !== s) : [...value.serials, s] });
  const n = value.serials.length;

  return (
    <div className="mb-3 flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-xs text-zinc-500">Mode</span>
        {MODES.map((m) => {
          const off = m.id === "hidden" && sessions === 0;
          const on = value.mode === m.id;
          return (
            <button
              key={m.id}
              type="button"
              aria-pressed={on}
              disabled={off}
              title={off ? "Turn on AI tests per phone (1 or 2) in Settings" : m.hint}
              onClick={() => onChange({ ...value, mode: m.id })}
              className={
                CHIP +
                (on ? "border-emerald-500/60 bg-emerald-600/15 text-emerald-300" : "border-zinc-700 text-zinc-300 hover:bg-zinc-800") +
                (off ? " cursor-not-allowed opacity-40" : "")
              }
            >
              {m.label}
            </button>
          );
        })}
        {value.mode === "auto" && sessions === 0 && (
          <span className="text-xs text-zinc-500">hidden sessions are off in Settings: one test per phone screen</span>
        )}
      </div>
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
          const on = d.serials.some((s) => value.serials.includes(s));
          const busy = phoneBusy(d, leases);
          return (
            <button
              key={d.id}
              type="button"
              aria-pressed={on}
              title={d.serials.join(", ")}
              onClick={() => toggle(value.serials.find((s) => d.serials.includes(s)) ?? d.serial)}
              className={CHIP + (on ? "border-emerald-500/60 bg-emerald-600/15 text-emerald-300" : "border-zinc-700 text-zinc-300 hover:bg-zinc-800")}
            >
              {d.model ?? d.serial}
              {d.serial.startsWith("emulator-") && <span className="ml-1 text-zinc-500">emulator</span>}
              {d.serials.length > 1 && <span className="ml-1 text-zinc-500">USB + Wi-Fi</span>}
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
