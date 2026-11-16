// Devices: phones on USB with their health, live screen and current job; and emulators.
import { useState } from "react";
import { Tabs } from "../components/ui";
import { MonitorPage } from "./MonitorPage";
import { EmulatorLabPage } from "./EmulatorLabPage";

type View = "phones" | "emulators";

export function DevicesPage() {
  const [view, setView] = useState<View>("phones");
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
