//! Parallel QA execution: a job queue, exclusive device leases, and one worker process per
//! device. A worker is the existing Telkomsel QA Skill runner (`run.py`), given an explicit
//! serial and its own evidence directory.

use crate::agent::{AgentConfig, Guard, PlanItem};
use crate::error::{AppError, Result};
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ---- configuration ----

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QaConfig {
    /// The telkomsel-qa-skills checkout, or its `skills/mytelkomsel-qa` directory.
    pub skill_dir: String,
    pub python: String,
    pub artifacts_dir: String,
    pub max_parallel: usize,
    pub timeout_secs: u64,
    /// The model CLI (Claude Code's headless protocol) and model the AI agent uses.
    pub ai_command: String,
    pub ai_model: String,
    /// Tool calls one agent job may make before it is stopped.
    pub ai_max_actions: u32,
    /// AI tests one physical phone runs at once, each on its own hidden display (0: one test
    /// on the phone's own screen; at most 2, MyTelkomsel's own limit).
    pub phone_sessions: usize,
    /// Give jobs to physical phones before emulators.
    pub physical_first: bool,
}

impl Default for QaConfig {
    fn default() -> Self {
        Self {
            skill_dir: String::new(),
            python: "python".into(),
            artifacts_dir: String::new(),
            max_parallel: 2,
            timeout_secs: 3600,
            ai_command: "claude".into(),
            ai_model: "claude-sonnet-5-5".into(),
            ai_max_actions: 80,
            phone_sessions: 2,
            physical_first: true,
        }
    }
}

impl QaConfig {
    pub fn load(data_dir: &Path) -> Self {
        let mut cfg: Self = std::fs::read_to_string(data_dir.join("qa.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        if cfg.artifacts_dir.is_empty() {
            cfg.artifacts_dir = data_dir.join("artifacts").to_string_lossy().to_string();
        }
        cfg
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(|e| AppError::Io(e.to_string()))?;
        Ok(std::fs::write(data_dir.join("qa.json"), text)?)
    }

    pub(crate) fn skill_root(&self) -> Result<PathBuf> {
        let base = PathBuf::from(&self.skill_dir);
        [base.clone(), base.join("skills").join("mytelkomsel-qa")]
            .into_iter()
            .find(|d| d.join("scripts").join("run.py").is_file())
            .ok_or_else(|| {
                AppError::NotFound(format!("scripts/run.py under \"{}\" (set the QA skill folder)", self.skill_dir))
            })
    }

    /// The AI agent's settings. Needs the skill checkout's `mcp/telkomsel-qa-mcp/serve.py`.
    pub fn agent(&self) -> Result<AgentConfig> {
        let root = self.skill_root()?;
        let server = root
            .ancestors()
            .map(|d| d.join("mcp").join("telkomsel-qa-mcp").join("serve.py"))
            .find(|p| p.is_file())
            .ok_or_else(|| AppError::NotFound("mcp/telkomsel-qa-mcp/serve.py in the QA skill checkout".into()))?;
        Ok(AgentConfig {
            command: self.ai_command.clone(),
            model: self.ai_model.clone(),
            python: PathBuf::from(&self.python),
            skill_root: root,
            mcp_server: server,
            max_actions: self.ai_max_actions.max(1),
        })
    }

    /// Test set files the skill ships, as absolute paths.
    pub fn testsets(&self) -> Result<Vec<String>> {
        let mut sets: Vec<String> = std::fs::read_dir(self.skill_root()?.join("testsets"))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml" || x == "csv"))
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        sets.sort();
        Ok(sets)
    }

    pub fn run_py(&self) -> Result<PathBuf> {
        Ok(self.skill_root()?.join("scripts").join("run.py"))
    }

    fn settings(&self, adb: &Path) -> Result<Settings> {
        let run_py = self.run_py()?;
        let adb = adb.to_path_buf();
        let probe = adb.clone();
        let scrcpy = adb.with_file_name(if cfg!(windows) { "scrcpy.exe" } else { "scrcpy" });
        // A run.py that knows sessions goes back to its first screen instead of restarting the
        // app; an older one would end the phone's other session, so its jobs take the whole phone.
        let scripts = std::fs::read_to_string(&run_py).is_ok_and(|t| t.contains("act_back_to_root"));
        let (python, lister) = (PathBuf::from(&self.python), run_py.clone());
        Ok(Settings {
            cases: Arc::new(move |testset, only| list_cases(&python, &lister, testset, only)),
            sessions: SessionSettings {
                per_phone: self.phone_sessions.min(2),
                physical_first: self.physical_first,
                scripts,
                scrcpy: scrcpy.is_file().then_some(scrcpy),
                adb: Some(adb.clone()),
            },
            runner: Runner {
                program: PathBuf::from(&self.python),
                args: vec![run_py.to_string_lossy().to_string()],
                adb_dir: adb.parent().map(Path::to_path_buf),
            },
            artifacts: PathBuf::from(&self.artifacts_dir),
            max_parallel: self.max_parallel.max(1),
            timeout: Duration::from_secs(self.timeout_secs),
            devices: Arc::new(move || ready_devices(&probe)),
            agent: self.agent().ok(),
        })
    }
}

/// One phone, however many ways adb reaches it: a phone on USB and wireless debugging at once
/// is listed twice by adb but runs one test at a time (or its sessions) like any other phone.
#[derive(Debug, Clone, PartialEq)]
pub struct Phone {
    /// `ro.serialno` for a physical phone, the adb serial for an emulator.
    pub id: String,
    /// The adb serials that reach it, USB first.
    pub serials: Vec<String>,
}

impl Phone {
    #[cfg(test)]
    pub fn one(serial: &str) -> Self {
        Self { id: serial.into(), serials: vec![serial.into()] }
    }

    /// Is `name` this phone, by its id or one of its serials? (Jobs pin devices by serial.)
    fn is(&self, name: &str) -> bool {
        self.id == name || self.serials.iter().any(|s| s == name)
    }

    fn physical(&self) -> bool {
        self.serials.first().is_some_and(|s| is_physical(s))
    }
}

/// Devices that are online and have finished booting, one entry per phone.
fn ready_devices(adb: &Path) -> Vec<Phone> {
    let serials: Vec<String> = crate::adb::devices(adb)
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.state == "device" && crate::adb::boot_completed(adb, &d.serial))
        .map(|d| d.serial)
        .collect();
    group_phones(serials.into_iter().map(|s| {
        let id = phone_id(adb, &s);
        (s, id)
    }))
}

/// The phone behind a serial: its `ro.serialno`, the same over USB and wireless. Asked once
/// per serial; an emulator, or a phone that does not answer, is its own serial.
pub(crate) fn phone_id(adb: &Path, serial: &str) -> String {
    static IDS: std::sync::OnceLock<Mutex<HashMap<String, String>>> = std::sync::OnceLock::new();
    let ids = IDS.get_or_init(Default::default);
    if !is_physical(serial) {
        return serial.to_string();
    }
    if let Some(id) = ids.lock().unwrap().get(serial) {
        return id.clone();
    }
    match crate::adb::shell(adb, serial, "getprop ro.serialno").map(|o| o.trim().to_string()) {
        Ok(id) if !id.is_empty() => {
            ids.lock().unwrap().insert(serial.to_string(), id.clone());
            id
        }
        _ => serial.to_string(),
    }
}

fn group_phones(pairs: impl IntoIterator<Item = (String, String)>) -> Vec<Phone> {
    let mut phones: Vec<Phone> = Vec::new();
    for (serial, id) in pairs {
        match phones.iter_mut().find(|p| p.id == id) {
            Some(p) => p.serials.push(serial),
            None => phones.push(Phone { id, serials: vec![serial] }),
        }
    }
    for p in &mut phones {
        p.serials.sort_by_key(|s| crate::monitor::connection(s) != "usb");
    }
    phones
}

/// One case of a test set, as `run.py --list` prints it.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseInfo {
    pub id: String,
    pub surface: String,
    pub repeat: u32,
    /// The case whose chat this one continues; it runs right after it, in the same job.
    pub follows: Option<String>,
}

impl CaseInfo {
    /// Does it open Veronika's chat? The goal surface may; count it as if it does.
    fn chat(&self) -> bool {
        self.surface == "veronika" || self.surface == "goal"
    }
}

/// `run.py --list` lines: `ID METHOD SURFACE [TYPE] xN  [<- PARENT  ]QUESTION`. Other lines
/// (the M1 sample and the totals) are skipped.
fn parse_list(text: &str) -> Vec<CaseInfo> {
    let is_method = |t: &str| t.len() == 2 && t.starts_with('m') && t[1..].chars().all(|c| c.is_ascii_digit());
    let is_repeat = |t: &str| t.len() > 1 && t.starts_with('x') && t[1..].chars().all(|c| c.is_ascii_digit());
    text.lines()
        .filter_map(|line| {
            let t: Vec<&str> = line.split_whitespace().collect();
            if t.len() < 4 || !is_method(t[1]) {
                return None;
            }
            let at = (3..t.len().min(5)).find(|&i| is_repeat(t[i]))?;
            Some(CaseInfo {
                id: t[0].to_string(),
                surface: t[2].to_string(),
                repeat: t[at][1..].parse().unwrap_or(1),
                follows: (t.get(at + 1) == Some(&"<-")).then(|| t.get(at + 2).map(|p| p.to_string())).flatten(),
            })
        })
        .collect()
}

/// The cases `run.py` would run for a test set (after its M1 sample), or None when it cannot
/// list them. Never touches a device.
fn list_cases(python: &Path, run_py: &Path, testset: &str, only: Option<&str>) -> Option<Vec<CaseInfo>> {
    let mut cmd = Command::new(python);
    cmd.arg(run_py).arg(testset).arg("--list");
    if let Some(only) = only {
        cmd.args(["--only", only]);
    }
    cmd.current_dir(std::env::temp_dir()).env("PYTHONIOENCODING", "utf-8").stdin(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd.output().ok().filter(|o| o.status.success())?;
    Some(parse_list(&String::from_utf8_lossy(&out.stdout)))
}

/// Split a test set's cases into at most `k_chat` jobs that open Veronika (a phone has one
/// Veronika, so chat jobs on one phone run one at a time) and native-screen jobs that fill the
/// rest of the `k_total` places. A follow-up chain stays in one job, in order. Each pool is
/// balanced by longest-first (LPT) on its runs; ids keep the test set's order within a job.
fn shard(cases: &[CaseInfo], k_chat: usize, k_total: usize) -> Vec<(Vec<String>, bool)> {
    // (case indexes, runs, opens Veronika)
    let mut chains: Vec<(Vec<usize>, u32, bool)> = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        match chains.last_mut() {
            Some(last) if c.follows.is_some() => {
                last.0.push(i);
                last.1 += c.repeat.max(1);
                last.2 |= c.chat();
            }
            _ => chains.push((vec![i], c.repeat.max(1), c.chat())),
        }
    }
    let lpt = |mut pool: Vec<(Vec<usize>, u32, bool)>, k: usize| -> Vec<Vec<usize>> {
        if pool.is_empty() {
            return Vec::new();
        }
        let k = k.clamp(1, pool.len());
        pool.sort_by_key(|c| std::cmp::Reverse(c.1));
        let mut bins: Vec<(u32, Vec<usize>)> = vec![(0, Vec::new()); k];
        for (idx, runs, _) in pool {
            let bin = bins.iter_mut().min_by_key(|b| b.0).unwrap();
            bin.0 += runs;
            bin.1.extend(idx);
        }
        bins.into_iter().map(|(_, mut b)| {
            b.sort_unstable();
            b
        }).collect()
    };
    let (chat, native): (Vec<_>, Vec<_>) = chains.into_iter().partition(|c| c.2);
    let chat_bins = lpt(chat, k_chat);
    let native_k = if chat_bins.is_empty() { k_total } else { k_total.saturating_sub(chat_bins.len()).max(1) };
    let ids = |b: Vec<usize>| b.into_iter().map(|i| cases[i].id.clone()).collect::<Vec<_>>();
    let mut out: Vec<(Vec<String>, bool)> = chat_bins.into_iter().map(|b| (ids(b), true)).collect();
    out.extend(lpt(native, native_k).into_iter().map(|b| (ids(b), false)));
    out
}

