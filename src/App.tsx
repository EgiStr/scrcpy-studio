import { useEffect, useRef, useState } from "react";
import { DeviceDock } from "./components/DeviceDock";
import { SessionList } from "./components/SessionList";
import { Sidebar, type Area } from "./components/Sidebar";
import { TestPage } from "./pages/TestPage";
import { DevicesPage } from "./pages/DevicesPage";
import { MirrorPage } from "./pages/MirrorPage";
import { SettingsPage } from "./pages/SettingsPage";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { useConfig } from "./state/config";
import { binaryCurrent, profileSave } from "./lib/ipc";

export default function App() {
  const [area, setArea] = useState<Area>("test");
  const [hasBinary, setHasBinary] = useState(false);

  const args = useConfig((s) => s.args);
  const activeProfile = useConfig((s) => s.activeProfile);
  const saveTimer = useRef<number | null>(null);
  const firstRun = useRef(true);

  useEffect(() => {
    const check = () =>
      binaryCurrent()
        .then((b) => setHasBinary(!!b))
        .catch(() => undefined);
    check();
    const t = setInterval(check, 3000);
    return () => clearInterval(t);
  }, []);

  // Auto-save parameter changes to the active profile (debounced).
  useEffect(() => {
    if (firstRun.current) {
      firstRun.current = false;
      return;
    }
    if (!activeProfile) return;
    if (saveTimer.current) window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => {
      profileSave(activeProfile, args).catch(() => undefined);
    }, 600);
  }, [args, activeProfile]);

  // The device dock and session list stay mounted everywhere (they keep the device list
  // fresh and catch session events) but show only where they are used.
  const showDock = area === "devices" || area === "mirror";

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-zinc-900 text-zinc-100">
      <Sidebar area={area} onNavigate={setArea} />
      <div className={showDock ? "contents" : "hidden"}>
        <DeviceDock />
      </div>

      <main className="flex min-w-0 flex-1 flex-col">
        <div className="min-h-0 flex-1">
          <ErrorBoundary key={area}>
            {area === "test" && <TestPage onDevices={() => setArea("devices")} onSettings={() => setArea("settings")} />}
            {area === "devices" && <DevicesPage />}
            {area === "mirror" && <MirrorPage hasBinary={hasBinary} />}
            {area === "settings" && <SettingsPage />}
          </ErrorBoundary>
        </div>
      </main>

      <div className={area === "mirror" ? "contents" : "hidden"}>
        <SessionList />
      </div>
    </div>
  );
}
