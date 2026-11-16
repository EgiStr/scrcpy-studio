// The app's three areas, plus Settings at the bottom.
import { useState } from "react";
import { FlaskConical, MonitorPlay, Settings, Smartphone, type LucideIcon } from "lucide-react";
import { qaState } from "../lib/ipc";
import { usePoll } from "./qa";

export type Area = "test" | "devices" | "mirror" | "settings";

const AREAS: { id: Area; label: string; icon: LucideIcon; hint: string }[] = [
  { id: "test", label: "Test", icon: FlaskConical, hint: "Plan, run and review QA tests (Ctrl+1)" },
  { id: "devices", label: "Devices", icon: Smartphone, hint: "Phones, live screens and emulators (Ctrl+2)" },
  { id: "mirror", label: "Mirror", icon: MonitorPlay, hint: "scrcpy screen mirroring and control (Ctrl+3)" },
];

function Item({
  id,
  label,
  icon: Icon,
  hint,
  area,
  badge,
  onNavigate,
}: (typeof AREAS)[number] & { area: Area; badge?: number; onNavigate: (a: Area) => void }) {
  const active = area === id;
  return (
    <button
      type="button"
      title={hint}
      aria-current={active ? "page" : undefined}
      onClick={() => onNavigate(id)}
      className={
        "relative flex w-full flex-col items-center gap-1 rounded-lg px-1 py-2 text-[11px] font-medium transition-colors " +
        (active ? "bg-emerald-600/15 text-emerald-300" : "text-zinc-400 hover:bg-zinc-800 hover:text-zinc-100")
      }
    >
      <Icon size={20} strokeWidth={1.75} />
      {label}
      {!!badge && (
        <span
          className="absolute right-2 top-1 min-w-4 rounded-full bg-amber-500 px-1 text-[10px] font-semibold leading-4 text-zinc-950"
          aria-label={`${badge} running or waiting`}
        >
          {badge}
        </span>
      )}
    </button>
  );
}

export function Sidebar({ area, onNavigate }: { area: Area; onNavigate: (a: Area) => void }) {
  const [running, setRunning] = useState(0);
  usePoll(
    async () => setRunning((await qaState()).jobs.filter((j) => j.status === "running" || j.status === "queued").length),
    3000,
    true,
    [],
  );
  return (
    <nav aria-label="Main" className="flex w-20 shrink-0 flex-col gap-1 border-r border-zinc-800 bg-zinc-950 p-2">
      {AREAS.map((a) => (
        <Item key={a.id} {...a} area={area} badge={a.id === "test" ? running : undefined} onNavigate={onNavigate} />
      ))}
      <div className="mt-auto">
        <Item id="settings" label="Settings" icon={Settings} hint="QA skill, AI model, setup check, audit log (Ctrl+4)" area={area} onNavigate={onNavigate} />
      </div>
    </nav>
  );
}