// ---- jobs ----

/// Where a run's jobs go on a physical phone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Hidden sessions on physical phones when they are on, else the whole device.
    #[default]
    Auto,
    /// The phone's own screen, one job per phone (to watch it, or mirror it).
    Screen,
    /// Hidden sessions on physical phones only.
    Hidden,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Scenario {
    /// Absolute path of a test set file.
    pub testset: String,
    /// Comma-separated case ids (`run.py --only`).
    pub only: Option<String>,
    /// A goal the AI agent drives toward instead of `run.py` (the test set is then unused).
    #[serde(default)]
    pub goal: Option<PlanItem>,
    /// Serials this scenario may run on; empty means any free device.
    #[serde(default)]
    pub devices: Vec<String>,
    #[serde(default)]
    pub mode: Mode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Running,
    Passed,
    Failed,
    Blocked,
    Error,
    Cancelled,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Running => "running",
            Status::Passed => "passed",
            Status::Failed => "failed",
            Status::Blocked => "blocked",
            Status::Error => "error",
            Status::Cancelled => "cancelled",
        }
    }

    fn is_final(self) -> bool {
        !matches!(self, Status::Queued | Status::Running)
    }

    /// The skill's exit codes: 0 all passed, 1 failures, 3 blocked; anything else is a
    /// setup or infrastructure error, never a pass.
    fn from_exit(code: Option<i32>) -> Self {
        match code {
            Some(0) => Status::Passed,
            Some(1) => Status::Failed,
            Some(3) => Status::Blocked,
            _ => Status::Error,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: u32,
    pub run_id: String,
    pub scenario: String,
    pub testset: String,
    pub only: Option<String>,
    /// "script" (`run.py`) or "agent" (the AI drives).
    pub kind: String,
    pub goal: Option<PlanItem>,
    /// Serials the job may run on; empty means any free device.
    #[serde(default)]
    pub devices: Vec<String>,
    /// Tool calls the agent has made so far.
    pub actions: u32,
    pub status: Status,
    pub serial: Option<String>,
    /// The hidden display the job ran on; None for the phone's own screen.
    #[serde(default)]
    pub display: Option<u32>,
    #[serde(default)]
    pub mode: Mode,
    /// It opens Veronika, so it never runs beside another such job on the same phone.
    #[serde(default)]
    pub veronika: bool,
    /// One test set split across places: the jobs of a group share it and their results are
    /// merged into `<run>/merged-<group>/run.json` when the run ends.
    #[serde(default)]
    pub group: Option<String>,
    pub evidence_dir: Option<String>,
    pub exit_code: Option<i32>,
    pub note: Option<String>,
    pub queued_at: u64,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lease {
    /// The phone (`Phone::id`) the place is on.
    pub phone: String,
    /// The adb serial the job reaches it by.
    pub serial: String,
    /// The job opens Veronika (one per phone).
    pub veronika: bool,
    /// The session (1 or 2) on that phone, or None for the whole device.
    pub slot: Option<u8>,
    pub job_id: u32,
    pub run_id: String,
    pub acquired_at: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QaState {
    pub jobs: Vec<Job>,
    pub leases: Vec<Lease>,
}

fn slug(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect()
}

fn scenario_label(s: &Scenario) -> String {
    if let Some(g) = &s.goal {
        return format!("goal:{}", g.id.as_deref().unwrap_or("AI"));
    }
    let stem = Path::new(&s.testset)
        .file_stem()
        .map(|x| x.to_string_lossy().to_string())
        .unwrap_or_else(|| s.testset.clone());
    match s.only.as_deref().map(str::trim).filter(|o| !o.is_empty()) {
        Some(only) => format!("{stem}:{only}"),
        None => stem,
    }
}

/// `run.py` exits 1 for "some failed or errored". Its run.json says which: a run whose
/// cases only errored never reached a product verdict, so it is an ERROR, not a FAIL.
fn refine(status: Status, evidence_dir: &Path) -> Status {
    if status != Status::Failed {
        return status;
    }
    let verdicts: Vec<String> = std::fs::read_to_string(evidence_dir.join("run.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["cases"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|c| c["verdict"].as_str().map(str::to_string))
        .collect();
    let only_errors = verdicts.iter().any(|v| v == "error") && verdicts.iter().all(|v| v == "error" || v == "pass");
    if only_errors { Status::Error } else { Status::Failed }
}

/// Where a job runs on its device.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Slot {
    /// The whole device: its own screen, nothing else running on it.
    Whole,
    /// Session 1 or 2 on a physical phone, each on its own hidden display.
    Session(u8),
}

fn lease_key(phone: &str, slot: Slot) -> String {
    match slot {
        Slot::Whole => phone.to_string(),
        Slot::Session(n) => format!("{phone}#{n}"),
    }
}

/// A queued job as the scheduler sees it.
pub struct Want {
    pub id: u32,
    /// Devices it may run on; empty means any.
    pub allowed: Vec<String>,
    /// It can run on a hidden display: AI jobs, and scripted ones when `run.py` knows sessions.
    pub session: bool,
    /// It opens Veronika, of which a phone has one: opening it in one session takes it from the other.
    pub veronika: bool,
    pub mode: Mode,
}

fn is_physical(serial: &str) -> bool {
    crate::monitor::connection(serial) != "emulator"
}

/// Pair queued jobs (oldest first) with a free place to run. A leased place is never handed
/// out, and a job waiting for its own device does not hold up later jobs.
///
/// With sessions on, a physical phone runs jobs on hidden displays, never on its own screen
/// (unless the job asks for the screen): session 1 only when nothing runs on the phone (it
/// restarts the app), session 2 only beside a running session 1 (MyTelkomsel makes its second
/// task from the first). Two jobs that open Veronika never share a phone. A job that cannot
/// run in a session takes the whole phone.
fn assign(queued: &[Want], ready: &[Phone], leased: &HashMap<String, Lease>, slots: usize, per_phone: usize, physical_first: bool) -> Vec<(u32, String, Slot)> {
    let mut taken: std::collections::HashSet<String> = leased.keys().cloned().collect();
    let mut chatting: std::collections::HashSet<String> = leased.values().filter(|l| l.veronika).map(|l| l.phone.clone()).collect();
    let mut devices: Vec<&Phone> = ready.iter().collect();
    if physical_first {
        devices.sort_by_key(|p| !p.physical());
    }
    let mut out = Vec::new();
    for want in queued {
        if out.len() >= slots {
            break;
        }
        let pick = devices.iter().filter(|p| want.allowed.is_empty() || want.allowed.iter().any(|a| p.is(a))).find_map(|p| {
            let s = &p.id;
            if want.veronika && chatting.contains(s) {
                return None;
            }
            let has = |slot| taken.contains(&lease_key(s, slot));
            let busy = has(Slot::Whole) || has(Slot::Session(1)) || has(Slot::Session(2));
            let slot = if hidden_ok(want.session, want.mode, per_phone, p) {
                if !busy {
                    Some(Slot::Session(1))
                } else if per_phone >= 2 && has(Slot::Session(1)) && !has(Slot::Session(2)) {
                    Some(Slot::Session(2))
                } else {
                    None
                }
            } else if want.mode != Mode::Hidden && !busy {
                Some(Slot::Whole)
            } else {
                None
            };
            slot.map(|slot| (s.clone(), slot))
        });
        if let Some((phone, slot)) = pick {
            taken.insert(lease_key(&phone, slot));
            if want.veronika {
                chatting.insert(phone.clone());
            }
            out.push((want.id, phone, slot));
        }
    }
    out
}

/// Does a job run on a hidden display on this phone?
fn hidden_ok(session: bool, mode: Mode, per_phone: usize, p: &Phone) -> bool {
    session && mode != Mode::Screen && per_phone > 0 && p.physical()
}

/// How many jobs of a scenario can run at once on these phones: (Veronika jobs, all jobs).
fn capacity(phones: &[Phone], allowed: &[String], session: bool, mode: Mode, per_phone: usize) -> (usize, usize) {
    let mut chat = 0;
    let mut places = 0;
    for p in phones.iter().filter(|p| allowed.is_empty() || allowed.iter().any(|a| p.is(a))) {
        let hidden = hidden_ok(session, mode, per_phone, p);
        if mode == Mode::Hidden && !hidden {
            continue;
        }
        chat += 1;
        places += if hidden { per_phone } else { 1 };
    }
    (chat, places)
}

/// A job a scenario becomes.
struct Planned {
    only: Option<String>,
    label: String,
    veronika: bool,
    group: Option<String>,
}

/// A scripted scenario over a whole test set is split across the places it can use (see
/// `shard`); anything else is one job. `index` makes the group unique within the run.
fn plan(settings: &Settings, s: &Scenario, index: usize, phones: &mut Option<Vec<Phone>>) -> Vec<Planned> {
    let only = s.only.as_deref().map(str::trim).filter(|o| !o.is_empty());
    let one = |veronika| vec![Planned { only: only.map(str::to_string), label: scenario_label(s), veronika, group: None }];
    if s.goal.is_some() {
        return one(true);
    }
    let Some(cases) = (settings.cases)(&s.testset, only) else { return one(true) };
    let veronika = cases.iter().any(CaseInfo::chat);
    if only.is_some() || cases.len() < 2 {
        return one(veronika);
    }
    let phones = phones.get_or_insert_with(|| (settings.devices)());
    let sessions = &settings.sessions;
    let (chat, places) = capacity(phones, &s.devices, sessions.scripts, s.mode, sessions.per_phone());
    let cap = settings.max_parallel.max(1);
    let bins = shard(&cases, chat.min(cap), places.min(cap));
    if places.min(cap) < 2 || bins.len() < 2 {
        return one(veronika);
    }
    let stem = scenario_label(s);
    let n = bins.len();
    bins.into_iter()
        .enumerate()
        .map(|(i, (ids, veronika))| Planned {
            only: Some(ids.join(",")),
            label: format!("{stem} [{}/{n}]", i + 1),
            veronika,
            group: Some(format!("{stem}-{}", index + 1)),
        })
        .collect()
}

// ---- engine ----

/// The command a worker runs; the scenario, `--out` and `-s` arguments are appended.
#[derive(Clone)]
pub struct Runner {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Put first on the worker's PATH so its `adb` is the app's adb (one adb server version).
    pub adb_dir: Option<PathBuf>,
}

#[derive(Clone)]
pub struct Settings {
    pub runner: Runner,
    /// A test set's cases (`run.py --list`), to split it across places; None: run it whole.
    pub cases: Arc<dyn Fn(&str, Option<&str>) -> Option<Vec<CaseInfo>> + Send + Sync>,
    pub artifacts: PathBuf,
    pub max_parallel: usize,
    pub timeout: Duration,
    pub devices: Arc<dyn Fn() -> Vec<Phone> + Send + Sync>,
    /// None when the skill checkout has no MCP server: goal jobs then end as errors.
    pub agent: Option<AgentConfig>,
    pub sessions: SessionSettings,
}

#[derive(Clone, Default)]
pub struct SessionSettings {
    /// AI tests per physical phone, each on a hidden display (0: none, the whole phone).
    pub per_phone: usize,
    pub physical_first: bool,
    /// `run.py` runs in a session (it never restarts the shared app); else only AI jobs do.
    pub scripts: bool,
    /// The bundled scrcpy, which opens the displays; without it there are no sessions.
    pub scrcpy: Option<PathBuf>,
    pub adb: Option<PathBuf>,
}

impl SessionSettings {
    fn per_phone(&self) -> usize {
        if self.scrcpy.is_some() && self.adb.is_some() { self.per_phone } else { 0 }
    }
}

/// What the trace reader learns about a running agent job.
#[derive(Default)]
struct Meter {
    actions: AtomicU32,
    broken: Mutex<Option<String>>,
}

#[derive(Default)]
struct Inner {
    jobs: Vec<Job>,
    children: HashMap<u32, Child>,
    meters: HashMap<u32, Arc<Meter>>,
    /// Open session displays, by job.
    sessions: HashMap<u32, crate::vsession::Session>,
    /// View-only windows on a job's hidden display (Watch), by job.
    watchers: HashMap<u32, Child>,
    leases: HashMap<String, Lease>,
    /// Consecutive device checks a lease's serial was missing from, by lease key.
    missing: HashMap<String, u32>,
    ticks: u64,
    settings: Option<Settings>,
    /// Run history and audit log; None in tests that do not need them.
    store: Option<Store>,
    next_id: u32,
    next_run: u32,
    scheduling: bool,
}

#[derive(Clone, Default)]
pub struct QaEngine {
    inner: Arc<Mutex<Inner>>,
}

impl QaEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// An engine that keeps its jobs in `store`. Jobs a previous app session left queued
    /// or running get a final state: their workers died with that session.
    pub fn with_store(store: Store) -> Self {
        let mut jobs = store.jobs(500).unwrap_or_default();
        for job in jobs.iter_mut().filter(|j| !j.status.is_final()) {
            let (status, note) = if job.status == Status::Running {
                (Status::Error, "interrupted: the app closed while this job ran")
            } else {
                (Status::Cancelled, "the app closed before this job started")
            };
            job.status = status;
            job.note = Some(note.into());
            job.finished_at = Some(now());
            let _ = store.save_job(job);
            let _ = store.event(now(), "interrupted", Some(job.id), Some(&job.run_id), note);
        }
        let next_id = store.max_job_id().unwrap_or(0);
        let inner = Inner { jobs, store: Some(store), next_id, ..Default::default() };
        Self { inner: Arc::new(Mutex::new(inner)) }
    }

    /// Record a user or AI action in the audit log.
    pub fn audit(&self, kind: &str, run_id: Option<&str>, detail: &str) {
        let inner = self.inner.lock().unwrap();
        if let Some(store) = &inner.store {
            let _ = store.event(now(), kind, None, run_id, detail);
        }
    }

    /// The newest audit-log entries, newest first.
    pub fn events(&self, limit: u32) -> Vec<crate::store::Event> {
        let inner = self.inner.lock().unwrap();
        inner.store.as_ref().and_then(|s| s.events(limit).ok()).unwrap_or_default()
    }

    pub fn state(&self) -> QaState {
        let inner = self.inner.lock().unwrap();
        let mut leases: Vec<Lease> = inner.leases.values().cloned().collect();
        leases.sort_by_key(|l| l.job_id);
        QaState { jobs: inner.jobs.clone(), leases }
    }

    /// Queue one job per scenario as a single run and make sure the scheduler is running.
    pub fn submit(&self, cfg: &QaConfig, adb: &Path, scenarios: Vec<Scenario>) -> Result<String> {
        self.submit_with(cfg.settings(adb)?, scenarios)
    }

    pub fn submit_with(&self, settings: Settings, scenarios: Vec<Scenario>) -> Result<String> {
        if scenarios.is_empty() {
            return Err(AppError::InvalidArgs("no scenarios selected".into()));
        }
        let sessions = &settings.sessions;
        for s in scenarios.iter().filter(|s| s.mode == Mode::Hidden) {
            if sessions.per_phone() == 0 {
                return Err(AppError::InvalidArgs("hidden sessions are off: set AI tests per phone to 1 or 2 in Settings".into()));
            }
            if s.goal.is_none() && !sessions.scripts {
                return Err(AppError::InvalidArgs("this QA skill's run.py cannot run in a hidden session; update the skill or run on the phone's screen".into()));
            }
        }
        // Listing a test set's cases runs python and asking for devices runs adb: both before the lock.
        let mut phones = None;
        let planned: Vec<(Scenario, Vec<Planned>)> = scenarios
            .into_iter()
            .enumerate()
            .map(|(i, mut s)| {
                s.devices = s.devices.iter().map(|d| d.trim().to_string()).filter(|d| !d.is_empty()).collect();
                let p = plan(&settings, &s, i, &mut phones);
                (s, p)
            })
            .collect();
        let mut inner = self.inner.lock().unwrap();
        inner.next_run += 1;
        let run_id = format!("RUN-{}-{:02}", now(), inner.next_run);
        for (s, jobs) in planned {
            for p in jobs {
                inner.next_id += 1;
                let job = Job {
                    id: inner.next_id,
                    run_id: run_id.clone(),
                    scenario: p.label,
                    testset: s.testset.clone(),
                    only: p.only,
                    kind: if s.goal.is_some() { "agent" } else { "script" }.into(),
                    goal: s.goal.clone(),
                    devices: s.devices.clone(),
                    actions: 0,
                    status: Status::Queued,
                    serial: None,
                    display: None,
                    mode: s.mode,
                    veronika: p.veronika,
                    group: p.group,
                    evidence_dir: None,
                    exit_code: None,
                    note: None,
                    queued_at: now(),
                    started_at: None,
                    finished_at: None,
                };
                persist(&inner, &job);
                inner.jobs.push(job);
            }
        }
        let labels: Vec<String> = inner.jobs.iter().filter(|j| j.run_id == run_id).map(|j| format!("J{} {}", j.id, j.scenario)).collect();
        audit(&inner, "submit", None, Some(&run_id), &labels.join(", "));
        inner.settings = Some(settings);
        if !inner.scheduling {
            inner.scheduling = true;
            let engine = self.clone();
            std::thread::spawn(move || {
                while engine.step() {
                    std::thread::sleep(Duration::from_millis(500));
                }
            });
        }
        Ok(run_id)
    }

    pub fn cancel(&self, id: u32) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let status = inner.jobs.iter().find(|j| j.id == id).map(|j| j.status);
        match status {
            Some(s) if !s.is_final() => {
                audit(&inner, "cancel", Some(id), None, "cancelled by user");
                kill(&mut inner, id);
                finish(&mut inner, id, Status::Cancelled, None, Some("cancelled by user".into()));
                Ok(())
            }
            Some(_) => Ok(()),
            None => Err(AppError::NotFound(format!("job {id}"))),
        }
    }

    /// A running job's hidden display as a small screenshot (`data:` URL), for monitoring.
    pub fn frame(&self, adb: &Path, id: u32) -> Result<String> {
        let (serial, surface) = {
            let inner = self.inner.lock().unwrap();
            let s = inner.sessions.get(&id).ok_or_else(|| AppError::NotFound(format!("a hidden display for job {id}")))?;
            (s.serial.clone(), s.surface.clone())
        };
        // Without its own id, screencap would show the phone's screen, which is the user's.
        let surface = surface.ok_or_else(|| AppError::NotFound(format!("the screen id of job {id}'s display")))?;
        crate::monitor::display_data_url(adb, &serial, Some(&surface), 360)
    }

    /// Open a view-only window on a running job's hidden display. Closing it leaves the job
    /// running; it closes by itself when the job ends. One window per job.
    pub fn watch(&self, id: u32) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(w) = inner.watchers.get_mut(&id) {
            if matches!(w.try_wait(), Ok(None)) {
                return Ok(());
            }
        }
        let s = inner.sessions.get(&id).ok_or_else(|| AppError::NotFound(format!("a hidden display for job {id}")))?;
        let (serial, display) = (s.serial.clone(), s.display);
        let sessions = inner.settings.as_ref().map(|st| st.sessions.clone()).unwrap_or_default();
        let (Some(scrcpy), Some(adb)) = (sessions.scrcpy, sessions.adb) else {
            return Err(AppError::NotFound("the bundled scrcpy".into()));
        };
        let title = inner.jobs.iter().find(|j| j.id == id).map(|j| format!("J{id} {} (view only)", j.scenario)).unwrap_or_default();
        let mut cmd = Command::new(&scrcpy);
        // --display-id mirrors an existing display; the session's cleanup never ends it
        // (it has no new_display argument), and closing it never ends the session.
        cmd.args(["-s", &serial])
            .arg(format!("--display-id={display}"))
            .args(["--no-control", "--no-audio", "--no-clipboard-autosync"])
            .arg(format!("--window-title={title}"))
            .env("ADB", &adb)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(d) = scrcpy.parent() {
            cmd.current_dir(d);
        }
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        let mut child = cmd.spawn().map_err(|e| AppError::Io(format!("could not start scrcpy: {e}")))?;
        if !crate::workers::adopt(&child) {
            crate::emulator::kill_tree(&mut child);
            return Err(AppError::Io("could not tie the window to the app".into()));
        }
        audit(&inner, "watch", Some(id), None, &format!("view-only window on {serial} display {display}"));
        inner.watchers.insert(id, child);
        Ok(())
    }

    /// The agent's readable trace (its calls, answers and words), newest last.
    pub fn trace(&self, id: u32, last: usize) -> Vec<crate::agent::TraceEvent> {
        let dir = {
            let inner = self.inner.lock().unwrap();
            inner.jobs.iter().find(|j| j.id == id).and_then(|j| j.evidence_dir.clone())
        };
        let Some(dir) = dir else { return Vec::new() };
        let bytes = std::fs::read(Path::new(&dir).join("trace.jsonl")).unwrap_or_default();
        let events = crate::agent::parse_trace(&String::from_utf8_lossy(&bytes));
        events[events.len().saturating_sub(last)..].to_vec()
    }

    /// Last lines of a job's worker log.
    pub fn log_tail(&self, id: u32, lines: usize) -> Result<String> {
        let dir = {
            let inner = self.inner.lock().unwrap();
            inner.jobs.iter().find(|j| j.id == id).and_then(|j| j.evidence_dir.clone())
        };
        let Some(dir) = dir else { return Ok(String::new()) };
        let bytes = std::fs::read(Path::new(&dir).join("worker.log")).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        let all: Vec<&str> = text.lines().collect();
        Ok(all[all.len().saturating_sub(lines)..].join("\n"))
    }

    /// One scheduler pass: reap finished workers, then start queued jobs on free devices.
    /// Returns false once nothing is queued or running (the scheduler thread then ends).
    fn step(&self) -> bool {
        let (settings, wants_devices) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(settings) = inner.settings.clone() else {
                inner.scheduling = false;
                return false;
            };
            reap(&mut inner, &settings);
            let queued = inner.jobs.iter().any(|j| j.status == Status::Queued);
            if !queued && inner.children.is_empty() {
                inner.scheduling = false;
                return false;
            }
            inner.ticks += 1;
            // Every 5 s (10 passes), look for leased devices that went away, e.g. an unplugged cable.
            let check_leases = !inner.leases.is_empty() && inner.ticks % 10 == 0;
            (settings.clone(), (queued && inner.children.len() < settings.max_parallel) || check_leases)
        };
        if !wants_devices {
            return true;
        }
        // adb is slow: ask it outside the lock, then re-check leases under the lock.
        let ready = (settings.devices)();
        let mut inner = self.inner.lock().unwrap();
        drop_disconnected(&mut inner, &ready);
        let queued: Vec<Want> = inner
            .jobs
            .iter()
            .filter(|j| j.status == Status::Queued)
            .map(|j| Want { id: j.id, allowed: j.devices.clone(), session: j.goal.is_some() || settings.sessions.scripts, veronika: j.veronika, mode: j.mode })
            .collect();
        let slots = settings.max_parallel.saturating_sub(inner.children.len());
        let sessions = &settings.sessions;
        let mut opening = Vec::new();
        for (id, phone, slot) in assign(&queued, &ready, &inner.leases, slots, sessions.per_phone(), sessions.physical_first) {
            let place = Place { serial: transport(&inner.leases, &ready, &phone), phone, slot };
            match slot {
                Slot::Whole => start(&mut inner, &settings, id, place, None),
                Slot::Session(_) => {
                    if reserve(&mut inner, &settings, id, &place) {
                        opening.push((id, place));
                    }
                }
            }
        }
        drop(inner);
        // Opening a display takes seconds (scrcpy, then the app); do it outside the lock, in
        // order, so session 2 finds session 1's display.
        for (id, place) in opening {
            self.open_session(&settings, id, place);
        }
        true
    }

    fn open_session(&self, settings: &Settings, id: u32, place: Place) {
        let (serial, slot) = (place.serial.clone(), place.slot);
        let (dir, sibling) = {
            let inner = self.inner.lock().unwrap();
            let dir = inner.jobs.iter().find(|j| j.id == id).and_then(|j| j.evidence_dir.clone()).unwrap_or_default();
            let sibling = inner
                .leases
                .values()
                .find(|l| l.phone == place.phone && l.slot == Some(1) && l.job_id != id)
                .and_then(|l| inner.sessions.get(&l.job_id))
                .map(|s| s.display);
            (PathBuf::from(dir), sibling)
        };
        let opened = match (&settings.sessions.adb, &settings.sessions.scrcpy) {
            _ if slot == Slot::Session(2) && sibling.is_none() => Err("session 1 on this phone ended before session 2 could open".to_string()),
            (Some(adb), Some(scrcpy)) => crate::vsession::Session::open(adb, scrcpy, &serial, crate::agent::APP_PACKAGE, sibling, &dir),
            _ => Err("no scrcpy to open a display with".to_string()),
        };
        let mut inner = self.inner.lock().unwrap();
        let still = inner.jobs.iter().any(|j| j.id == id && j.status == Status::Running);
        match opened {
            Ok(session) if still => {
                let display = session.display;
                inner.sessions.insert(id, session);
                start(&mut inner, settings, id, place, Some(display));
            }
            Ok(session) => {
                std::thread::spawn(move || session.close());
            }
            Err(e) if still => finish(&mut inner, id, Status::Error, None, Some(format!("could not open a session: {e}"))),
            Err(_) => {}
        }
    }
}

