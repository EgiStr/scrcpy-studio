//! AI-driven QA (PRD phase 3) through a model CLI that speaks Claude Code's headless
//! protocol (`-p --output-format json|stream-json`). The model plans and drives; every
//! device action is a Telkomsel QA Skill MCP tool on one leased serial; a pre-tool hook
//! refuses anything that could spend money, type a secret or leave the lease; and the
//! verdict comes from the run bundle the skill recorded, never from the model's words.

use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub const APP_PACKAGE: &str = "com.telkomsel.telkomselcm";
const SERVER: &str = "telkomsel-qa";

/// Tools the driving agent may call. Anything else (shell, files, installs, settings,
/// other devices) is denied by the CLI and again by the hook.
const DRIVE_TOOLS: &[&str] = &[
    "adb_screen_read",
    "adb_screen_find",
    "adb_screen_wait",
    "adb_screen_assert",
    "adb_screen_activity",
    "adb_input_tap",
    "adb_input_type",
    "adb_input_key",
    "adb_input_scroll",
    "adb_input_scroll_until",
    "adb_apps_launch",
    "adb_apps_stop",
    "adb_apps_info",
    "adb_capture_shot",
    "adb_logs_crashes",
    "qa_record_start",
    "qa_record_status",
    "qa_record_step",
    "qa_record_take",
    "qa_record_fail",
    "qa_record_finish",
];

/// The skill's own list of labels that may spend money, send data or destroy state
/// (`android-adb/scripts/_safety.py`). A planned goal naming one of these is refused.
const RISKY_WORDS: &[&str] = &[
    "beli", "bayar", "pay", "buy", "purchase", "order", "checkout", "kirim", "send", "transfer", "hapus",
    "delete", "remove", "konfirmasi", "confirm", "submit", "topup", "tukar", "redeem", "aktifkan",
    "aktivasi", "activate", "langganan", "subscribe", "logout", "keluar", "reset", "unduh", "download",
    "install",
];

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub command: String,
    pub model: String,
    pub python: PathBuf,
    /// `skills/mytelkomsel-qa`: the planner reads its test sets from here.
    pub skill_root: PathBuf,
    /// `mcp/telkomsel-qa-mcp/serve.py`.
    pub mcp_server: PathBuf,
    pub max_actions: u32,
}

/// A command line as program + arguments; double quotes group a path with spaces.
fn split_command(line: &str) -> (String, Vec<String>) {
    let (mut parts, mut cur, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    for c in line.trim().chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    parts.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        parts.push(cur);
    }
    let program = if parts.is_empty() { String::new() } else { parts.remove(0) };
    (program, parts)
}

/// The CLI as the user would start it: when the app itself was launched from inside a
/// Claude session, that session's variables (child-session flags, its API base URL, MCP
/// tool naming) are not passed on. An API key stays.
fn base_command(line: &str) -> Command {
    let (program, args) = split_command(line);
    let mut cmd = Command::new(program);
    cmd.args(args);
    for (key, _) in std::env::vars_os() {
        let k = key.to_string_lossy();
        if k.starts_with("CLAUDE") || k == "ANTHROPIC_BASE_URL" {
            cmd.env_remove(&key);
        }
    }
    cmd
}

// ---- planning ----

/// One scenario of a plan: either cases of an existing test set (run by `run.py`, no
/// model in the loop) or a goal the agent drives toward and records.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PlanItem {
    /// "testset" or "goal".
    pub kind: String,
    pub testset: Option<String>,
    pub only: Option<String>,
    pub id: Option<String>,
    pub topic: Option<String>,
    pub goal: Option<String>,
    /// The skill's keypoint form: `re:<regex> :: <label>`.
    pub keypoints: Vec<String>,
    pub expected: Option<String>,
    pub why: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub items: Vec<PlanItem>,
    /// Scenarios the model proposed that were refused, with the reason.
    pub rejected: Vec<String>,
    pub cost_usd: Option<f64>,
}

