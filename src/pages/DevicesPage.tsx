// Devices: phones on USB with their health, live screen and current job; and emulators.
import { useEffect, useState } from "react";
import { remember, remembered } from "../lib/remember";
import { Tabs } from "../components/ui";
import { MonitorPage } from "./MonitorPage";
import { EmulatorLabPage } from "./EmulatorLabPage";

type View = "phones" | "emulators";

export function DevicesPage() {
  const [view, setView] = useState<View>(() => remembered("devices.view", ["phones", "emulators"] as View[], "phones"));
  useEffect(() => {
    remember("devices.view", view);
  }, [view]);
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="border-b border-zinc-800 px-6 py-3">
        <Tabs<View>
          tabs={[
            { id: "phones", label: "Phones" },
            { id: "emulators", label: "Emulators" },
          ]}
          value={view}
          onChange={setView}
        />
      </div>
      <div className="min-h-0 flex-1">{view === "phones" ? <MonitorPage /> : <EmulatorLabPage />}</div>
    </div>
  );
}