fn kill(inner: &mut Inner, id: u32) {
    if let Some(mut child) = inner.children.remove(&id) {
        crate::emulator::kill_tree(&mut child);
    }
}

fn reap(inner: &mut Inner, settings: &Settings) {
    let timeout = settings.timeout;
    let budget = settings.agent.as_ref().map_or(u32::MAX, |a| a.max_actions);
    let ids: Vec<u32> = inner.children.keys().copied().collect();
    for id in ids {
        if let Some(meter) = inner.meters.get(&id).cloned() {
            let actions = meter.actions.load(Ordering::Relaxed);
            if let Some(job) = inner.jobs.iter_mut().find(|j| j.id == id) {
                job.actions = actions;
            }
            let broken = meter.broken.lock().unwrap().clone();
            if let Some(why) = broken {
                kill(inner, id);
                finish(inner, id, Status::Blocked, None, Some(format!("stopped: the agent tried a refused action ({why})")));
                continue;
            }
            if actions > budget {
                kill(inner, id);
                finish(inner, id, Status::Error, None, Some(format!("stopped after {budget} actions without finishing")));
                continue;
            }
        }
        let exited = inner.children.get_mut(&id).and_then(|c| c.try_wait().ok().flatten());
        if let Some(status) = exited {
            inner.children.remove(&id);
            let job = inner.jobs.iter().find(|j| j.id == id);
            let agent = job.is_some_and(|j| j.goal.is_some());
            let dir = PathBuf::from(job.and_then(|j| j.evidence_dir.clone()).unwrap_or_default());
            let (verdict, note) = if agent {
                crate::agent::verdict(&dir)
            } else {
                (refine(Status::from_exit(status.code()), &dir), None)
            };
            finish(inner, id, verdict, status.code(), note);
            continue;
        }
        let started = inner.jobs.iter().find(|j| j.id == id).and_then(|j| j.started_at).unwrap_or(0);
        if now().saturating_sub(started) > timeout.as_secs() {
            kill(inner, id);
            let note = format!("timed out after {}s", timeout.as_secs());
            finish(inner, id, Status::Error, None, Some(note));
        }
    }
}

