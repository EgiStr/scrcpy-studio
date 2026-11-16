// Settings: the QA skill and AI model in one form, the setup check, the audit log, and
// updates.
import { useEffect, useState, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  binaryUpdate,
  errMessage,
  qaConfigGet,
  qaConfigSet,
  qaEvents,
  type AuditEvent,
  type QaConfig,
} from "../lib/ipc";
import { Button, Field } from "../components/ui";
import { SetupCheck } from "../components/SetupCheck";

const SCRCPY_REPO = "https://github.com/Genymobile/scrcpy";
const APP_REPO = "https://github.com/rifqioe/scrcpy-studio";

const INPUT =
  "w-full min-w-0 rounded-md border border-zinc-700 bg-zinc-900 px-2.5 py-1.5 text-sm text-zinc-100 outline-none focus:border-emerald-500";

function Section({ title, desc, children }: { title: string; desc?: string; children: ReactNode }) {
  return (
    <section className="rounded-lg border border-zinc-800 bg-zinc-950 p-4">
      <h2 className="text-sm font-medium text-zinc-200">{title}</h2>
      {desc && <p className="mt-0.5 text-xs text-zinc-500">{desc}</p>}
      <div className="mt-3">{children}</div>
    </section>
  );
}

export function SettingsPage() {
  const [config, setConfig] = useState<QaConfig>();
  const [saved, setSaved] = useState<QaConfig>();
  const [note, setNote] = useState<string>();
  const [error, setError] = useState<string>();
  const [audit, setAudit] = useState<AuditEvent[]>();

  useEffect(() => {
    qaConfigGet().then((c) => {
      setConfig(c);
      setSaved(c);
    });
  }, []);

  const dirty = JSON.stringify(config) !== JSON.stringify(saved);
  const set = <K extends keyof QaConfig>(k: K, v: QaConfig[K]) => config && setConfig({ ...config, [k]: v });

  async function save() {
    if (!config) return;
    try {
      await qaConfigSet(config);
      setSaved(config);
      setError(undefined);
      setNote("Saved.");
    } catch (e) {
      setError(errMessage(e));
    }
  }

  async function updateScrcpy() {
    setNote("Checking scrcpy…");
    try {
      const bin = await binaryUpdate();
      setNote(`scrcpy ${bin.version} (latest)`);
    } catch (e) {
      setNote(errMessage(e));
    }
  }

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto flex max-w-3xl flex-col gap-4 p-6">
        <h1 className="text-xl font-semibold text-zinc-100">Settings</h1>

        {config && (
          <Section title="QA and AI" desc="Used by every run. Check setup below after changing them.">
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <div className="sm:col-span-2">
                <Field label="QA skill folder" hint="Your telkomsel-qa-skills checkout">
                  <input className={INPUT} value={config.skillDir} onChange={(e) => set("skillDir", e.target.value)} />
                </Field>
              </div>
              <Field label="Python">
                <input className={INPUT} value={config.python} onChange={(e) => set("python", e.target.value)} />
              </Field>
              <Field label="Evidence folder" hint="Where runs save screenshots, videos and reports">
                <input className={INPUT} value={config.artifactsDir} onChange={(e) => set("artifactsDir", e.target.value)} />
              </Field>
              <Field label="Phones in parallel">
                <input
                  className={INPUT}
                  type="number"
                  min={1}
                  value={config.maxParallel}
                  onChange={(e) => set("maxParallel", Number(e.target.value) || 1)}
                />
              </Field>
              <Field label="Job timeout (seconds)">
                <input
                  className={INPUT}
                  type="number"
                  min={60}
                  value={config.timeoutSecs}
                  onChange={(e) => set("timeoutSecs", Number(e.target.value) || 3600)}
                />
              </Field>
              <Field label="AI command" hint="The Claude Code CLI">
                <input className={INPUT} value={config.aiCommand} onChange={(e) => set("aiCommand", e.target.value)} />
              </Field>
              <Field label="AI model" hint="For example claude-sonnet-5-5">
                <input className={INPUT} value={config.aiModel} onChange={(e) => set("aiModel", e.target.value)} />
              </Field>
              <Field label="Max AI actions per goal">
                <input
                  className={INPUT}
                  type="number"
                  min={5}
                  value={config.aiMaxActions}
                  onChange={(e) => set("aiMaxActions", Number(e.target.value) || 80)}
                />
              </Field>
            </div>
            <div className="mt-4 flex items-center gap-3">
              <Button variant="primary" disabled={!dirty} onClick={save}>
                Save
              </Button>
              {dirty && (
                <Button variant="ghost" onClick={() => setConfig(saved)}>
                  Discard
                </Button>
              )}
              {error && <span className="text-xs text-red-400">{error}</span>}
            </div>
          </Section>
        )}

        <Section title="Setup check" desc="Checks each thing a run needs and says how to fix what is missing.">
          <SetupCheck />
        </Section>

        <Section title="Audit log" desc="Runs, plans, AI investigations, exports and setting changes.">
          <Button
            variant="ghost"
            onClick={() => (audit ? setAudit(undefined) : qaEvents().then(setAudit).catch((e) => setError(errMessage(e))))}
          >
            {audit ? "Hide" : "Show the last 300 entries"}
          </Button>
          {audit && (
            <div className="mt-2 max-h-96 overflow-y-auto">
              <table className="w-full text-left font-mono text-xs text-zinc-400">
                <tbody>
                  {audit.map((e, i) => (
                    <tr key={i} className="border-t border-zinc-800 align-top">
                      <td className="whitespace-nowrap py-1 pr-3">{new Date(e.at * 1000).toLocaleString()}</td>
                      <td className="pr-3 text-zinc-200">{e.kind}</td>
                      <td className="whitespace-nowrap pr-3">{e.jobId ? `J${e.jobId}` : (e.runId ?? "")}</td>
                      <td className="break-all">{e.detail}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </Section>

        <Section title="Updates and about">
          <div className="flex flex-wrap gap-2">
            <Button onClick={updateScrcpy}>Update scrcpy</Button>
            <Button variant="ghost" onClick={() => openUrl(`${APP_REPO}/releases`)}>
              scrcpy-studio releases ↗
            </Button>
            <Button variant="ghost" onClick={() => openUrl(APP_REPO)}>
              About scrcpy-studio ↗
            </Button>
            <Button variant="ghost" onClick={() => openUrl(SCRCPY_REPO)}>
              About scrcpy ↗
            </Button>
          </div>
        </Section>

        {note && <p className="text-xs text-zinc-500">{note}</p>}
      </div>
    </div>
  );
}
