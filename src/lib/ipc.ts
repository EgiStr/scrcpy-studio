// Typed wrappers around Tauri's invoke / event APIs.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Binaries, Device, ScrcpyArgs, SessionInfo } from "./types";

// ---- Binary management ----
export const binaryCurrent = () => invoke<Binaries | null>("binary_current");
export const binaryLatestVersion = () => invoke<string>("binary_latest_version");
export const binaryInstallLatest = () => invoke<Binaries>("binary_install_latest");
export const binaryUpdate = () => invoke<Binaries>("binary_update");

// ---- adb ----
export const listDevices = () => invoke<Device[]>("list_devices");
export const adbConnect = (addr: string) => invoke<string>("adb_connect", { addr });
export const adbDisconnect = (addr: string) => invoke<string>("adb_disconnect", { addr });
export const adbPair = (addr: string, code: string) =>
  invoke<string>("adb_pair", { addr, code });
export const adbTcpip = (serial: string, port: number) =>
  invoke<string>("adb_tcpip", { serial, port });
export const goWireless = (serial: string) => invoke<string>("go_wireless", { serial });

// ---- QR wireless pairing ----
export interface QrSession {
  payload: string;
  name: string;
  code: string;
}
export interface QrPairResult {
  success: boolean;
  message: string;
}
export const qrPairStart = (autoConnect: boolean) =>
  invoke<QrSession>("qr_pair_start", { autoConnect });
export const onQrPairResult = (cb: (e: QrPairResult) => void): Promise<UnlistenFn> =>
  listen<QrPairResult>("qr-pair-result", (event) => cb(event.payload));

// ---- scrcpy launch / sessions ----
export const previewArgv = (args: ScrcpyArgs) => invoke<string>("preview_argv", { args });
export const launch = (args: ScrcpyArgs) => invoke<SessionInfo>("launch", { args });
export const stopSession = (id: number) => invoke<void>("stop_session", { id });
export const listSessions = () => invoke<SessionInfo[]>("list_sessions");
export const scrcpyList = (kind: string, serial?: string) =>
  invoke<string>("scrcpy_list", { kind, serial });

// ---- app launcher ----
export interface DeviceApp {
  name: string;
  package: string;
  system: boolean;
}
export const listApps = (serial: string | undefined, includeSystem: boolean) =>
  invoke<DeviceApp[]>("list_apps", { serial, includeSystem });

export interface ShortcutResult {
  path: string;
  iconApplied: boolean;
  warning?: string | null;
}
export const createShortcut = (args: ScrcpyArgs, pkg: string, label: string, iconUrl?: string) =>
  invoke<ShortcutResult>("create_shortcut", { args, package: pkg, label, iconUrl });

export const iconWeb = (pkg: string) => invoke<string | null>("icon_web", { package: pkg });

export const pullApk = (serial: string, pkg: string, dest: string) =>
  invoke<string[]>("pull_apk", { serial, package: pkg, dest });

// ---- device control ----
export const deviceAction = (serial: string, action: string) =>
  invoke<void>("device_action", { serial, action });
export const deviceScreenshot = (serial: string, stamp: number) =>
  invoke<string>("device_screenshot", { serial, stamp: String(stamp) });

export interface WinRect {
  x: number;
  y: number;
  w: number;
  h: number;
}
export const scrcpyWindowRect = (title?: string) =>
  invoke<WinRect | null>("scrcpy_window_rect", { title });

export const fitWindow = (width: number, height: number) =>
  invoke<void>("fit_window", { width, height });

export const rotateScrcpy = (title?: string) => invoke<void>("rotate_scrcpy", { title });

// ---- profiles ----
export const profileSave = (name: string, args: ScrcpyArgs) =>
  invoke<void>("profile_save", { name, args });
export const profileLoad = (name: string) => invoke<ScrcpyArgs>("profile_load", { name });
export const profileList = () => invoke<string[]>("profile_list");
export const profileDelete = (name: string) => invoke<void>("profile_delete", { name });

// ---- emulator fleet ----
export interface Sdk {
  root: string;
  emulator: string;
  adb?: string | null;
}
export type EmulatorState =
  | "stopped"
  | "starting"
  | "booting"
  | "ready"
  | "stopping"
  | "offline"
  | "failed";
export interface EmulatorInfo {
  name: string;
  api?: string | null;
  device?: string | null;
  ram?: string | null;
  resolution?: string | null;
  state: EmulatorState;
  serial?: string | null;
  managed: boolean;
  detail?: string | null;
}
export const emuSdk = () => invoke<Sdk | null>("emu_sdk");
export const emuList = () => invoke<EmulatorInfo[]>("emu_list");
export const emuStart = (avd: string, headless: boolean) =>
  invoke<string>("emu_start", { avd, headless });
export const emuStop = (avd: string) => invoke<void>("emu_stop", { avd });

// ---- parallel QA ----
export interface QaConfig {
  skillDir: string;
  python: string;
  artifactsDir: string;
  maxParallel: number;
  timeoutSecs: number;
  aiCommand: string;
  aiModel: string;
  aiMaxActions: number;
}
export type QaStatus =
  | "queued"
  | "running"
  | "passed"
  | "failed"
  | "blocked"
  | "error"
  | "cancelled";