/// A leased device missing from two device checks in a row (about 10 s) is gone: its job
/// ends as an infrastructure error and the lease is released.
///
/// It is the lease's own serial that counts: a session over USB ends when the cable is pulled,
/// even while the phone is still reachable over wireless debugging.
fn drop_disconnected(inner: &mut Inner, ready: &[Phone]) {
    let leased: Vec<(String, String, u32)> = inner.leases.iter().map(|(k, l)| (k.clone(), l.serial.clone(), l.job_id)).collect();
    inner.missing.retain(|k, _| leased.iter().any(|(l, _, _)| l == k));
    for (key, serial, id) in leased {
        if ready.iter().any(|p| p.serials.contains(&serial)) {
            inner.missing.remove(&key);
            continue;
        }
        let n = inner.missing.entry(key.clone()).or_insert(0);
        *n += 1;
        if *n >= 2 {
            inner.missing.remove(&key);
            kill(inner, id);
            finish(inner, id, Status::Error, None, Some(format!("device {serial} disconnected")));
        }
    }
}

/// Where a job runs: a phone, the serial it reaches the phone by, and the slot on it.
#[derive(Debug, Clone)]
struct Place {
    phone: String,
    serial: String,
    slot: Slot,
}

/// The serial a new job on `phone` uses: the one its other session already uses (session 2
/// opens beside session 1, through the same adb connection), else USB when it is plugged in.
fn transport(leases: &HashMap<String, Lease>, ready: &[Phone], phone: &str) -> String {
    leases
        .values()
        .find(|l| l.phone == phone)
        .map(|l| l.serial.clone())
        .or_else(|| ready.iter().find(|p| p.id == phone).and_then(|p| p.serials.first().cloned()))
        .unwrap_or_else(|| phone.to_string())
}

/// Mark the job running and lease its place before anything slow happens, so no other job
/// gets the same place. Returns false (the job then ended as an error) when its evidence
/// folder cannot be made.
fn reserve(inner: &mut Inner, settings: &Settings, id: u32, place: &Place) -> bool {
    let (serial, slot) = (place.serial.as_str(), place.slot);
    let Some(job) = inner.jobs.iter_mut().find(|j| j.id == id) else { return false };
    let dir = settings.artifacts.join(&job.run_id).join(slug(&place.phone)).join(format!("J{}-{}", job.id, slug(&job.scenario)));
    job.serial = Some(serial.to_string());
    job.evidence_dir = Some(dir.to_string_lossy().to_string());
    job.started_at = Some(now());
    job.status = Status::Running;
    let lease = Lease {
        phone: place.phone.clone(),
        serial: serial.to_string(),
        veronika: job.veronika,
        slot: match slot {
            Slot::Whole => None,
            Slot::Session(n) => Some(n),
        },
        job_id: id,
        run_id: job.run_id.clone(),
        acquired_at: now(),
    };
    inner.leases.insert(lease_key(&place.phone, slot), lease);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        finish(inner, id, Status::Error, None, Some(format!("could not make the evidence folder: {e}")));
        return false;
    }
    true
}

/// Spawn the job's worker on its leased place. A spawn failure ends the job as an error.
/// `display` is the job's session display, already open (the place was reserved then).
fn start(inner: &mut Inner, settings: &Settings, id: u32, place: Place, display: Option<u32>) {
    if display.is_none() && !reserve(inner, settings, id, &place) {
        return;
    }
    let serial = place.serial;
    let Some(job) = inner.jobs.iter_mut().find(|j| j.id == id) else { return };
    job.display = display;
    let dir = PathBuf::from(job.evidence_dir.clone().unwrap_or_default());

    let goal = job.goal.clone();
    let testset = job.testset.clone();
    let only = job.only.clone();
    let meter = Arc::new(Meter::default());

    let spawned = (|| -> std::io::Result<Child> {
        std::fs::create_dir_all(&dir)?;
        let log = std::fs::File::create(dir.join("worker.log"))?;
        let r = &settings.runner;
        let state_dir = std::env::temp_dir().join("android-agent-skills").join(slug(&serial));
        let path = r.adb_dir.as_ref().and_then(|adb_dir| {
            let mut paths = vec![adb_dir.clone()];
            paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
            std::env::join_paths(paths).ok()
        });
        let mut cmd = match &goal {
            Some(case) => {
                let agent = settings.agent.as_ref().ok_or_else(|| {
                    std::io::Error::other("the AI agent is not set up (no mcp/telkomsel-qa-mcp in the QA skill folder)")
                })?;
                let mut cmd = crate::agent::command(agent, &serial, display, &dir, case, &state_dir, path.as_deref())?;
                cmd.stdout(Stdio::piped());
                cmd
            }
            None => {
                let mut cmd = Command::new(&r.program);
                cmd.args(&r.args).arg(&testset).arg("--out").arg(&dir).args(["-s", &serial]);
                if let Some(only) = &only {
                    cmd.args(["--only", only]);
                }
                cmd.stdout(log.try_clone()?);
                cmd
            }
        };
        // ANDROID_SERIAL scopes any adb call the worker makes without -s.
        // The skill's screen-read lock lives in its state directory; one per device keeps
        // workers on different devices from waiting on each other.
        // ANDROID_DISPLAY points every tap, key, screenshot and screen read at the session.
        cmd.env("ANDROID_SERIAL", &serial)
            .env("ANDROID_DISPLAY", display.map(|d| d.to_string()).unwrap_or_default())
            .env("ANDROID_SKILLS_STATE", &state_dir)
            .env("PYTHONUNBUFFERED", "1")
            .env("PYTHONIOENCODING", "utf-8")
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stderr(log);
        if let Some(path) = &path {
            cmd.env("PATH", path);
        }
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        let mut child = cmd.spawn()?;
        if !crate::workers::adopt(&child) {
            crate::emulator::kill_tree(&mut child);
            return Err(std::io::Error::other("could not tie the worker to the app, so it could outlive it"));
        }
        if let Some(out) = child.stdout.take() {
            let trace = std::fs::File::create(dir.join("trace.jsonl"))?;
            let guard = Guard { serial: serial.clone(), run: dir.clone() };
            let meter = meter.clone();
            std::thread::spawn(move || record_trace(out, trace, &guard, &meter));
        }
        Ok(child)
    })();

    match spawned {
        Ok(child) => {
            if let Some(job) = inner.jobs.iter().find(|j| j.id == id) {
                persist(inner, job);
                let place = display.map(|d| format!(" display {d}")).unwrap_or_default();
                audit(inner, "start", Some(id), Some(&job.run_id), &format!("on {serial}{place}"));
            }
            inner.children.insert(id, child);
            if goal.is_some() {
                inner.meters.insert(id, meter);
            }
        }
        Err(e) => finish(inner, id, Status::Error, None, Some(format!("could not start the worker: {e}"))),
    }
}

