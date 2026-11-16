// Mirror: everything scrcpy. Options and profiles shape the command; Apps controls the
// phone; the command bar launches it.
import { useEffect, useState } from "react";
import { remember, remembered } from "../lib/remember";
import { Tabs } from "../components/ui";
import { CommandBar } from "../components/CommandBar";
import { ShortcutsPanel } from "../components/ShortcutsPanel";
import { ConfigurationPage } from "./ConfigurationPage";
import { ProfilesPage } from "./ProfilesPage";
import { ControlPage } from "./ControlPage";

type View = "options" | "profiles" | "apps" | "shortcuts";

export function MirrorPage({ hasBinary }: { hasBinary: boolean }) {
  const [view, setView] = useState<View>(() => remembered("mirror.view", ["options", "profiles", "apps", "shortcuts"] as View[], "options"));
  useEffect(() => {
    remember("mirror.view", view);
  }, [view]);
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="border-b border-zinc-800 px-6 py-3">
        <Tabs<View>
          tabs={[
            { id: "options", label: "Options" },
            { id: "profiles", label: "Profiles" },
            {
              id: "apps",
              label: (
                <>
                  Apps
                  <sup className="ml-1 rounded bg-amber-500/20 px-1 text-[9px] font-semibold text-amber-400">beta</sup>
                </>
              ),
            },
            { id: "shortcuts", label: "Shortcuts" },
          ]}
          value={view}
          onChange={setView}
        />
      </div>
      <div className="min-h-0 flex-1">
        {view === "options" && <ConfigurationPage />}
        {view === "profiles" && (
          <div className="h-full overflow-y-auto">
            <ProfilesPage />
          </div>
        )}
        {view === "apps" && <ControlPage />}
        {view === "shortcuts" && (
          <div className="h-full overflow-y-auto p-5">
            <ShortcutsPanel />
          </div>
        )}
      </div>
      <CommandBar hasBinary={hasBinary} />
    </div>
  );
}