fn plan_prompt(objective: &str) -> String {
    format!(
        r#"You plan QA for the MyTelkomsel Android app ({APP_PACKAGE}). Turn the objective below into a short test plan.

The current directory is the mytelkomsel-qa skill. Its test sets are testsets/*.yaml; each has cases with an id, topic and question or goal. Read them first (Glob, Grep, Read) and prefer existing cases: they run without a model. Only add a goal scenario when no existing case covers part of the objective.

A goal scenario is a screen to reach from the home screen the way a customer would, in one Indonesian sentence like testsets/goal-scenarios.yaml. It never buys, pays, sends, tops up, subscribes, deletes or logs out, and contains no personal data. Its keypoints are what that final screen must show, in the form "re:<regex> :: <label>".

Answer with one JSON object and nothing else:
{{"items":[
  {{"kind":"testset","testset":"<file name in testsets/>","only":"<comma-separated case ids>","why":"<one sentence>"}},
  {{"kind":"goal","id":"AI-001","topic":"<short>","goal":"<Indonesian sentence>","keypoints":["re:... :: ..."],"expected":"<what the final screen shows>","why":"<one sentence>"}}
]}}
At most 6 items.

Objective: {objective}"#
    )
}

/// Ask the model for a plan. Read-only: it may look at the skill's test sets and nothing else.
pub fn plan(cfg: &AgentConfig, objective: &str) -> Result<Plan> {
    let objective = objective.trim();
    if objective.is_empty() {
        return Err(AppError::InvalidArgs("write an objective first".into()));
    }
    let mut cmd = base_command(&cfg.command);
    cmd.arg("-p")
        .arg(plan_prompt(objective))
        .args(["--model", &cfg.model, "--output-format", "json"])
        .args(["--allowedTools", "Read,Glob,Grep"])
        .args(["--disallowedTools", "Bash,Edit,Write,NotebookEdit,WebFetch,WebSearch"])
        .args(["--strict-mcp-config", "--setting-sources", ""])
        .current_dir(&cfg.skill_root)
        .stdin(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd
        .output()
        .map_err(|e| AppError::NotFound(format!("the AI command \"{}\" ({e})", cfg.command)))?;
    let reply: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        AppError::Io(format!(
            "the AI command gave no JSON (exit {:?}): {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    })?;
    let text = reply["result"].as_str().unwrap_or_default();
    if reply["is_error"].as_bool().unwrap_or(false) {
        return Err(AppError::Io(format!("the AI could not plan: {text}")));
    }
    let raw = extract_json(text).ok_or_else(|| AppError::Io(format!("the plan is not JSON: {text}")))?;
    let items: Vec<PlanItem> = serde_json::from_value(raw["items"].clone())
        .map_err(|e| AppError::Io(format!("the plan does not match the expected shape: {e}")))?;
    let (items, rejected) = validate(items, &cfg.skill_root.join("testsets"));
    Ok(Plan { items, rejected, cost_usd: reply["total_cost_usd"].as_f64() })
}

/// The first `{...}` in a model reply (it may wrap the JSON in prose or a code fence).
fn extract_json(text: &str) -> Option<serde_json::Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(text.get(start..=end)?).ok()
}

fn words(text: &str) -> Vec<String> {
    let lower = text.to_lowercase().replace("top up", "topup").replace("top-up", "topup");
    lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

fn risky_word(text: &str) -> Option<String> {
    words(text).into_iter().find(|w| RISKY_WORDS.contains(&w.as_str()))
}

/// Keep what can run safely: test sets that exist (resolved to absolute paths) and goals
/// that name no account change. Everything else is reported back, never run.
fn validate(items: Vec<PlanItem>, testsets: &Path) -> (Vec<PlanItem>, Vec<String>) {
    let (mut ok, mut rejected) = (Vec::new(), Vec::new());
    let mut n = 0;
    for mut it in items {
        match it.kind.as_str() {
            "testset" => {
                let name = it.testset.as_deref().map(|t| t.replace('\\', "/")).unwrap_or_default();
                let file = name.rsplit('/').next().unwrap_or_default();
                let path = testsets.join(file);
                if file.is_empty() || !path.is_file() {
                    rejected.push(format!("test set \"{name}\" does not exist"));
                    continue;
                }
                it.testset = Some(path.to_string_lossy().to_string());
                it.only = it.only.map(|o| o.split(',').map(str::trim).filter(|x| !x.is_empty()).collect::<Vec<_>>().join(","));
                ok.push(it);
            }
            "goal" => {
                let goal = it.goal.clone().unwrap_or_default();
                if goal.trim().is_empty() {
                    rejected.push("a goal scenario without a goal".into());
                } else if let Some(w) = risky_word(&format!("{goal} {}", it.topic.clone().unwrap_or_default())) {
                    rejected.push(format!("goal \"{goal}\" names \"{w}\", an account change the agent may not make"));
                } else {
                    n += 1;
                    let id: String = it.id.clone().unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
                    it.id = Some(if id.is_empty() { format!("AI-{n:03}") } else { id });
                    it.topic = it.topic.or_else(|| Some(goal.chars().take(60).collect()));
                    ok.push(it);
                }
            }
            other => rejected.push(format!("unknown scenario kind \"{other}\"")),
        }
    }
    (ok, rejected)
}

// ---- driving one goal ----

/// A goal case as a one-case test set in the skill's schema. JSON is valid YAML, so the
/// skill's loader reads it as-is.
fn testset_json(case: &PlanItem, max_actions: u32) -> serde_json::Value {
    serde_json::json!({
        "name": "aqaos-ai",
        "description": "Goal scenario planned by the AQAOS agent.",
        "app": APP_PACKAGE,
        "surface": "goal",
        "source": "app",
        "method": "m2",
        "repeat": 1,
        "max_steps": max_actions,
        "cases": [{
            "id": case.id,
            "topic": case.topic,
            "question_type": "scenario",
            "difficulty": "medium",
            "goal": case.goal,
            "expected_keypoints": case.keypoints,
            "expected_answer": case.expected.clone().unwrap_or_default(),
        }],
    })
}

fn drive_prompt(serial: &str, dir: &Path, case: &PlanItem, max_actions: u32) -> String {
    let run = fwd(dir);
    let id = case.id.as_deref().unwrap_or("AI-001");
    let labels: Vec<&str> = case.keypoints.iter().map(|k| k.rsplit("::").next().unwrap_or(k).trim()).collect();
    format!(
        r#"You are the AQAOS test agent. You drive one Android phone (serial {serial}) with the telkomsel-qa tools toward a goal in the MyTelkomsel app ({APP_PACKAGE}), the way a customer would, and record every step.

Run directory: {run}
Test set: {run}/plan.yaml
Case: {id}
Goal: {goal}
The final screen must show: {labels}

1. qa_record_start with run "{run}" and testsets ["{run}/plan.yaml"].
2. If MyTelkomsel is not in front, adb_apps_launch it. Then qa_record_step for case {id} with action "start".
3. Repeat: adb_screen_read; pick one action (tap by the visible label or text, type ASCII, key BACK, scroll); do it; then qa_record_step with the action and why (one sentence, no personal data).
4. When the screen shows the goal: qa_record_take for case {id}. If you cannot get there within {max_actions} actions, or a login, error or a screen you must not touch blocks the way: qa_record_fail for case {id} with the reason.
5. Always end with qa_record_finish with run "{run}".

Rules: never buy, pay, send, top up, subscribe, delete, log out or type a PIN, OTP or password; such actions are refused. Tap by label or text, never by coordinates. Use only device {serial}. The verdict is graded from what you record, not from what you say, so do not claim a result. End with one sentence on what happened."#,
        goal = case.goal.as_deref().unwrap_or_default(),
        labels = labels.join(", "),
    )
}

fn fwd(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// The pre-tool hook. Exit 2 makes the CLI refuse the call and tell the model why.
/// Keep these rules identical to [`Guard::violation`].
const GUARD_PY: &str = r#"import json, re, sys
SERIAL = __SERIAL__
RUN = __RUN__
PREFIX = "mcp__telkomsel-qa__"

def norm(p):
    return str(p or "").replace("\\", "/").rstrip("/").lower()

def violation(name, args):
    if not name.startswith(PREFIX):
        return "only the telkomsel-qa device tools are allowed"
    short = name[len(PREFIX):]
    for key in ("confirm", "safe"):
        if args.get(key):
            return "`%s` is for a human to approve; the agent cannot approve risky actions" % key
    if args.get("serial") not in (None, "", SERIAL):
        return "this agent may only use device " + SERIAL
    if short.startswith("qa_record_") and norm(args.get("run")) != norm(RUN):
        return "record into the run directory " + RUN
    if short == "adb_input_tap":
        t = args.get("target")
        t = " ".join(str(x) for x in t) if isinstance(t, list) else str(t or "")
        if re.fullmatch(r"\s*-?\d+[\s,]+-?\d+\s*", t):
            return "tap by the visible label or text, not by coordinates, so the safety check can read what is tapped"
    return None

data = json.load(sys.stdin)
why = violation(data.get("tool_name", ""), data.get("tool_input") or {})
if why:
    print("AQAOS guard: " + why, file=sys.stderr)
    sys.exit(2)
"#;

/// The same rules as the hook, applied to the stream as a second line of defence. An
/// attempt to approve a risky action or leave the lease stops the job at once; any other
/// refused call stops it only if the hook did not refuse it (the hook was bypassed).
#[derive(Debug, Clone)]
pub struct Guard {
    pub serial: String,
    pub run: PathBuf,
}

/// What one stream-json line says about tool calls.
#[derive(Debug, Default)]
pub struct Scan {
    pub calls: u32,
    /// A call that must stop the job now.
    pub stop: Option<String>,
    /// Calls the hook must refuse: (tool_use id, reason).
    pub watch: Vec<(String, String)>,
    /// Answers: (tool_use id, refused by the hook).
    pub answers: Vec<(String, bool)>,
}

impl Guard {
    /// Approving a risky action or touching another device: never acceptable, even once.
    fn severe(&self, args: &serde_json::Value) -> bool {
        let truthy = |v: &serde_json::Value| match v {
            serde_json::Value::Null => false,
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::String(s) => !s.is_empty(),
            serde_json::Value::Array(a) => !a.is_empty(),
            _ => true,
        };
        truthy(&args["confirm"]) || truthy(&args["safe"]) || args["serial"].as_str().is_some_and(|s| !s.is_empty() && s != self.serial)
    }

    pub fn violation(&self, name: &str, args: &serde_json::Value) -> Option<String> {
        let prefix = format!("mcp__{SERVER}__");
        let Some(short) = name.strip_prefix(&prefix) else {
            return Some("only the telkomsel-qa device tools are allowed".into());
        };
        let truthy = |v: &serde_json::Value| match v {
            serde_json::Value::Null => false,
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::String(s) => !s.is_empty(),
            serde_json::Value::Array(a) => !a.is_empty(),
            _ => true,
        };
        for key in ["confirm", "safe"] {
            if truthy(&args[key]) {
                return Some(format!("`{key}` is for a human to approve; the agent cannot approve risky actions"));
            }
        }
        match args["serial"].as_str() {
            None | Some("") => {}
            Some(s) if s == self.serial => {}
            Some(_) => return Some(format!("this agent may only use device {}", self.serial)),
        }
        let norm = |s: &str| s.replace('\\', "/").trim_end_matches('/').to_lowercase();
        if short.starts_with("qa_record_") && norm(args["run"].as_str().unwrap_or_default()) != norm(&fwd(&self.run)) {
            return Some(format!("record into the run directory {}", fwd(&self.run)));
        }
        if short == "adb_input_tap" {
            let target = match &args["target"] {
                serde_json::Value::Array(a) => a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or(x.to_string())).collect::<Vec<_>>().join(" "),
                v => v.as_str().unwrap_or_default().to_string(),
            };
            let parts: Vec<&str> = target.split(|c: char| c.is_whitespace() || c == ',').filter(|p| !p.is_empty()).collect();
            if parts.len() == 2 && parts.iter().all(|p| p.trim_start_matches('-').chars().all(|c| c.is_ascii_digit()) && !p.trim_start_matches('-').is_empty()) {
                return Some("tap by the visible label or text, not by coordinates, so the safety check can read what is tapped".into());
            }
        }
        None
    }

    pub fn scan(&self, line: &str) -> Scan {
        let mut out = Scan::default();
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return out };
        for c in v["message"]["content"].as_array().into_iter().flatten() {
            let id = c["id"].as_str().or(c["tool_use_id"].as_str()).unwrap_or_default().to_string();
            match c["type"].as_str() {
                Some("tool_use") => {
                    out.calls += 1;
                    if let Some(why) = self.violation(c["name"].as_str().unwrap_or_default(), &c["input"]) {
                        if self.severe(&c["input"]) {
                            out.stop.get_or_insert(why);
                        } else {
                            out.watch.push((id, why));
                        }
                    }
                }
                Some("tool_result") => {
                    let text = match &c["content"] {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    out.answers.push((id, c["is_error"].as_bool().unwrap_or(false) && text.contains("AQAOS guard")));
                }
                _ => {}
            }
        }
        out
    }
}

/// Write the case, MCP config and guard into `dir` and build the CLI command that drives
/// it. The caller pipes stdout (the stream-json trace) and sets the device environment.
pub fn command(cfg: &AgentConfig, serial: &str, dir: &Path, case: &PlanItem, state_dir: &Path, path_env: Option<&std::ffi::OsStr>) -> std::io::Result<Command> {
    let write_json = |name: &str, v: &serde_json::Value| std::fs::write(dir.join(name), serde_json::to_string_pretty(v).unwrap_or_default());
    write_json("plan.yaml", &testset_json(case, cfg.max_actions))?;

    let mut env = serde_json::json!({
        "ANDROID_SERIAL": serial,
        "ANDROID_SKILLS_STATE": state_dir.to_string_lossy(),
        "PYTHONIOENCODING": "utf-8",
        "PYTHONUTF8": "1",
    });
    if let Some(p) = path_env {
        env["PATH"] = serde_json::Value::String(p.to_string_lossy().to_string());
    }
    write_json("mcp.json", &serde_json::json!({
        "mcpServers": { SERVER: { "command": cfg.python, "args": [cfg.mcp_server], "env": env } }
    }))?;

    let guard = GUARD_PY
        .replace("__SERIAL__", &serde_json::to_string(serial).unwrap_or_default())
        .replace("__RUN__", &serde_json::to_string(&fwd(dir)).unwrap_or_default());
    std::fs::write(dir.join("guard.py"), guard)?;
    let hook = format!("\"{}\" \"{}/guard.py\"", fwd(&cfg.python), fwd(dir));
    write_json("settings.json", &serde_json::json!({
        "hooks": { "PreToolUse": [{ "matcher": ".*", "hooks": [{ "type": "command", "command": hook }] }] }
    }))?;

    let allowed: Vec<String> = DRIVE_TOOLS.iter().map(|t| format!("mcp__{SERVER}__{t}")).collect();
    let mut cmd = base_command(&cfg.command);
    cmd.arg("-p")
        .arg(drive_prompt(serial, dir, case, cfg.max_actions))
        .args(["--model", &cfg.model, "--output-format", "stream-json", "--verbose"])
        .arg("--mcp-config")
        .arg(dir.join("mcp.json"))
        .arg("--strict-mcp-config")
        .arg("--settings")
        .arg(dir.join("settings.json"))
        .args(["--setting-sources", ""])
        .arg("--allowedTools")
        .arg(allowed.join(","))
        .args(["--disallowedTools", "Bash,Edit,Write,NotebookEdit,WebFetch,WebSearch,Read,Glob,Grep"]);
    Ok(cmd)
}

/// The verdict of an agent job, from the bundle the skill wrote. No bundle or no graded
/// case is an ERROR: the agent's own account of the run never counts.
pub fn verdict(dir: &Path) -> (crate::qa::Status, Option<String>) {
    use crate::qa::Status;
    let bundle = std::fs::read_to_string(dir.join("run.json")).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    let Some(bundle) = bundle else {
        return (Status::Error, Some("the agent recorded no evidence (no run.json)".into()));
    };
    let cases = bundle["cases"].as_array().cloned().unwrap_or_default();
    let v: Vec<&str> = cases.iter().filter_map(|c| c["verdict"].as_str()).collect();
    if v.is_empty() {
        return (Status::Error, Some("run.json has no graded case".into()));
    }
    if v.iter().any(|x| *x == "fail" || *x == "flaky") {
        (Status::Failed, None)
    } else if v.iter().all(|x| *x == "pass") {
        (Status::Passed, None)
    } else {
        let why = cases
            .iter()
            .flat_map(|c| c["runs"].as_array().cloned().unwrap_or_default())
            .find_map(|r| r["error"].as_str().map(str::to_string));
        (Status::Error, why.or(Some("the goal was not reached".into())))
    }
}

// ---- trace ----

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceEvent {
    /// "say" (model text), "tool" (a call), "result" (its answer), "done" (the end).
    pub kind: String,
    pub name: Option<String>,
    pub detail: String,
    pub error: bool,
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// The readable events of a stream-json trace: what the model said, each tool call with
/// its arguments, each answer, and the final result. Images are summarised, not inlined.
pub fn parse_trace(text: &str) -> Vec<TraceEvent> {
    let mut out = Vec::new();
    let prefix = format!("mcp__{SERVER}__");
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let content = v["message"]["content"].as_array().cloned().unwrap_or_default();
        match v["type"].as_str() {
            Some("assistant") => {
                for c in content {
                    match c["type"].as_str() {
                        Some("text") => {
                            let t = c["text"].as_str().unwrap_or_default().trim();
                            if !t.is_empty() {
                                out.push(TraceEvent { kind: "say".into(), name: None, detail: clip(t, 600), error: false });
                            }
                        }
                        Some("tool_use") => {
                            let name = c["name"].as_str().unwrap_or_default();
                            out.push(TraceEvent {
                                kind: "tool".into(),
                                name: Some(name.strip_prefix(&prefix).unwrap_or(name).to_string()),
                                detail: clip(&c["input"].to_string(), 300),
                                error: false,
                            });
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                for c in content.into_iter().filter(|c| c["type"] == "tool_result") {
                    let detail = match &c["content"] {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Array(parts) => parts
                            .iter()
                            .map(|p| if p["type"] == "image" { "[screenshot]".to_string() } else { p["text"].as_str().unwrap_or_default().to_string() })
                            .collect::<Vec<_>>()
                            .join("\n"),
                        _ => String::new(),
                    };
                    out.push(TraceEvent { kind: "result".into(), name: None, detail: clip(detail.trim(), 400), error: c["is_error"].as_bool().unwrap_or(false) });
                }
            }
            Some("result") => {
                let mut detail = v["result"].as_str().unwrap_or_default().trim().to_string();
                if let Some(cost) = v["total_cost_usd"].as_f64() {
                    detail.push_str(&format!(" (turns {}, ${cost:.3})", v["num_turns"].as_u64().unwrap_or(0)));
                }
                out.push(TraceEvent { kind: "done".into(), name: None, detail: clip(&detail, 600), error: v["is_error"].as_bool().unwrap_or(false) });
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn guard() -> Guard {
        Guard { serial: "33061JEHN09954".into(), run: PathBuf::from(r"C:\art\RUN-1\33061JEHN09954\J1-goal_AI-001") }
    }

    /// (tool, input, refused?) — shared by the Rust guard and the Python hook tests.
    fn cases() -> Vec<(&'static str, serde_json::Value, bool)> {
        let run = "C:/art/RUN-1/33061JEHN09954/J1-goal_AI-001";
        vec![
            ("mcp__telkomsel-qa__adb_screen_read", json!({}), false),
            ("mcp__telkomsel-qa__adb_input_tap", json!({"target": ["Cari", "Produk"]}), false),
            ("mcp__telkomsel-qa__adb_input_tap", json!({"target": ["540", "1200"]}), true),
            ("mcp__telkomsel-qa__adb_input_tap", json!({"target": "540,1200"}), true),
            ("mcp__telkomsel-qa__adb_input_tap", json!({"target": ["Beli"], "confirm": true}), true),
            ("mcp__telkomsel-qa__adb_input_tap", json!({"target": ["Beli"], "safe": ["Beli"]}), true),
            ("mcp__telkomsel-qa__adb_input_type", json!({"text": ["123456"], "confirm": true}), true),
            ("mcp__telkomsel-qa__adb_screen_read", json!({"serial": "emulator-5554"}), true),
            ("mcp__telkomsel-qa__adb_screen_read", json!({"serial": "33061JEHN09954"}), false),
            ("mcp__telkomsel-qa__qa_record_start", json!({"run": run, "testsets": [format!("{run}/plan.yaml")]}), false),
            ("mcp__telkomsel-qa__qa_record_step", json!({"run": r"C:\art\RUN-1\33061JEHN09954\J1-goal_AI-001\", "case": "AI-001"}), false),
            ("mcp__telkomsel-qa__qa_record_take", json!({"run": "C:/elsewhere", "case": "AI-001"}), true),
            ("Bash", json!({"command": "adb shell input tap 1 1"}), true),
        ]
    }

    #[test]
    fn guard_refuses_risky_or_out_of_lease_calls() {
        let g = guard();
        for (name, input, refused) in cases() {
            assert_eq!(g.violation(name, &input).is_some(), refused, "{name} {input}");
        }
    }

    #[test]
    fn python_hook_applies_the_same_rules() {
        let python = std::env::var("AQAOS_PYTHON").unwrap_or_else(|_| "python".into());
        if Command::new(&python).arg("--version").output().is_err() {
            eprintln!("skipped: no python");
            return;
        }
        let g = guard();
        let dir = tempfile::tempdir().unwrap();
        let script = GUARD_PY
            .replace("__SERIAL__", &serde_json::to_string(&g.serial).unwrap())
            .replace("__RUN__", &serde_json::to_string(&fwd(&g.run)).unwrap());
        let path = dir.path().join("guard.py");
        std::fs::write(&path, script).unwrap();
        for (name, input, refused) in cases() {
            let mut child = Command::new(&python).arg(&path).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
            use std::io::Write;
            child.stdin.take().unwrap().write_all(json!({"tool_name": name, "tool_input": input}).to_string().as_bytes()).unwrap();
            let out = child.wait_with_output().unwrap();
            assert_eq!(out.status.code() == Some(2), refused, "{name} {input}: {}", String::from_utf8_lossy(&out.stderr));
        }
    }

    #[test]
    fn plan_keeps_known_test_sets_and_safe_goals_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("search-playbook.yaml"), "cases: []").unwrap();
        let items = vec![
            PlanItem { kind: "testset".into(), testset: Some("testsets/search-playbook.yaml".into()), only: Some(" SRC-T001 , SRC-T008 ".into()), ..Default::default() },
            PlanItem { kind: "testset".into(), testset: Some("missing.yaml".into()), ..Default::default() },
            PlanItem { kind: "goal".into(), goal: Some("Dari beranda, buka menu Roaming untuk Jepang.".into()), keypoints: vec!["re:jepang :: Japan".into()], ..Default::default() },
            PlanItem { kind: "goal".into(), goal: Some("Beli paket 10 GB lalu bayar".into()), ..Default::default() },
            PlanItem { kind: "goal".into(), goal: Some("Buka halaman Top-Up pulsa".into()), ..Default::default() },
            PlanItem { kind: "shell".into(), ..Default::default() },
        ];
        let (ok, rejected) = validate(items, dir.path());
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[0].testset.as_deref(), Some(dir.path().join("search-playbook.yaml").to_string_lossy().as_ref()));
        assert_eq!(ok[0].only.as_deref(), Some("SRC-T001,SRC-T008"));
        assert_eq!(ok[1].id.as_deref(), Some("AI-001"));
        assert_eq!(rejected.len(), 4, "{rejected:?}");
    }

    #[test]
    fn ai_command_may_carry_its_own_arguments() {
        assert_eq!(split_command("claude"), ("claude".into(), vec![]));
        assert_eq!(
            split_command(r#""C:\Program Files\x\py.exe" "D:\a b\fake.py" pass"#),
            (r"C:\Program Files\x\py.exe".into(), vec![r"D:\a b\fake.py".into(), "pass".into()])
        );
    }

    #[test]
    fn plan_json_is_found_inside_prose_or_fences() {
        let v = extract_json("Here is the plan:\n```json\n{\"items\":[{\"kind\":\"goal\"}]}\n```").unwrap();
        assert_eq!(v["items"][0]["kind"], "goal");
        assert!(extract_json("no json here").is_none());
    }

    #[test]
    fn verdict_comes_from_the_bundle_never_from_the_model() {
        use crate::qa::Status;
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(verdict(dir.path()).0, Status::Error, "no bundle is never a pass");
        let write = |cases: &str| std::fs::write(dir.path().join("run.json"), format!("{{\"cases\":[{cases}]}}")).unwrap();
        write("");
        assert_eq!(verdict(dir.path()).0, Status::Error);
        write(r#"{"verdict":"pass"}"#);
        assert_eq!(verdict(dir.path()).0, Status::Passed);
        write(r#"{"verdict":"flaky"}"#);
        assert_eq!(verdict(dir.path()).0, Status::Failed);
        write(r#"{"verdict":"error","runs":[{"status":"error","error":"a login sheet blocks the way"}]}"#);
        assert_eq!(verdict(dir.path()), (Status::Error, Some("a login sheet blocks the way".into())));
    }

    const STREAM: &str = r#"{"type":"system","subtype":"init","model":"claude-fable-5-2"}
{"type":"assistant","message":{"content":[{"type":"text","text":"Starting the run."},{"type":"tool_use","id":"t1","name":"mcp__telkomsel-qa__qa_record_start","input":{"run":"C:/art/RUN-1/33061JEHN09954/J1-goal_AI-001","testsets":["x"]}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"record.py: exit 0 (ok)"}]}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"mcp__telkomsel-qa__adb_input_tap","input":{"target":["540","1200"]}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","is_error":true,"content":"AQAOS guard: tap by the visible label"},{"type":"tool_result","tool_use_id":"t3","content":[{"type":"image","source":{}}]}]}}
{"type":"result","subtype":"success","is_error":false,"result":"Reached the roaming list.","num_turns":4,"total_cost_usd":0.0123}
"#;

    #[test]
    fn trace_shows_calls_answers_and_the_end() {
        let t = parse_trace(STREAM);
        let kinds: Vec<&str> = t.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, ["say", "tool", "result", "tool", "result", "result", "done"]);
        assert_eq!(t[1].name.as_deref(), Some("qa_record_start"));
        assert!(t[4].error);
        assert_eq!(t[5].detail, "[screenshot]");
        assert!(t[6].detail.contains("$0.012"));
    }

    #[test]
    fn stream_scan_counts_calls_and_watches_refused_ones() {
        let g = guard();
        let lines: Vec<Scan> = STREAM.lines().map(|l| g.scan(l)).collect();
        assert_eq!(lines.iter().map(|l| l.calls).sum::<u32>(), 2);
        assert!(lines.iter().all(|l| l.stop.is_none()));
        assert_eq!(lines[3].watch.len(), 1, "a coordinate tap waits for the hook's answer");
        assert_eq!(lines[3].watch[0].0, "t2");
        assert_eq!(lines[4].answers[0], ("t2".to_string(), true), "the hook refused it");
        let approve = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t9","name":"mcp__telkomsel-qa__adb_input_tap","input":{"target":["Beli"],"confirm":true}}]}}"#;
        assert!(g.scan(approve).stop.is_some(), "approving a risky tap stops the job at once");
    }

    #[test]
    fn command_writes_case_config_and_hook_into_the_job_dir() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AgentConfig {
            command: "claude".into(),
            model: "claude-fable-5-2".into(),
            python: "python".into(),
            skill_root: dir.path().into(),
            mcp_server: PathBuf::from("D:/skills/mcp/telkomsel-qa-mcp/serve.py"),
            max_actions: 40,
        };
        let case = PlanItem { kind: "goal".into(), id: Some("AI-001".into()), goal: Some("Buka Roaming".into()), keypoints: vec!["re:roaming :: Roaming".into()], ..Default::default() };
        let cmd = command(&cfg, "SER1", dir.path(), &case, Path::new("C:/state/SER1"), None).unwrap();
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert!(args.iter().any(|a| a.contains("mcp__telkomsel-qa__adb_input_tap")));
        assert!(!args.iter().any(|a| a.contains("adb_apps_install") || a.contains("qa_run")));
        let mcp: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("mcp.json")).unwrap()).unwrap();
        assert_eq!(mcp["mcpServers"]["telkomsel-qa"]["env"]["ANDROID_SERIAL"], "SER1");
        let set: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("plan.yaml")).unwrap()).unwrap();
        assert_eq!(set["surface"], "goal");
        assert_eq!(set["cases"][0]["expected_keypoints"][0], "re:roaming :: Roaming");
        assert!(std::fs::read_to_string(dir.path().join("guard.py")).unwrap().contains("SERIAL = \"SER1\""));
        let removed: Vec<String> = cmd.get_envs().filter(|(_, v)| v.is_none()).map(|(k, _)| k.to_string_lossy().to_string()).collect();
        for (k, _) in std::env::vars().filter(|(k, _)| k.starts_with("CLAUDE")) {
            assert!(removed.contains(&k), "{k} is not passed to the CLI");
        }
    }
}