/// Copy the agent's stream-json to trace.jsonl, counting tool calls and noting the first
/// call the guard rules refuse (the scheduler then stops the job).
fn record_trace(out: impl std::io::Read, mut trace: std::fs::File, guard: &Guard, meter: &Meter) {
    let mut watched: HashMap<String, String> = HashMap::new();
    for line in std::io::BufReader::new(out).lines().map_while(|l| l.ok()) {
        let _ = writeln!(trace, "{line}");
        let scan = guard.scan(&line);
        meter.actions.fetch_add(scan.calls, Ordering::Relaxed);
        watched.extend(scan.watch);
        let slipped = scan.answers.into_iter().find_map(|(id, refused)| watched.remove(&id).filter(|_| !refused));
        if let Some(why) = scan.stop.or(slipped) {
            meter.broken.lock().unwrap().get_or_insert(why);
        }
    }
}

/// Record the outcome and release the device. Every way a worker ends goes through here,
/// so a crashed or killed worker can never keep its device locked.
fn finish(inner: &mut Inner, id: u32, status: Status, code: Option<i32>, note: Option<String>) {
    inner.leases.retain(|_, l| l.job_id != id);
    inner.meters.remove(&id);
    if let Some(mut w) = inner.watchers.remove(&id) {
        crate::emulator::kill_tree(&mut w);
    }
    if let Some(session) = inner.sessions.remove(&id) {
        // A few adb calls and up to 6 s for scrcpy to finish the recording: off the lock.
        std::thread::spawn(move || session.close());
    }
    let Some(job) = inner.jobs.iter_mut().find(|j| j.id == id) else { return };
    job.status = status;
    job.exit_code = code;
    job.note = note;
    job.finished_at = Some(now());
    let run_id = job.run_id.clone();
    let detail = format!("{}{}", job.status.as_str(), job.note.as_deref().map(|n| format!(": {n}")).unwrap_or_default());
    let job = job.clone();
    persist(inner, &job);
    audit(inner, "finish", Some(id), Some(&run_id), &detail);

    let run: Vec<&Job> = inner.jobs.iter().filter(|j| j.run_id == run_id).collect();
    if run.iter().all(|j| j.status.is_final()) {
        if let Some(settings) = &inner.settings {
            let merged = merge_groups(&settings.artifacts.join(&run_id), &run);
            let _ = write_summary(&settings.artifacts, &run_id, &run, &merged);
            let run: Vec<Job> = run.into_iter().cloned().collect();
            let _ = crate::defects::export(&settings.artifacts, &run_id, &run, &inner.jobs);
        }
    }
}

fn persist(inner: &Inner, job: &Job) {
    if let Some(store) = &inner.store {
        let _ = store.save_job(job);
    }
}

fn audit(inner: &Inner, kind: &str, job_id: Option<u32>, run_id: Option<&str>, detail: &str) {
    if let Some(store) = &inner.store {
        let _ = store.event(now(), kind, job_id, run_id, detail);
    }
}

/// For every test set split across jobs, one bundle of all its cases:
/// `<run>/merged-<group>/run.json`. Returns (group, folder) for each one written.
fn merge_groups(run_dir: &Path, run: &[&Job]) -> Vec<(String, PathBuf)> {
    let mut groups: Vec<&str> = run.iter().filter_map(|j| j.group.as_deref()).collect();
    groups.dedup();
    let mut out = Vec::new();
    for group in groups {
        let shards: Vec<(&Job, Option<serde_json::Value>)> = run
            .iter()
            .filter(|j| j.group.as_deref() == Some(group))
            .map(|j| {
                let bundle = j.evidence_dir.as_ref().and_then(|d| std::fs::read_to_string(Path::new(d).join("run.json")).ok());
                (*j, bundle.and_then(|t| serde_json::from_str(&t).ok()))
            })
            .collect();
        if shards.len() < 2 || shards.iter().all(|(_, b)| b.is_none()) {
            continue;
        }
        let dir = run_dir.join(format!("merged-{}", slug(group)));
        let merged = merge_bundles(run_dir, group, &shards);
        let written = std::fs::create_dir_all(&dir)
            .and_then(|_| std::fs::write(dir.join("run.json"), serde_json::to_string_pretty(&merged).unwrap_or_default()));
        if written.is_ok() {
            out.push((group.to_string(), dir));
        }
    }
    out
}

/// One bundle from a split test set's job bundles. Cases keep their evidence where their job
/// wrote it: their `evidence/`, `video/` and `logs/` paths point into that job's folder.
/// Two jobs on one phone read the same crash log, so crash lines are counted once.
fn merge_bundles(run_dir: &Path, group: &str, shards: &[(&Job, Option<serde_json::Value>)]) -> serde_json::Value {
    use serde_json::{json, Value};
    let mut base = shards.iter().find_map(|(_, b)| b.clone()).unwrap_or_else(|| json!({}));
    let (mut cases, mut lines, mut info) = (Vec::new(), Vec::<Value>::new(), Vec::new());
    let (mut fatal, mut anr) = (0, 0);
    let (mut started, mut finished): (Option<String>, Option<String>) = (None, None);
    for (job, bundle) in shards {
        let dir = PathBuf::from(job.evidence_dir.clone().unwrap_or_default());
        let rel = dir.strip_prefix(run_dir).map(|r| format!("../{}/", r.to_string_lossy().replace('\\', "/"))).unwrap_or_default();
        info.push(json!({
            "job": job.id,
            "only": job.only,
            "serial": job.serial,
            "display": job.display,
            "status": job.status.as_str(),
            "dir": dir,
            "recording": job.display.map(|_| dir.join("session.mkv")),
            "bundle": bundle.is_some(),
        }));
        let Some(b) = bundle else { continue };
        for c in b["cases"].as_array().into_iter().flatten() {
            let mut c = c.clone();
            rebase(&mut c, &rel);
            cases.push(c);
        }
        let st = &b["stability"];
        fatal = fatal.max(st["fatal"].as_u64().unwrap_or(0));
        anr = anr.max(st["anr"].as_u64().unwrap_or(0));
        for l in st["lines"].as_array().into_iter().flatten() {
            if !lines.contains(l) {
                lines.push(l.clone());
            }
        }
        if let Some(t) = b["started"].as_str() {
            started = Some(started.map_or(t.to_string(), |s| s.min(t.to_string())));
        }
        if let Some(t) = b["finished"].as_str() {
            finished = Some(finished.map_or(t.to_string(), |s| s.max(t.to_string())));
        }
    }
    cases.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let count = |what: &str| lines.iter().filter(|l| l.as_str().is_some_and(|l| l.contains(what))).count() as u64;
    base["run_id"] = json!(group);
    base["cases"] = Value::Array(cases);
    base["stability"] = json!({ "fatal": fatal.max(count("FATAL EXCEPTION")), "anr": anr.max(count("ANR in ")), "lines": lines });
    base["started"] = json!(started);
    base["finished"] = json!(finished);
    base["shards"] = Value::Array(info);
    base
}

fn rebase(v: &mut serde_json::Value, rel: &str) {
    match v {
        serde_json::Value::String(s) if ["evidence/", "video/", "logs/"].iter().any(|p| s.starts_with(p)) => *s = format!("{rel}{s}"),
        serde_json::Value::Array(a) => a.iter_mut().for_each(|x| rebase(x, rel)),
        serde_json::Value::Object(o) => o.values_mut().for_each(|x| rebase(x, rel)),
        _ => {}
    }
}