export interface QaJob {
  id: number;
  runId: string;
  scenario: string;
  testset: string;
  only?: string | null;
  kind: "script" | "agent";
  goal?: PlanItem | null;
  devices: string[];
  actions: number;
  status: QaStatus;
  serial?: string | null;
  evidenceDir?: string | null;
  exitCode?: number | null;
  note?: string | null;
  queuedAt: number;
  startedAt?: number | null;
  finishedAt?: number | null;
}
export interface QaLease {
  serial: string;
  jobId: number;
  runId: string;
  acquiredAt: number;
}
export const qaConfigGet = () => invoke<QaConfig>("qa_config_get");
export const qaConfigSet = (config: QaConfig) => invoke<void>("qa_config_set", { config });
export const qaTestsets = () => invoke<string[]>("qa_testsets");
export interface QaScenario {
  testset: string;
  only?: string;
  goal?: PlanItem;
  devices?: string[];
}
export const qaSubmit = (scenarios: QaScenario[]) =>
  invoke<string>("qa_submit", { scenarios });
export const qaState = () => invoke<{ jobs: QaJob[]; leases: QaLease[] }>("qa_state");
export const qaCancel = (id: number) => invoke<void>("qa_cancel", { id });
export const qaLog = (id: number) => invoke<string>("qa_log", { id });
export interface AuditEvent {
  at: number;
  kind: string;
  jobId?: number | null;
  runId?: string | null;
  detail: string;
}
export interface SetupCheck {
  name: string;
  ok: boolean;
  detail: string;
  fix?: string | null;
}
export const qaDoctor = () => invoke<SetupCheck[]>("qa_doctor");
export const qaEvents = () => invoke<AuditEvent[]>("qa_events");

export type DefectCategory =
  | "ui-defect"
  | "navigation-defect"
  | "business-rule-defect"
  | "app-crash"
  | "automation-error"
  | "infrastructure-issue"
  | "inconclusive";
export interface Finding {
  id: string;
  runId: string;
  jobId: number;
  scenario: string;
  caseId: string;
  verdict: string;
  category: DefectCategory;
  reason: string;
  summary: string;
  expected: string[];
  actual: string;
  missed: string[];
  steps: string[];
  evidence: string[];
  evidenceDir?: string | null;
  serial?: string | null;
  device: string;
  appVersion: string;
}
export interface CaseChange {
  jobId: number;
  caseId: string;
  key: string;
  now: string;
  before?: string | null;
  beforeRun?: string | null;
  change: "new" | "regressed" | "fixed" | "still failing" | "still passing" | "inconclusive";
}
export interface Hypothesis {
  cause: string;
  confidence: number;
  supporting: string[];
  against: string[];
}
export interface Investigation {
  findingId: string;
  hypotheses: Hypothesis[];
  limitations: string[];
  nextSteps: string[];
  model: string;
  costUsd?: number | null;
  at: number;
  note: string;
}
export const qaFindings = (runId: string) =>
  invoke<{ findings: Finding[]; changes: CaseChange[] }>("qa_findings", { runId });
export const qaExport = (runId: string) => invoke<string>("qa_export", { runId });
export const qaInvestigate = (jobId: number, caseId: string) =>
  invoke<Investigation>("qa_investigate", { jobId, caseId });
export const qaTrace = (id: number) => invoke<TraceEvent[]>("qa_trace", { id });

// ---- AI agent ----
export interface PlanItem {
  kind: "testset" | "goal";
  testset?: string | null;
  only?: string | null;
  id?: string | null;
  topic?: string | null;
  goal?: string | null;
  keypoints: string[];
  expected?: string | null;
  why?: string | null;
}
export interface ScenarioDraft {
  name: string;
  yaml: string;
  costUsd?: number | null;
}
export interface ScenarioCase {
  id: string;
  surface: string;
  question: string;
  topic: string;
}
export const aiScenario = (objective: string) => invoke<ScenarioDraft>("ai_scenario", { objective });
export const qaScenarioCheck = (yaml: string) =>
  invoke<{ name: string; cases: ScenarioCase[] }>("qa_scenario_check", { yaml });
export const qaScenarioSave = (yaml: string) => invoke<string>("qa_scenario_save", { yaml });
export interface Plan {
  items: PlanItem[];
  rejected: string[];
  costUsd?: number | null;
}
export interface TraceEvent {
  kind: "say" | "tool" | "result" | "done";
  name?: string | null;
  detail: string;
  error: boolean;
}
export const aiPlan = (objective: string) => invoke<Plan>("ai_plan", { objective });

// ---- USB monitoring ----
export interface UsbDevice {
  serial: string;
  state: string;
  connection: "usb" | "emulator" | "wifi";
  model?: string | null;
  android?: string | null;
  battery?: number | null;
  temperatureC?: number | null;
  foreground?: string | null;
}
export const monDevices = () => invoke<UsbDevice[]>("mon_devices");
export const monScreen = (serial: string) => invoke<string>("mon_screen", { serial });

// ---- events ----
export interface SessionLog {
  id: number;
  stream: "stdout" | "stderr";
  line: string;
}
export interface SessionExit {
  id: number;
  code: number | null;
}

export const onSessionLog = (cb: (e: SessionLog) => void): Promise<UnlistenFn> =>
  listen<SessionLog>("session-log", (event) => cb(event.payload));

export const onSessionExit = (cb: (e: SessionExit) => void): Promise<UnlistenFn> =>
  listen<SessionExit>("session-exit", (event) => cb(event.payload));

// Normalize a thrown command error (the Rust AppError) into a readable string.
export function errMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object") {
    const obj = e as { kind?: string; message?: string };
    if (obj.message) return obj.kind ? `${obj.kind}: ${obj.message}` : obj.message;
  }
  return String(e);
}
