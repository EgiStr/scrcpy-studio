// Emulator Lab: discover AVDs, boot/stop them, and open a scrcpy mirror per emulator.
import { useEffect, useState } from "react";
import {
  emuList,
  emuSdk,
  emuStart,
  emuStop,
  errMessage,
  launch,
  type EmulatorInfo,
  type EmulatorState,
  type Sdk,
} from "../lib/ipc";
import { useConfig } from "../state/config";
import { useSessions } from "../state/sessions";
import { Button } from "../components/ui";

const BADGE: Record<EmulatorState, string> = {
  stopped: "bg-zinc-800 text-zinc-400",
  starting: "bg-amber-500/20 text-amber-300",
  booting: "bg-amber-500/20 text-amber-300",
  ready: "bg-emerald-500/20 text-emerald-300",
  stopping: "bg-zinc-700 text-zinc-300",
  offline: "bg-red-500/20 text-red-300",
  failed: "bg-red-500/20 text-red-300",
};

export function EmulatorLabPage() {
  const [sdk, setSdk] = useState<Sdk | null>();
  const [emulators, setEmulators] = useState<EmulatorInfo[]>([]);
  const [headless, setHeadless] = useState(false);
  const [error, setError] = useState<string>();
  const args = useConfig((s) => s.args);
  const sessions = useSessions((s) => s.sessions);
  const addSession = useSessions((s) => s.add);

  useEffect(() => {
    emuSdk().then(setSdk).catch(() => setSdk(null));
    const poll = () =>
      emuList()
        .then(setEmulators)
        .catch((e) => setError(errMessage(e)));
    poll();
    const t = setInterval(poll, 2000);
    return () => clearInterval(t);
  }, []);

  async function run(action: () => Promise<unknown>) {
    setError(undefined);
    try {
      await action();
      setEmulators(await emuList());
    } catch (e) {
      setError(errMessage(e));
    }
  }

  const mirror = (serial: string) =>
    run(async () => addSession(await launch({ ...args, connect: { serial } })));

  const running = emulators.filter((e) => e.state !== "stopped" && e.state !== "failed").length;
  const ready = emulators.filter((e) => e.state === "ready").length;

  return (
    <div className="h-full overflow-y-auto p-6">
      <p className={"mb-4 text-xs " + (sdk === null ? "text-amber-300" : "text-zinc-500")}>
        {sdk === null
          ? "Android SDK with the emulator package was not found. Install it from Android Studio or set ANDROID_HOME."
          : `${emulators.length} AVDs · ${running} running · ${ready} ready${sdk ? ` · SDK ${sdk.root}` : ""}`}
      </p>

      <label className="mb-4 flex items-center gap-2 text-xs text-zinc-400">
        <input type="checkbox" checked={headless} onChange={(e) => setHeadless(e.target.checked)} />
        Start headless (no emulator window; use Mirror to see the screen)
      </label>

      {error && <p className="mb-3 text-xs text-red-400">{error}</p>}
      {sdk && emulators.length === 0 && (
        <p className="text-sm text-zinc-500">No AVDs yet. Create one in Android Studio's Device Manager.</p>
      )}

      <div className="grid grid-cols-[repeat(auto-fill,minmax(19rem,1fr))] gap-3">
        {emulators.map((e) => {
          const off = e.state === "stopped" || e.state === "failed";
          const mirrored = sessions.some((s) => s.serial === e.serial);
          return (
            <div key={e.name} className="rounded-md border border-zinc-800 bg-zinc-950 p-3">
              <div className="flex items-center justify-between gap-2">
                <span className="truncate text-sm font-medium text-zinc-100">{e.name}</span>
                <span className={"rounded px-2 py-0.5 text-[11px] font-semibold uppercase " + BADGE[e.state]}>
                  {e.state}
                </span>
              </div>
              <p className="mt-1 text-xs text-zinc-500">
                {[e.api && `API ${e.api}`, e.device, e.resolution, e.ram && `${e.ram} RAM`]
                  .filter(Boolean)
                  .join(" · ")}
              </p>
              <p className="mt-1 h-4 truncate font-mono text-xs text-zinc-400" title={e.detail ?? undefined}>
                {e.state === "failed" ? e.detail : e.serial}
                {e.serial && !e.managed && " (started outside the app)"}
              </p>
              <div className="mt-3 flex flex-wrap gap-2">
                {off ? (
                  <Button variant="primary" disabled={!sdk} onClick={() => run(() => emuStart(e.name, headless))}>
                    Start
                  </Button>
                ) : (
                  <>
                    <Button
                      variant="danger"
                      disabled={e.state === "stopping"}
                      onClick={() => run(() => emuStop(e.name))}
                    >
                      Stop
                    </Button>
                    <Button
                      disabled={e.state !== "ready" || mirrored}
                      title={mirrored ? "A mirror session is already open" : "Open a scrcpy mirror"}
                      onClick={() => e.serial && mirror(e.serial)}
                    >
                      Mirror
                    </Button>
                  </>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