/// `<artifacts>/<run_id>/reports/summary.json`: every job with its device, scenario,
/// verdict and evidence directory, and the merged bundle of each split test set.
fn write_summary(artifacts: &Path, run_id: &str, jobs: &[&Job], merged: &[(String, PathBuf)]) -> std::io::Result<()> {
    let count = |s: Status| jobs.iter().filter(|j| j.status == s).count();
    let summary = serde_json::json!({
        "runId": run_id,
        "finishedAt": now(),
        "totals": {
            "jobs": jobs.len(),
            "passed": count(Status::Passed),
            "failed": count(Status::Failed),
            "blocked": count(Status::Blocked),
            "error": count(Status::Error),
            "cancelled": count(Status::Cancelled),
        },
        "jobs": jobs,
        "merged": merged.iter().map(|(group, dir)| serde_json::json!({ "group": group, "dir": dir })).collect::<Vec<_>>(),
    });
    let dir = artifacts.join(run_id).join("reports");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("summary.json"), serde_json::to_string_pretty(&summary)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(serial: &str, job_id: u32) -> (String, Lease) {
        (serial.into(), Lease { phone: serial.into(), serial: serial.into(), veronika: false, slot: None, job_id, run_id: "R".into(), acquired_at: 0 })
    }

    fn phones(serials: &[&str]) -> Vec<Phone> {
        serials.iter().map(|s| Phone::one(s)).collect()
    }

    #[test]
    fn exit_codes_never_turn_unknown_into_pass() {
        assert_eq!(Status::from_exit(Some(0)), Status::Passed);
        assert_eq!(Status::from_exit(Some(1)), Status::Failed);
        assert_eq!(Status::from_exit(Some(3)), Status::Blocked);
        assert_eq!(Status::from_exit(Some(2)), Status::Error);
        assert_eq!(Status::from_exit(None), Status::Error);
    }

    #[test]
    fn errored_cases_are_not_reported_as_product_failures() {
        let dir = tempfile::tempdir().unwrap();
        let write = |cases: &str| std::fs::write(dir.path().join("run.json"), format!("{{\"cases\":[{cases}]}}")).unwrap();
        assert_eq!(refine(Status::Failed, dir.path()), Status::Failed, "no bundle: keep the exit code's verdict");
        write(r#"{"verdict":"pass"},{"verdict":"error"}"#);
        assert_eq!(refine(Status::Failed, dir.path()), Status::Error);
        write(r#"{"verdict":"fail"},{"verdict":"error"}"#);
        assert_eq!(refine(Status::Failed, dir.path()), Status::Failed);
        write(r#"{"verdict":"error"}"#);
        assert_eq!(refine(Status::Passed, dir.path()), Status::Passed);
    }

    /// Scripted jobs with sessions off: the scheduler as it was before sessions.
    fn plain(queued: &[(u32, Vec<String>)], ready: &[Phone], leased: &HashMap<String, Lease>, slots: usize) -> Vec<(u32, String)> {
        let wants: Vec<Want> = queued.iter().map(|(id, allowed)| Want { id: *id, allowed: allowed.clone(), ..script(*id) }).collect();
        assign(&wants, ready, leased, slots, 0, false).into_iter().map(|(id, s, slot)| {
            assert_eq!(slot, Slot::Whole);
            (id, s)
        }).collect()
    }

    /// An AI job: it may open Veronika.
    fn ai(id: u32) -> Want {
        Want { id, allowed: vec![], session: true, veronika: true, mode: Mode::Auto }
    }

    /// A scripted job on native screens (search, home), with a session-aware run.py.
    fn native(id: u32) -> Want {
        Want { veronika: false, ..ai(id) }
    }

    /// A scripted job with a run.py that restarts the app: it needs the whole phone.
    fn script(id: u32) -> Want {
        Want { session: false, veronika: false, ..ai(id) }
    }

    fn held(places: &[(&str, Option<u8>, u32)]) -> HashMap<String, Lease> {
        places
            .iter()
            .map(|(serial, slot, job_id)| {
                let key = lease_key(serial, slot.map_or(Slot::Whole, Slot::Session));
                (key, Lease { phone: serial.to_string(), serial: serial.to_string(), veronika: false, slot: *slot, job_id: *job_id, run_id: "R".into(), acquired_at: 0 })
            })
            .collect()
    }

    #[test]
    fn one_phone_runs_two_ai_sessions_and_scripts_wait_for_the_whole_phone() {
        let ready = phones(&["phone"]);
        let none = HashMap::new();
        // Two jobs share the phone: session 1, then session 2 beside it.
        assert_eq!(
            assign(&[native(1), ai(2), native(3)], &ready, &none, 5, 2, true),
            vec![(1, "phone".into(), Slot::Session(1)), (2, "phone".into(), Slot::Session(2))]
        );
        // A scripted job needs the whole phone, so it waits while any session runs.
        assert!(assign(&[script(1)], &ready, &held(&[("phone", Some(2), 7)]), 5, 2, true).is_empty());
        assert_eq!(assign(&[script(1)], &ready, &none, 5, 2, true), vec![(1, "phone".into(), Slot::Whole)]);
        // With session 1 gone and 2 still running, no new session 1 starts (it restarts the
        // app); a free session 2 next to a running session 1 is refilled.
        assert!(assign(&[ai(4)], &ready, &held(&[("phone", Some(2), 7)]), 5, 2, true).is_empty());
        assert_eq!(assign(&[ai(4)], &ready, &held(&[("phone", Some(1), 7)]), 5, 2, true), vec![(4, "phone".into(), Slot::Session(2))]);
        // One session per phone, or sessions off: jobs take the phone one at a time.
        assert_eq!(assign(&[native(1), native(2)], &ready, &none, 5, 1, true), vec![(1, "phone".into(), Slot::Session(1))]);
        assert_eq!(assign(&[native(1), native(2)], &ready, &none, 5, 0, true), vec![(1, "phone".into(), Slot::Whole)]);
        // The parallel limit still counts every session.
        assert_eq!(assign(&[native(1), native(2)], &ready, &none, 1, 2, true).len(), 1);
    }

    #[test]
    fn physical_phones_come_before_emulators_and_emulators_get_no_sessions() {
        let ready = phones(&["emulator-5554", "phone"]);
        let none = HashMap::new();
        assert_eq!(
            assign(&[native(1), native(2), native(3)], &ready, &none, 5, 2, true),
            vec![(1, "phone".into(), Slot::Session(1)), (2, "phone".into(), Slot::Session(2)), (3, "emulator-5554".into(), Slot::Whole)]
        );
        assert_eq!(assign(&[script(1)], &ready, &none, 5, 2, true), vec![(1, "phone".into(), Slot::Whole)]);
        assert_eq!(assign(&[script(1)], &ready, &none, 5, 2, false), vec![(1, "emulator-5554".into(), Slot::Whole)]);
    }

    #[test]
    fn assign_skips_leased_devices_and_respects_slots() {
        let ready = phones(&["a", "b", "c"]);
        let leased: HashMap<String, Lease> = [lease("a", 9)].into();
        let any = |ids: &[u32]| ids.iter().map(|i| (*i, Vec::new())).collect::<Vec<_>>();
        assert_eq!(plain(&any(&[1, 2, 3]), &ready, &leased, 5), vec![(1, "b".into()), (2, "c".into())]);
        assert_eq!(plain(&any(&[1, 2, 3]), &ready, &leased, 1), vec![(1, "b".into())]);
        assert!(plain(&any(&[1]), &ready, &leased, 0).is_empty());
    }

    #[test]
    fn assign_keeps_pinned_jobs_on_their_devices() {
        let ready = phones(&["a", "b"]);
        let none: HashMap<String, Lease> = HashMap::new();
        let on = |s: &str| vec![s.to_string()];
        // Two jobs pinned to "b": the first takes it, the second waits; the unpinned job
        // behind them is not held up and takes "a".
        assert_eq!(
            plain(&[(1, on("b")), (2, on("b")), (3, vec![])], &ready, &none, 5),
            vec![(1, "b".into()), (3, "a".into())]
        );
        // A job pinned to a device that is busy or absent waits; others still start.
        let leased: HashMap<String, Lease> = [lease("a", 9)].into();
        assert_eq!(plain(&[(1, on("a")), (2, on("zz")), (3, vec![])], &ready, &leased, 5), vec![(3, "b".into())]);
        // One job per device ("run on every phone") runs on both at once.
        assert_eq!(plain(&[(1, on("a")), (2, on("b"))], &ready, &none, 2), vec![(1, "a".into()), (2, "b".into())]);
    }

    #[test]
    fn a_job_pinned_to_a_device_runs_there() {
        let out = tempfile::tempdir().unwrap();
        let engine = QaEngine::new();
        let pinned = vec![
            Scenario { testset: "0".into(), devices: vec!["dev-b".into()], ..Default::default() },
            Scenario { testset: "0".into(), devices: vec!["dev-b".into()], ..Default::default() },
        ];
        engine.submit_with(fake(out.path(), &["dev-a", "dev-b"], 2, 60), pinned).unwrap();
        let state = wait_done(&engine, |s| {
            assert!(s.jobs.iter().filter(|j| j.status == Status::Running).count() <= 1, "one pinned device, one job at a time");
        });
        assert!(state.jobs.iter().all(|j| j.serial.as_deref() == Some("dev-b") && j.status == Status::Passed));
    }

    #[test]
    fn labels_scenarios_by_testset_and_case_filter() {
        let s = Scenario { testset: "C:\\x\\veronika-starter.yaml".into(), only: Some(" VER-T001 ".into()), ..Default::default() };
        assert_eq!(scenario_label(&s), "veronika-starter:VER-T001");
        assert_eq!(slug("veronika-starter:VER-T001"), "veronika-starter_VER-T001");
    }

    /// A stand-in worker: sleeps, then exits with the code named by its "test set" argument.
    fn fake(artifacts: &Path, devices: &[&str], max_parallel: usize, timeout: u64) -> Settings {
        let devices = phones(devices);
        let script = "import sys,time; time.sleep(1.5); print('worker', sys.argv[1:]); sys.exit(int(sys.argv[1]))";
        Settings {
            runner: Runner { program: "python".into(), args: vec!["-c".into(), script.into()], adb_dir: None },
            cases: Arc::new(|_, _| None),
            artifacts: artifacts.to_path_buf(),
            max_parallel,
            timeout: Duration::from_secs(timeout),
            devices: Arc::new(move || devices.clone()),
            agent: None,
            sessions: SessionSettings::default(),
        }
    }

    /// A stand-in model CLI: prints a stream-json trace for the given mode and, like the
    /// real agent, leaves its run bundle (run.json) in its working directory.
    const FAKE_AGENT: &str = r#"
import json, sys, time
mode = sys.argv[1]
def say(o): print(json.dumps(o), flush=True)
def call(i, name, args): say({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t%d" % i, "name": "mcp__telkomsel-qa__" + name, "input": args}]}})
if mode in ("pass", "silent"):
    for i in range(3):
        call(i, "adb_screen_read", {})
    if mode == "pass":
        open("run.json", "w").write(json.dumps({"cases": [{"verdict": "pass"}]}))
    say({"type": "result", "is_error": False, "result": "Reached it, it all passed.", "num_turns": 3})
elif mode == "approve":
    call(1, "adb_input_tap", {"target": ["Beli"], "confirm": True})
    time.sleep(30)
elif mode == "loop":
    for i in range(50):
        call(i, "adb_input_scroll", {"direction": "down"})
    time.sleep(30)
"#;

    fn fake_agent(artifacts: &Path, devices: &[&str], max_actions: u32, mode: &str) -> (Settings, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("fake_agent.py");
        std::fs::write(&script, FAKE_AGENT).unwrap();
        let mut settings = fake(artifacts, devices, 1, 60);
        settings.agent = Some(AgentConfig {
            command: format!("python \"{}\" {mode}", script.display()),
            model: "fake".into(),
            python: "python".into(),
            skill_root: tmp.path().into(),
            mcp_server: tmp.path().join("serve.py"),
            max_actions,
        });
        (settings, tmp)
    }

    fn goal() -> Scenario {
        let case = PlanItem { kind: "goal".into(), id: Some("AI-001".into()), goal: Some("Buka Roaming".into()), ..Default::default() };
        Scenario { goal: Some(case), ..Default::default() }
    }

    /// Run one goal job with the fake agent in `mode`; return it and its trace.
    fn run_agent(mode: &str) -> (Job, Vec<crate::agent::TraceEvent>, tempfile::TempDir) {
        let out = tempfile::tempdir().unwrap();
        let (settings, _tmp) = fake_agent(out.path(), &["p1"], 20, mode);
        let engine = QaEngine::new();
        engine.submit_with(settings, vec![goal()]).unwrap();
        let state = wait_done(&engine, |_| {});
        assert!(state.leases.is_empty());
        let job = state.jobs[0].clone();
        let trace = engine.trace(job.id, 50);
        (job, trace, out)
    }

    fn scenarios(codes: &[&str]) -> Vec<Scenario> {
        codes.iter().map(|c| Scenario { testset: c.to_string(), ..Default::default() }).collect()
    }

    fn wait_done(engine: &QaEngine, mut each: impl FnMut(&QaState)) -> QaState {
        for _ in 0..200 {
            let state = engine.state();
            each(&state);
            if state.jobs.iter().all(|j| j.status.is_final()) {
                return state;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("jobs did not finish: {:?}", engine.state().jobs);
    }

    #[test]
    fn runs_jobs_in_parallel_with_exclusive_leases_and_isolated_evidence() {
        let out = tempfile::tempdir().unwrap();
        let engine = QaEngine::new();
        // Three jobs, two devices: one passes, one fails, one waits for a free device.
        let run = engine.submit_with(fake(out.path(), &["dev-a", "dev-b"], 2, 60), scenarios(&["0", "1", "0"])).unwrap();

        let mut peak = 0;
        let state = wait_done(&engine, |s| {
            let running: Vec<&Job> = s.jobs.iter().filter(|j| j.status == Status::Running).collect();
            peak = peak.max(running.len());
            let mut serials: Vec<_> = running.iter().map(|j| j.serial.clone()).collect();
            serials.sort();
            serials.dedup();
            assert_eq!(serials.len(), running.len(), "a device was given to two workers");
        });

        assert_eq!(peak, 2, "two workers ran at the same time");
        let statuses: Vec<Status> = state.jobs.iter().map(|j| j.status).collect();
        assert_eq!(statuses, vec![Status::Passed, Status::Failed, Status::Passed]);
        assert!(state.leases.is_empty(), "leases are released when workers end");

        let dirs: std::collections::HashSet<_> = state.jobs.iter().map(|j| j.evidence_dir.clone().unwrap()).collect();
        assert_eq!(dirs.len(), 3, "every job has its own evidence directory");
        for j in &state.jobs {
            let dir = j.evidence_dir.as_ref().unwrap();
            assert!(dir.contains(j.serial.as_ref().unwrap()));
            assert!(std::fs::read_to_string(Path::new(dir).join("worker.log")).unwrap().contains("worker"));
        }
        let summary = std::fs::read_to_string(out.path().join(&run).join("reports").join("summary.json")).unwrap();
        assert!(summary.contains("\"passed\": 2") && summary.contains("\"failed\": 1"));
        assert!(summary.contains("dev-a") && summary.contains("dev-b"));
    }

    #[test]
    fn cancel_and_timeout_release_the_device() {
        let out = tempfile::tempdir().unwrap();
        let engine = QaEngine::new();
        // One device: job 1 is cancelled while running, job 2 then takes the device and times out.
        engine.submit_with(fake(out.path(), &["dev-a"], 2, 0), scenarios(&["0", "0"])).unwrap();
        for _ in 0..100 {
            if engine.state().jobs[0].status == Status::Running {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        engine.cancel(1).unwrap();
        let state = wait_done(&engine, |_| {});
        assert_eq!(state.jobs[0].status, Status::Cancelled);
        assert_eq!(state.jobs[1].status, Status::Error);
        assert_eq!(state.jobs[1].serial.as_deref(), Some("dev-a"));
        assert!(state.jobs[1].note.as_deref().unwrap().contains("timed out"));
        assert!(state.leases.is_empty());
    }

    #[test]
    fn agent_job_passes_only_on_recorded_evidence() {
        let (job, trace, _out) = run_agent("pass");
        assert_eq!((job.status, job.kind.as_str(), job.actions), (Status::Passed, "agent", 3));
        assert_eq!(job.scenario, "goal:AI-001");
        assert_eq!(trace.iter().filter(|e| e.kind == "tool").count(), 3);
        assert_eq!(trace.last().unwrap().kind, "done");
        let dir = Path::new(job.evidence_dir.as_ref().unwrap());
        for f in ["plan.yaml", "mcp.json", "guard.py", "settings.json", "trace.jsonl"] {
            assert!(dir.join(f).is_file(), "{f} is in the evidence directory");
        }

        let (job, _, _out) = run_agent("silent");
        assert_eq!(job.status, Status::Error, "the model said it passed but recorded nothing");
        assert!(job.note.as_deref().unwrap().contains("no evidence"));
    }

    #[test]
    fn agent_job_is_stopped_on_a_refused_action_or_an_endless_run() {
        let (job, _, _out) = run_agent("approve");
        assert_eq!(job.status, Status::Blocked);
        assert!(job.note.as_deref().unwrap().contains("refused action"));

        let (job, _, _out) = run_agent("loop");
        assert_eq!(job.status, Status::Error);
        assert!(job.note.as_deref().unwrap().contains("20 actions"));
    }

    #[test]
    fn history_survives_a_restart_and_interrupted_jobs_get_a_final_state() {
        let out = tempfile::tempdir().unwrap();
        let db = out.path().join("history.db");
        let first = QaEngine::with_store(Store::open(&db).unwrap());
        first.submit_with(fake(out.path(), &["d1"], 1, 60), scenarios(&["0"])).unwrap();
        wait_done(&first, |_| {});

        let mut slow = fake(out.path(), &["d1"], 1, 120);
        slow.runner.args = vec!["-c".into(), "import time; time.sleep(60)".into()];
        first.submit_with(slow, scenarios(&["slow", "waiting"])).unwrap();
        for _ in 0..100 {
            if first.state().jobs[1].status == Status::Running {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        // The app restarts while job 2 runs and job 3 waits for the only device.
        let second = QaEngine::with_store(Store::open(&db).unwrap());
        let jobs = second.state().jobs;
        assert_eq!(jobs.iter().map(|j| j.status).collect::<Vec<_>>(), vec![Status::Passed, Status::Error, Status::Cancelled]);
        assert!(jobs[1].note.as_deref().unwrap().contains("interrupted"));
        assert!(jobs[1].evidence_dir.is_some(), "its evidence is still referenced");
        let kinds: Vec<String> = second.events(50).into_iter().map(|e| e.kind).collect();
        for k in ["submit", "start", "finish", "interrupted"] {
            assert!(kinds.iter().any(|x| x == k), "{k} is in the audit log: {kinds:?}");
        }

        let run = second.submit_with(fake(out.path(), &["d1"], 1, 60), scenarios(&["0"])).unwrap();
        let state = second.state();
        assert_eq!(state.jobs.last().unwrap().id, 4, "ids continue after a restart");
        assert_eq!(state.jobs.last().unwrap().run_id, run);
        first.cancel(2).unwrap();
        wait_done(&second, |_| {});
    }

    #[test]
    fn an_unplugged_device_ends_its_job_and_frees_the_lease() {
        let out = tempfile::tempdir().unwrap();
        let attached = Arc::new(Mutex::new(vec!["usb-1".to_string()]));
        let mut settings = fake(out.path(), &[], 1, 120);
        let seen = attached.clone();
        settings.devices = Arc::new(move || seen.lock().unwrap().iter().map(|s| Phone::one(s)).collect());
        settings.runner.args = vec!["-c".into(), "import time; time.sleep(60)".into()];
        let engine = QaEngine::new();
        engine.submit_with(settings, scenarios(&["0"])).unwrap();
        for _ in 0..100 {
            if engine.state().jobs[0].status == Status::Running {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        attached.lock().unwrap().clear();
        let state = wait_done(&engine, |_| {});
        assert_eq!(state.jobs[0].status, Status::Error);
        assert!(state.jobs[0].note.as_deref().unwrap().contains("disconnected"));
        assert!(state.leases.is_empty());
    }

    #[test]
    fn one_phone_on_usb_and_wireless_is_one_place_reached_over_usb() {
        let wifi = "adb-PIX1-abc._adb-tls-connect._tcp";
        let ready = group_phones([(wifi.to_string(), "PIX1".to_string()), ("PIX1".to_string(), "PIX1".to_string()), ("emulator-5554".to_string(), "emulator-5554".to_string())]);
        assert_eq!(ready, vec![Phone { id: "PIX1".into(), serials: vec!["PIX1".into(), wifi.into()] }, Phone::one("emulator-5554")]);
        let none = HashMap::new();
        // Two scripted jobs: one on the phone, one on the emulator, never two on the phone.
        assert_eq!(assign(&[script(1), script(2), script(3)], &ready, &none, 5, 0, true), vec![(1, "PIX1".into(), Slot::Whole), (2, "emulator-5554".into(), Slot::Whole)]);
        // A job pinned to the phone's wireless serial runs on that phone; new jobs go over USB.
        let pinned = Want { allowed: vec![wifi.into()], ..script(4) };
        assert_eq!(assign(&[pinned], &ready, &none, 5, 0, true), vec![(4, "PIX1".into(), Slot::Whole)]);
        assert_eq!(transport(&none, &ready, "PIX1"), "PIX1");
        // Session 2 uses the connection session 1 already uses.
        let mut s1 = held(&[("PIX1", Some(1), 7)]);
        s1.values_mut().for_each(|l| l.serial = wifi.into());
        assert_eq!(transport(&s1, &ready, "PIX1"), wifi);
    }

    #[test]
    fn a_session_ends_when_its_own_connection_goes_even_if_the_phone_is_still_reachable() {
        let mut inner = Inner::default();
        inner.jobs.push(Job { id: 7, status: Status::Running, ..job() });
        inner.leases = held(&[("PIX1", Some(1), 7)]);
        let still = vec![Phone { id: "PIX1".into(), serials: vec!["wifi-serial".into()] }];
        drop_disconnected(&mut inner, &still);
        assert_eq!(inner.jobs[0].status, Status::Running, "one missed check is not enough");
        drop_disconnected(&mut inner, &still);
        assert_eq!(inner.jobs[0].status, Status::Error);
        assert!(inner.leases.is_empty());
    }

    #[test]
    fn veronika_jobs_never_share_a_phone_and_modes_pick_the_screen_or_a_session() {
        let ready = phones(&["emulator-5554", "phone"]);
        let none = HashMap::new();
        // Two AI jobs: the phone has one Veronika, so the second goes to the emulator.
        assert_eq!(assign(&[ai(1), ai(2)], &ready, &none, 5, 2, true), vec![(1, "phone".into(), Slot::Session(1)), (2, "emulator-5554".into(), Slot::Whole)]);
        // A native job pairs with a running Veronika job.
        let mut chat = held(&[("phone", Some(1), 7)]);
        chat.values_mut().for_each(|l| l.veronika = true);
        assert_eq!(assign(&[ai(1), native(2)], &phones(&["phone"]), &chat, 5, 2, true), vec![(2, "phone".into(), Slot::Session(2))]);
        // Screen: the phone's own screen, one job at a time.
        let screen = |id| Want { mode: Mode::Screen, ..native(id) };
        assert_eq!(assign(&[screen(1), screen(2)], &phones(&["phone"]), &none, 5, 2, true), vec![(1, "phone".into(), Slot::Whole)]);
        // Hidden: sessions only, so never an emulator and never with sessions off.
        let hidden = |id| Want { mode: Mode::Hidden, ..native(id) };
        assert_eq!(assign(&[hidden(1), hidden(2), hidden(3)], &ready, &none, 5, 2, true), vec![(1, "phone".into(), Slot::Session(1)), (2, "phone".into(), Slot::Session(2))]);
        assert!(assign(&[hidden(1)], &ready, &none, 5, 0, true).is_empty());
    }

    fn case(id: &str, surface: &str, repeat: u32, follows: Option<&str>) -> CaseInfo {
        CaseInfo { id: id.into(), surface: surface.into(), repeat, follows: follows.map(str::to_string) }
    }

    #[test]
    fn list_lines_are_parsed_with_their_follow_up_parent() {
        let text = "FUP-A1     m2 veronika       scenario     x1  Apa itu roaming?\n\
FUP-A2     m2 veronika       scenario     x1  <- FUP-A1  Berapa harganya?\n\
REC-01     m2 search         scenario     x3  roaming\n\
M1 sample: 2 of 5 m1 cases, seed 7\n3 case(s), 5 run(s)\n";
        assert_eq!(parse_list(text), vec![
            case("FUP-A1", "veronika", 1, None),
            case("FUP-A2", "veronika", 1, Some("FUP-A1")),
            case("REC-01", "search", 3, None),
        ]);
    }

    #[test]
    fn shards_keep_chains_whole_balance_runs_and_keep_veronika_to_one_job_per_phone() {
        let cases = vec![
            case("V1", "veronika", 1, None),
            case("V2", "veronika", 1, Some("V1")),
            case("V3", "veronika", 2, None),
            case("S1", "search", 3, None),
            case("S2", "search", 1, None),
            case("S3", "search-suggest", 2, None),
        ];
        // One phone, two sessions: one Veronika job (its whole chain in order) and one native job.
        assert_eq!(shard(&cases, 1, 2), vec![
            (vec!["V1".into(), "V2".into(), "V3".into()], true),
            (vec!["S1".into(), "S2".into(), "S3".into()], false),
        ]);
        // Two phones with two sessions each: two of each, balanced by runs (S1=3 | S3+S2=3).
        assert_eq!(shard(&cases, 2, 4), vec![
            (vec!["V1".into(), "V2".into()], true),
            (vec!["V3".into()], true),
            (vec!["S1".into()], false),
            (vec!["S2".into(), "S3".into()], false),
        ]);
        // Only native cases: every place gets work.
        assert_eq!(shard(&cases[3..], 1, 2).len(), 2);
    }

    #[test]
    fn split_runs_merge_into_one_bundle_that_points_at_each_jobs_evidence() {
        let run = Path::new("A").join("RUN-1");
        let at = |id, dir: &str, display| Job { id, evidence_dir: Some(run.join("PIX1").join(dir).to_string_lossy().to_string()), display, status: Status::Passed, ..job() };
        let (j1, j2) = (at(1, "J1-x", Some(20)), at(2, "J2-x", Some(21)));
        let crash = "E/AndroidRuntime: FATAL EXCEPTION: main";
        let b1 = serde_json::json!({"started": "T2", "finished": "T5", "cases": [{"id": "S2", "runs": [{"evidence": [{"file": "evidence/E-S2-1.txt"}]}]}],
            "stability": {"fatal": 1, "anr": 0, "lines": [crash]}});
        let b2 = serde_json::json!({"started": "T1", "finished": "T4", "cases": [{"id": "S1", "runs": [{"recording": {"files": ["video/S1-1.mp4"]}}]}],
            "stability": {"fatal": 1, "anr": 0, "lines": [crash]}});
        let m = merge_bundles(&run, "set-1", &[(&j1, Some(b1)), (&j2, Some(b2))]);
        assert_eq!(m["cases"][0]["id"], "S1");
        assert_eq!(m["cases"][0]["runs"][0]["recording"]["files"][0], "../PIX1/J2-x/video/S1-1.mp4");
        assert_eq!(m["cases"][1]["runs"][0]["evidence"][0]["file"], "../PIX1/J1-x/evidence/E-S2-1.txt");
        assert_eq!(m["stability"]["fatal"], 1, "the same crash read by both sessions counts once");
        assert_eq!((m["started"].as_str(), m["finished"].as_str()), (Some("T1"), Some("T5")));
        assert_eq!(m["shards"].as_array().unwrap().len(), 2);
        assert!(m["shards"][0]["recording"].as_str().unwrap().ends_with("session.mkv"));
    }

    #[test]
    fn a_whole_test_set_is_split_across_free_places_and_merged_when_the_run_ends() {
        let out = tempfile::tempdir().unwrap();
        let mut settings = fake(out.path(), &["dev-a", "dev-b"], 2, 60);
        settings.cases = Arc::new(|_, _| Some(vec![case("S1", "search", 1, None), case("S2", "search", 1, None), case("V1", "veronika", 1, None)]));
        // The stand-in worker writes a bundle with the cases it was given.
        settings.runner.args = vec!["-c".into(), "import sys,json,time; time.sleep(1); ids=sys.argv[sys.argv.index('--only')+1].split(','); \
open('run.json','w').write(json.dumps({'cases':[{'id':i,'verdict':'pass'} for i in ids],'stability':{'fatal':0,'anr':0,'lines':[]}}))".into()];
        let engine = QaEngine::new();
        let run = engine.submit_with(settings, vec![Scenario { testset: "C:/sets/mixed.yaml".into(), ..Default::default() }]).unwrap();
        let state = wait_done(&engine, |_| {});
        let labels: Vec<&str> = state.jobs.iter().map(|j| j.scenario.as_str()).collect();
        assert_eq!(labels, vec!["mixed [1/2]", "mixed [2/2]"]);
        assert_eq!(state.jobs.iter().map(|j| (j.only.clone().unwrap(), j.veronika)).collect::<Vec<_>>(), vec![("V1".into(), true), ("S1,S2".into(), false)]);
        let merged: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.path().join(&run).join("merged-mixed-1").join("run.json")).unwrap()).unwrap();
        let ids: Vec<&str> = merged["cases"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["S1", "S2", "V1"]);
        let summary = std::fs::read_to_string(out.path().join(&run).join("reports").join("summary.json")).unwrap();
        assert!(summary.contains("merged-mixed-1"));
    }

    #[test]
    fn a_hidden_run_is_refused_when_sessions_are_off() {
        let out = tempfile::tempdir().unwrap();
        let err = QaEngine::new().submit_with(fake(out.path(), &["p"], 1, 60), vec![Scenario { testset: "0".into(), mode: Mode::Hidden, ..Default::default() }]);
        assert!(err.is_err());
    }

    fn job() -> Job {
        Job {
            id: 0,
            run_id: "R".into(),
            scenario: "s".into(),
            testset: "t".into(),
            only: None,
            kind: "script".into(),
            goal: None,
            devices: vec![],
            actions: 0,
            status: Status::Queued,
            serial: None,
            display: None,
            mode: Mode::Auto,
            veronika: false,
            group: None,
            evidence_dir: None,
            exit_code: None,
            note: None,
            queued_at: 0,
            started_at: None,
            finished_at: None,
        }
    }

    /// Phase 2 acceptance against real emulators and the real QA skill.
    /// `AQAOS_SKILL_DIR=<telkomsel-qa-skills> AQAOS_E2E="search-playbook.yaml:SRC-T001,search-playbook.yaml:SRC-T008"
    ///  cargo test --lib parallel_skill -- --ignored --nocapture`
    #[test]
    #[ignore = "boots two real emulators and runs the QA skill on them"]
    fn parallel_skill_run_on_two_emulators() {
        use crate::emulator::{locate_sdk, EmulatorManager, State};
        let sdk = locate_sdk().expect("Android SDK");
        let adb = sdk.adb.clone().expect("platform-tools adb");
        let out = std::env::temp_dir().join(format!("aqaos-e2e-{}", now()));
        let cfg = QaConfig {
            skill_dir: std::env::var("AQAOS_SKILL_DIR").expect("AQAOS_SKILL_DIR"),
            artifacts_dir: out.to_string_lossy().to_string(),
            timeout_secs: 420,
            ..QaConfig::default()
        };
        let testsets = cfg.testsets().unwrap();
        let scenarios: Vec<Scenario> = std::env::var("AQAOS_E2E")
            .expect("AQAOS_E2E")
            .split(',')
            .map(|item| {
                let (file, only) = item.split_once(':').unwrap_or((item, ""));
                let testset = testsets.iter().find(|t| t.ends_with(file)).expect("test set").clone();
                Scenario { testset, only: Some(only.to_string()), ..Default::default() }
            })
            .collect();

        let fleet = EmulatorManager::new();
        let logs = tempfile::tempdir().unwrap();
        let avds: Vec<String> = fleet.snapshot(&adb).unwrap().into_iter().take(2).map(|e| e.avd.name).collect();
        for avd in &avds {
            fleet.start(&sdk, &adb, avd, true, logs.path()).unwrap();
        }
        let ready = || fleet.snapshot(&adb).unwrap().iter().filter(|e| e.state == State::Ready).count();
        for _ in 0..140 {
            if ready() == avds.len() {
                break;
            }
            std::thread::sleep(Duration::from_secs(3));
        }
        assert_eq!(ready(), 2, "both emulators booted");

        let engine = QaEngine::new();
        let run = engine.submit(&cfg, &adb, scenarios).unwrap();
        let mut peak = 0;
        let mut state = engine.state();
        for _ in 0..900 {
            state = engine.state();
            peak = peak.max(state.jobs.iter().filter(|j| j.status == Status::Running).count());
            if state.jobs.iter().all(|j| j.status.is_final()) {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        for j in &state.jobs {
            let serial = j.serial.clone().unwrap_or_default();
            let pm = Command::new(&adb).args(["-s", &serial, "shell", "pm", "path", "com.telkomsel.telkomselcm"]).output().unwrap();
            println!("{serial} app: {}", String::from_utf8_lossy(&pm.stdout).lines().next().unwrap_or("NOT INSTALLED"));
        }
        for avd in &avds {
            let _ = fleet.stop(&adb, avd);
        }
        for j in &state.jobs {
            println!("J{} {} on {:?}: {:?} exit {:?} {:?}", j.id, j.scenario, j.serial, j.status, j.exit_code, j.note);
            println!("  evidence: {:?}", j.evidence_dir);
            println!("{}", engine.log_tail(j.id, 12).unwrap());
        }
        println!("summary: {}", out.join(&run).join("reports").join("summary.json").display());
        std::thread::sleep(Duration::from_secs(25)); // let the emulators shut down
        assert_eq!(peak, 2, "both workers ran at the same time");
        let serials: std::collections::HashSet<_> = state.jobs.iter().map(|j| j.serial.clone()).collect();
        assert_eq!(serials.len(), 2, "one device per worker");
    }

    /// The QA skill on whatever phones are connected (no emulators): jobs share the devices,
    /// with sessions when `AQAOS_ADB` is the bundled adb (scrcpy next to it). Same environment
    /// variables as the emulator test above; an `AQAOS_E2E` item without `:ids` is a whole set.
    #[test]
    #[ignore = "runs the QA skill on the connected phones"]
    fn skill_run_on_connected_devices() {
        let adb = std::env::var("AQAOS_ADB").map(PathBuf::from).ok().or_else(|| crate::emulator::locate_sdk().and_then(|s| s.adb)).expect("adb");
        let devices = ready_devices(&adb);
        assert!(!devices.is_empty(), "connect a phone with USB debugging on");
        let out = std::env::temp_dir().join(format!("aqaos-phone-{}", now()));
        let cfg = QaConfig {
            skill_dir: std::env::var("AQAOS_SKILL_DIR").expect("AQAOS_SKILL_DIR"),
            artifacts_dir: out.to_string_lossy().to_string(),
            timeout_secs: 480,
            ..QaConfig::default()
        };
        let testsets = cfg.testsets().unwrap();
        let scenarios: Vec<Scenario> = std::env::var("AQAOS_E2E")
            .expect("AQAOS_E2E")
            .split(',')
            .map(|item| {
                let (file, only) = item.split_once(':').unwrap_or((item, ""));
                let testset = testsets.iter().find(|t| t.ends_with(file)).expect("test set").clone();
                Scenario { testset, only: Some(only.to_string()), ..Default::default() }
            })
            .collect();

        let engine = QaEngine::new();
        let run = engine.submit(&cfg, &adb, scenarios).unwrap();
        let mut peak = 0;
        let mut state = engine.state();
        for _ in 0..1200 {
            state = engine.state();
            let running: Vec<&Job> = state.jobs.iter().filter(|j| j.status == Status::Running).collect();
            peak = peak.max(running.len());
            // A session job has its display once it opens; until then only its lease marks the place.
            let shown: Vec<_> = running.iter().filter_map(|j| j.display.map(|d| (j.serial.clone(), d))).collect();
            let places: std::collections::HashSet<_> = shown.iter().cloned().collect();
            assert_eq!(places.len(), shown.len(), "a display was given to two workers");
            for l in state.leases.iter().filter(|l| l.slot.is_none()) {
                assert_eq!(state.leases.iter().filter(|o| o.phone == l.phone).count(), 1, "a whole phone was shared");
            }
            if state.jobs.iter().all(|j| j.status.is_final()) {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        for j in &state.jobs {
            println!("J{} {} on {:?} display {:?} only {:?}: {:?} exit {:?} {:?}", j.id, j.scenario, j.serial, j.display, j.only, j.status, j.exit_code, j.note);
            println!("  evidence: {:?}", j.evidence_dir);
            println!("{}", engine.log_tail(j.id, 10).unwrap());
        }
        println!("devices {devices:?}, peak parallel {peak}");
        println!("summary: {}", out.join(&run).join("reports").join("summary.json").display());
        assert!(state.jobs.iter().all(|j| j.status.is_final()), "jobs finished");
        assert!(peak <= devices.len() * 2, "never more workers than places");
    }
}
