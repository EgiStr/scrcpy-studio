// The app's three areas, plus Settings at the bottom.
import { FlaskConical, MonitorPlay, Settings, Smartphone, type LucideIcon } from "lucide-react";

export type Area = "test" | "devices" | "mirror" | "settings";

const AREAS: { id: Area; label: string; icon: LucideIcon; hint: string }[] = [
  { id: "test", label: "Test", icon: FlaskConical, hint: "Plan, run and review QA tests" },
  { id: "devices", label: "Devices", icon: Smartphone, hint: "Phones, live screens and emulators" },
  { id: "mirror", label: "Mirror", icon: MonitorPlay, hint: "scrcpy screen mirroring and control" },
];

function Item({ id, label, icon: Icon, hint, area, onNavigate }: (typeof AREAS)[number] & { area: Area; onNavigate: (a: Area) => void }) {
  const active = area === id;
  return (
    <button
      type="button"
      title={hint}
      aria-current={active ? "page" : undefined}
      onClick={() => onNavigate(id)}
      className={
        "flex w-full flex-col items-center gap-1 rounded-lg px-1 py-2 text-[11px] font-medium transition-colors " +
        (active ? "bg-emerald-600/15 text-emerald-300" : "text-zinc-400 hover:bg-zinc-800 hover:text-zinc-100")
      }
    >
      <Icon size={20} strokeWidth={1.75} />
      {label}
    </button>
  );
}

export function Sidebar({ area, onNavigate }: { area: Area; onNavigate: (a: Area) => void }) {
  return (
    <nav aria-label="Main" className="flex w-20 shrink-0 flex-col gap-1 border-r border-zinc-800 bg-zinc-950 p-2">
      {AREAS.map((a) => (
        <Item key={a.id} {...a} area={area} onNavigate={onNavigate} />
      ))}
      <div className="mt-auto">
        <Item id="settings" label="Settings" icon={Settings} hint="QA skill, AI model, setup check, audit log" area={area} onNavigate={onNavigate} />
      </div>
    </nav>
  );
}
