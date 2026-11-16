//! Defect intelligence: every case that did not pass becomes a finding with a failure
//! category, what was expected and seen, how to reproduce it, and the evidence files.
//! Categories come from fixed rules over the skill's run.json and the job's outcome, so an
//! automation or device problem is never reported as an app defect. Exports are redacted.

use crate::qa::{Job, Status};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::sync::OnceLock;

/// PRD Module 10's failure categories.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    UiDefect,
    NavigationDefect,
    BusinessRuleDefect,
    AppCrash,
    AutomationError,
    InfrastructureIssue,
    Inconclusive,
}

impl Category {
    /// A product defect, as opposed to a problem with the test setup.
    pub fn is_product(self) -> bool {
        matches!(self, Category::UiDefect | Category::NavigationDefect | Category::BusinessRuleDefect | Category::AppCrash)
    }

    pub fn label(self) -> &'static str {
        match self {
            Category::UiDefect => "UI defect",
            Category::NavigationDefect => "Navigation defect",
            Category::BusinessRuleDefect => "Business-rule defect",
            Category::AppCrash => "Application crash",
            Category::AutomationError => "Automation error",
            Category::InfrastructureIssue => "Device or infrastructure issue",
            Category::Inconclusive => "Inconclusive",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub id: String,
    pub run_id: String,
    pub job_id: u32,
    pub scenario: String,
    pub case_id: String,
    pub verdict: String,
    pub category: Category,
    /// Which rule chose the category.
    pub reason: String,
    pub summary: String,
    pub expected: Vec<String>,
    pub actual: String,
    pub missed: Vec<String>,
    pub steps: Vec<String>,
    /// Paths relative to the job's evidence directory.
    pub evidence: Vec<String>,
    pub evidence_dir: Option<String>,
    pub serial: Option<String>,
    pub device: String,
    pub app_version: String,
}

fn rx(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid pattern"))
}

/// Mask phone numbers, one-time codes and tokens before anything leaves the machine.
/// The phone pattern is the skill's own (run.py MSISDN_RE).
pub fn redact(text: &str) -> String {
    static PHONE: OnceLock<Regex> = OnceLock::new();
    static CODE: OnceLock<Regex> = OnceLock::new();
    static BEARER: OnceLock<Regex> = OnceLock::new();
    static TOKEN: OnceLock<Regex> = OnceLock::new();
    let t = rx(&PHONE, r"(?:\+?62[\s-]?|\b0)8\d{1,3}[\s-]?\d{3,4}[\s-]?\d{3,5}\b").replace_all(text, "<phone>");
    let t = rx(&CODE, r"(?i)\b(otp|kode|code|pin|password|sandi)(\W{1,12})\d{4,8}\b").replace_all(&t, "$1$2<code>");
    let t = rx(&BEARER, r"(?i)\b(bearer|token)(\s*[:=]?\s*)[A-Za-z0-9._~+/=-]{12,}").replace_all(&t, "$1$2<token>");
    rx(&TOKEN, r"\b[A-Za-z0-9_-]{40,}\b").replace_all(&t, "<token>").into_owned()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect()
}

/// The category of a case that did not pass, and the rule that chose it.
fn classify(case: &Value, run: &Value, crashed: bool) -> (Category, String) {
    let verdict = case["verdict"].as_str().unwrap_or_default();
    if crashed {
        return (Category::AppCrash, "the app crashed during the case (FATAL EXCEPTION or ANR in its logs)".into());
    }
    if verdict == "error" || run["status"] == "error" {
        let why = run["error"].as_str().unwrap_or("the case could not be graded");
        return (Category::AutomationError, format!("the run errored before a verdict: {why}"));
    }
    if verdict == "flaky" {
        return (Category::Inconclusive, "the case passed in some repeats and failed in others".into());
    }
    let grade = &run["grade"];
    if grade["violations"].as_array().is_some_and(|v| !v.is_empty()) {
        return (Category::BusinessRuleDefect, "the answer or screen said something it must not".into());
    }
    if run["checks"].as_array().is_some_and(|c| c.iter().any(|c| c["ok"] == false)) {
        return (Category::BusinessRuleDefect, "a business check on the result failed".into());
    }
    match case["surface"].as_str().unwrap_or_default() {
        "goal" => (Category::NavigationDefect, "the goal screen was not reached or lacked required content".into()),
        "veronika" | "chat" => (Category::BusinessRuleDefect, "the assistant's answer lacked required information".into()),
        _ => (Category::UiDefect, "the screen lacked required content".into()),
    }
}

/// A job that ended without a graded case: why, from its status and note.
fn job_category(job: &Job) -> (Category, String) {
    let note = job.note.clone().unwrap_or_default();
    let infra = ["disconnected", "timed out", "interrupted", "app closed", "could not start"];
    if infra.iter().any(|w| note.contains(w)) {
        (Category::InfrastructureIssue, note)
    } else if job.status == Status::Blocked || job.status == Status::Error {
        (Category::AutomationError, if note.is_empty() { "the job ended without a verdict".into() } else { note })
    } else {
        (Category::Inconclusive, "no graded case in the evidence".into())
    }
}

fn crashed(dir: &Path, case_id: &str) -> bool {
    let log = std::fs::read_to_string(dir.join("logs").join(format!("{case_id}.log"))).unwrap_or_default();
    log.contains("FATAL EXCEPTION") || log.contains("ANR in com.telkomsel")
}

/// The findings of one job: none for a pass or a cancelled job.
pub fn findings(job: &Job) -> Vec<Finding> {
    if matches!(job.status, Status::Passed | Status::Cancelled | Status::Queued | Status::Running) {
        return Vec::new();
    }
    let dir = job.evidence_dir.as_deref().map(Path::new);
    let bundle: Value = dir
        .and_then(|d| std::fs::read_to_string(d.join("run.json")).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null);
    let device = &bundle["device"];
    let base = |case_id: &str, verdict: &str, category: Category, reason: String| Finding {
        id: format!("DEF-{}-J{}-{}", job.run_id.trim_start_matches("RUN-"), job.id, case_id),
        run_id: job.run_id.clone(),
        job_id: job.id,
        scenario: job.scenario.clone(),
        case_id: case_id.into(),
        verdict: verdict.into(),
        category,
        reason,
        summary: String::new(),
        expected: Vec::new(),
        actual: String::new(),
        missed: Vec::new(),
        steps: Vec::new(),
        evidence: Vec::new(),
        evidence_dir: job.evidence_dir.clone(),
        serial: job.serial.clone(),
        device: format!("{} · Android {}", device["model"].as_str().unwrap_or("?"), device["android"].as_str().unwrap_or("?")),
        app_version: device["app_version"].as_str().unwrap_or("unknown").into(),
    };

    let cases = bundle["cases"].as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for case in cases.iter().filter(|c| c["verdict"] != "pass") {
        let id = case["id"].as_str().unwrap_or("?");
        let runs = case["runs"].as_array().cloned().unwrap_or_default();
        let run = runs.iter().find(|r| r["status"] != "pass").cloned().unwrap_or(Value::Null);
        let (category, reason) = classify(case, &run, dir.is_some_and(|d| crashed(d, id)));
        let mut f = base(id, case["verdict"].as_str().unwrap_or("?"), category, reason);
        f.missed = run["grade"]["keypoints"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|k| k["hit"] == false)
            .filter_map(|k| k["label"].as_str().map(str::to_string))
            .collect();
        let task = case["goal"].as_str().filter(|g| !g.is_empty()).or(case["question"].as_str()).unwrap_or(id);
        f.summary = match (&f.category, f.missed.is_empty()) {
            (Category::AutomationError, _) => format!("{task}: not graded ({})", run["error"].as_str().unwrap_or("run error")),
            (_, false) => format!("{task}: missing {}", f.missed.join(", ")),
            _ => format!("{task}: {}", f.reason),
        };
        f.expected = case["expected_keypoints"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|k| k.as_str())
            .map(|k| k.rsplit("::").next().unwrap_or(k).trim().to_string())
            .collect();
        if let Some(a) = case["expected_answer"].as_str().filter(|a| !a.is_empty()) {
            f.expected.push(a.to_string());
        }
        f.actual = run["response"].as_str().unwrap_or_default().chars().take(1200).collect();
        f.steps = run["steps"]
            .as_array()
            .map(|s| s.iter().map(|s| format!("{} ({})", s["action"].as_str().unwrap_or("?"), s["why"].as_str().unwrap_or(""))).collect())
            .unwrap_or_else(|| strs(&case["steps"]));
        if f.steps.is_empty() {
            f.steps.push(task.to_string());
        }
        for e in run["evidence"].as_array().into_iter().flatten() {
            f.evidence.extend([&e["shot"], &e["file"]].into_iter().filter_map(|x| x.as_str().map(str::to_string)));
        }
        f.evidence.extend(strs(&run["recording"]["files"]));
        f.evidence.push(format!("logs/{id}.log"));
        out.push(f);
    }

    if out.is_empty() {
        // Nothing graded: the job itself is the finding.
        let (category, reason) = job_category(job);
        let mut f = base("JOB", job.status.as_str(), category, reason.clone());
        f.summary = format!("{}: {reason}", job.scenario);
        f.evidence = ["worker.log", "trace.jsonl"].iter().filter(|p| dir.is_some_and(|d| d.join(p).is_file())).map(|p| p.to_string()).collect();
        out.push(f);
    }
    out
}

// ---- comparison with earlier runs ----

/// How one case did in this run against the last earlier run of the same case.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub job_id: u32,
    pub case_id: String,
    pub key: String,
    pub now: String,
    pub before: Option<String>,
    pub before_run: Option<String>,
    /// "new", "regressed", "fixed", "still failing" or "still passing".
    pub change: String,
}

/// A case's identity across runs. A scripted case is its test set and id. A goal case is
/// its goal and required content, because the planner hands out AI-NNN ids afresh.
fn case_key(job: &Job, case: &Value) -> String {
    use sha2::{Digest, Sha256};
    if case["surface"] == "goal" || job.goal.is_some() {
        let mut h = Sha256::new();
        h.update(case["goal"].as_str().unwrap_or_default().trim().to_lowercase());
        for k in case["expected_keypoints"].as_array().into_iter().flatten() {
            h.update(b"\n");
            h.update(k.as_str().unwrap_or_default());
        }
        let hex: String = h.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect();
        return format!("goal:{hex}");
    }
    let stem = Path::new(&job.testset).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    format!("{stem}:{}", case["id"].as_str().unwrap_or("?"))
}

/// (key, case id, verdict) of every graded case in a job's evidence.
fn outcomes(job: &Job) -> Vec<(String, String, String)> {
    let bundle: Value = job
        .evidence_dir
        .as_deref()
        .and_then(|d| std::fs::read_to_string(Path::new(d).join("run.json")).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null);
    bundle["cases"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| (case_key(job, c), c["id"].as_str().unwrap_or("?").to_string(), c["verdict"].as_str().unwrap_or("?").to_string()))
        .collect()
}

/// Compare every case of `run` with its last outcome in `history` (any order).
pub fn compare(run: &[Job], history: &[Job]) -> Vec<Change> {
    let first = run.iter().map(|j| j.id).min().unwrap_or(0);
    let mut earlier: Vec<&Job> = history.iter().filter(|j| j.id < first).collect();
    earlier.sort_by_key(|j| std::cmp::Reverse(j.id));
    let past: Vec<(&Job, Vec<(String, String, String)>)> = earlier.into_iter().map(|j| (j, outcomes(j))).collect();
    let mut out = Vec::new();
    for job in run {
        for (key, case_id, now) in outcomes(job) {
            let before = past.iter().find_map(|(j, o)| o.iter().find(|(k, _, _)| *k == key).map(|(_, _, v)| (v.clone(), j.run_id.clone())));
            let change = match (&before, now == "pass") {
                (None, _) => "new",
                (Some((b, _)), true) if b == "pass" => "still passing",
                (Some(_), true) => "fixed",
                (Some((b, _)), false) if b == "pass" => "regressed",
                (Some(_), false) => "still failing",
            };
            out.push(Change {
                job_id: job.id,
                case_id,
                key,
                now,
                before: before.as_ref().map(|b| b.0.clone()),
                before_run: before.map(|b| b.1),
                change: change.into(),
            });
        }
    }
    out
}

/// findings.md: a defect report per finding, redacted, product defects first.
pub fn markdown(run_id: &str, jobs: &[Job], findings: &[Finding], changes: &[Change]) -> String {
    let mut md = format!("# QA findings · {run_id}\n\n");
    let product = findings.iter().filter(|f| f.category.is_product()).count();
    md += &format!(
        "{} jobs · {} findings · {} product defects · {} automation, device or inconclusive\n\n",
        jobs.len(),
        findings.len(),
        product,
        findings.len() - product
    );
    md += "Categories come from fixed rules over the recorded evidence. A hypothesis about the cause is not a confirmed root cause.\n\n";
    let moved: Vec<&Change> = changes.iter().filter(|c| c.change == "regressed" || c.change == "fixed").collect();
    if !moved.is_empty() {
        md += "## Compared with earlier runs\n\n";
        for c in moved {
            md += &format!(
                "- **{}** J{} {}: {} (was {} in {})\n",
                c.change,
                c.job_id,
                c.case_id,
                c.now,
                c.before.as_deref().unwrap_or("?"),
                c.before_run.as_deref().unwrap_or("?")
            );
        }
        md += "\n";
    }
    let mut sorted: Vec<&Finding> = findings.iter().collect();
    sorted.sort_by_key(|f| (!f.category.is_product(), f.job_id));
    for f in sorted {
        md += &format!("## {} · {}\n\n", f.id, f.category.label());
        md += &format!("- **Summary:** {}\n", redact(&f.summary));
        md += &format!("- **Verdict:** {} · **Why this category:** {}\n", f.verdict, redact(&f.reason));
        md += &format!("- **Run / job / scenario / case:** {} / J{} / {} / {}\n", f.run_id, f.job_id, f.scenario, f.case_id);
        md += &format!("- **Device:** {} · {} · app {}\n", f.device, f.serial.as_deref().unwrap_or("?"), f.app_version);
        if !f.expected.is_empty() {
            md += "- **Expected:**\n";
            for e in &f.expected {
                md += &format!("  - {}\n", redact(e));
            }
        }
        if !f.missed.is_empty() {
            md += &format!("- **Missing:** {}\n", redact(&f.missed.join(", ")));
        }
        if !f.actual.is_empty() {
            md += &format!("- **Actual (excerpt):**\n\n```text\n{}\n```\n", redact(&f.actual));
        }
        md += "- **Reproduction steps:**\n";
        for (i, s) in f.steps.iter().enumerate() {
            md += &format!("  {}. {}\n", i + 1, redact(s));
        }
        md += &format!("- **Evidence** (in `{}`):\n", f.evidence_dir.as_deref().unwrap_or("?"));
        for e in &f.evidence {
            md += &format!("  - `{e}`\n");
        }
        md += "\n";
    }
    md
}

/// Write `<artifacts>/<run_id>/reports/findings.md` and `findings.json`; return the .md path.
pub fn export(artifacts: &Path, run_id: &str, jobs: &[Job], history: &[Job]) -> std::io::Result<std::path::PathBuf> {
    let findings: Vec<Finding> = jobs.iter().flat_map(findings).collect();
    let changes = compare(jobs, history);
    let dir = artifacts.join(run_id).join("reports");
    std::fs::create_dir_all(&dir)?;
    let json = redact(&serde_json::to_string_pretty(&serde_json::json!({"findings": findings, "changes": changes})).unwrap_or_default());
    std::fs::write(dir.join("findings.json"), json)?;
    let md = dir.join("findings.md");
    std::fs::write(&md, markdown(run_id, jobs, &findings, &changes))?;
    Ok(md)
}

// ---- AI investigator ----

/// One possible cause, as the model sees it. A hypothesis, never a verdict.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Hypothesis {
    pub cause: String,
    /// 0 to 1.
    pub confidence: f64,
    pub supporting: Vec<String>,
    pub against: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Investigation {
    pub finding_id: String,
    pub hypotheses: Vec<Hypothesis>,
    pub limitations: Vec<String>,
    pub next_steps: Vec<String>,
    pub model: String,
    pub cost_usd: Option<f64>,
    pub at: u64,
    pub note: String,
}

const INVESTIGATION_NOTE: &str =
    "AI hypotheses about the cause, read from the evidence. They are not a confirmed root cause and do not change the verdict.";

fn investigate_prompt(f: &Finding) -> String {
    let finding = redact(&serde_json::to_string_pretty(f).unwrap_or_default());
    format!(
        r#"A MyTelkomsel QA case did not pass. Find out why. The current directory is its evidence: run.json (the graded result), logs/, screenshots and step records. Read what you need (Glob, Grep, Read).

The finding, already classified by fixed rules:
{finding}

Give at most 3 hypotheses about the cause, most likely first. Each cites the evidence files and lines that support it and those that argue against it. Say whether the cause looks like the app, the test case (wrong expectation), the automation, or the device. Do not guess beyond the evidence; if it is thin, say so in limitations.

Answer with one JSON object and nothing else:
{{"hypotheses":[{{"cause":"<one sentence>","confidence":0.0,"supporting":["<file: what it shows>"],"against":["<file: what it shows>"]}}],
"limitations":["<what the evidence cannot tell>"],
"nextSteps":["<what a tester should check or rerun>"]}}"#
    )
}

/// The model's reply as an investigation: confidences kept in 0..1, most likely first.
fn parse_investigation(text: &str) -> Option<(Vec<Hypothesis>, Vec<String>, Vec<String>)> {
    let raw = crate::agent::extract_json(text)?;
    let mut hyps: Vec<Hypothesis> = serde_json::from_value(raw["hypotheses"].clone()).ok()?;
    hyps.retain(|h| !h.cause.trim().is_empty());
    for h in hyps.iter_mut() {
        h.confidence = if h.confidence.is_finite() { h.confidence.clamp(0.0, 1.0) } else { 0.0 };
    }
    hyps.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
    hyps.truncate(3);
    let list = |k: &str| -> Vec<String> {
        raw[k].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect()
    };
    Some((hyps, list("limitations"), list("nextSteps")))
}

/// Ask the model for hypotheses. Read-only, inside the evidence directory only. The result
/// is saved next to the evidence as investigation-<case>.json.
pub fn investigate(cfg: &crate::agent::AgentConfig, f: &Finding) -> crate::error::Result<Investigation> {
    use crate::error::AppError;
    let dir = f
        .evidence_dir
        .as_deref()
        .filter(|d| Path::new(d).is_dir())
        .ok_or_else(|| AppError::NotFound(format!("evidence for {}", f.id)))?;
    let mut cmd = crate::agent::base_command(&cfg.command);
    cmd.arg("-p")
        .arg(investigate_prompt(f))
        .args(["--model", &cfg.model, "--output-format", "json"])
        .args(["--allowedTools", "Read,Glob,Grep"])
        .args(["--disallowedTools", "Bash,Edit,Write,NotebookEdit,WebFetch,WebSearch"])
        .args(["--strict-mcp-config", "--setting-sources", ""])
        .current_dir(dir)
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| AppError::NotFound(format!("the AI command \"{}\" ({e})", cfg.command)))?;
    let reply: Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        AppError::Io(format!(
            "the AI command gave no JSON (exit {:?}): {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    })?;
    let text = reply["result"].as_str().unwrap_or_default();
    if reply["is_error"].as_bool().unwrap_or(false) {
        return Err(AppError::Io(format!("the AI could not investigate: {text}")));
    }
    let (hypotheses, limitations, next_steps) = parse_investigation(text)
        .ok_or_else(|| AppError::Io(format!("the investigation is not in the expected JSON: {text}")))?;
    let inv = Investigation {
        finding_id: f.id.clone(),
        hypotheses,
        limitations,
        next_steps,
        model: cfg.model.clone(),
        cost_usd: reply["total_cost_usd"].as_f64(),
        at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        note: INVESTIGATION_NOTE.into(),
    };
    let file = Path::new(dir).join(format!("investigation-{}.json", f.case_id.replace(['/', '\\', ':'], "_")));
    let _ = std::fs::write(file, redact(&serde_json::to_string_pretty(&inv).unwrap_or_default()));
    Ok(inv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn job(dir: &Path, status: Status, note: Option<&str>) -> Job {
        serde_json::from_value(json!({
            "id": 3, "runId": "RUN-1-01", "scenario": "veronika-complex-5", "testset": "x", "only": null,
            "kind": "script", "goal": null, "actions": 0, "status": status, "serial": "33061JEHN09954",
            "evidenceDir": dir.to_string_lossy(), "exitCode": 1, "note": note, "queuedAt": 1,
            "startedAt": 2, "finishedAt": 3
        }))
        .unwrap()
    }

    fn bundle(dir: &Path, cases: Value) {
        let b = json!({"device": {"model": "Pixel 7a", "android": "17", "app_version": "9.4.0"}, "cases": cases});
        std::fs::write(dir.join("run.json"), b.to_string()).unwrap();
    }

    fn run(status: &str, missed: &[&str], extra: Value) -> Value {
        let mut r = json!({
            "status": status, "error": null, "response": "Hubungi 081234567890, kode OTP 482913",
            "grade": {"keypoints": missed.iter().map(|m| json!({"label": m, "hit": false})).collect::<Vec<_>>(), "violations": []},
            "evidence": [{"shot": "evidence/E-1.jpg", "file": "evidence/E-1.txt"}],
            "recording": {"files": ["video/C-1.mp4"]}
        });
        if let (Some(r), Some(e)) = (r.as_object_mut(), extra.as_object()) {
            r.extend(e.clone());
        }
        r
    }

    #[test]
    fn product_defects_are_told_apart_from_automation_and_device_problems() {
        let dir = tempfile::tempdir().unwrap();
        bundle(
            dir.path(),
            json!([
                {"id": "C-1", "verdict": "pass", "surface": "veronika", "runs": [run("pass", &[], json!({}))]},
                {"id": "C-2", "verdict": "fail", "surface": "veronika", "question": "OTP?", "expected_keypoints": ["re:188 :: lapor 188"], "runs": [run("fail", &["lapor 188"], json!({}))]},
                {"id": "C-3", "verdict": "error", "surface": "veronika", "runs": [run("error", &[], json!({"error": "the send button is not on screen"}))]},
                {"id": "C-4", "verdict": "fail", "surface": "goal", "goal": "Buka Roaming", "runs": [run("fail", &["harga"], json!({"steps": [{"action": "tap Roaming", "why": "menu"}]}))]},
                {"id": "C-5", "verdict": "fail", "surface": "search", "runs": [run("fail", &[], json!({"grade": {"keypoints": [], "violations": ["promo palsu"]}}))]},
                {"id": "C-6", "verdict": "flaky", "surface": "search", "runs": [run("fail", &["x"], json!({}))]},
                {"id": "C-7", "verdict": "fail", "surface": "search", "runs": [run("fail", &["y"], json!({}))]}
            ]),
        );
        std::fs::create_dir_all(dir.path().join("logs")).unwrap();
        std::fs::write(dir.path().join("logs").join("C-7.log"), "E AndroidRuntime: FATAL EXCEPTION: main").unwrap();
        let f = findings(&job(dir.path(), Status::Failed, None));
        let cats: Vec<(&str, Category)> = f.iter().map(|f| (f.case_id.as_str(), f.category)).collect();
        assert_eq!(
            cats,
            [
                ("C-2", Category::BusinessRuleDefect),
                ("C-3", Category::AutomationError),
                ("C-4", Category::NavigationDefect),
                ("C-5", Category::BusinessRuleDefect),
                ("C-6", Category::Inconclusive),
                ("C-7", Category::AppCrash),
            ]
        );
        assert!(!f[1].category.is_product(), "a run error is not an app defect");
        assert_eq!(f[0].missed, ["lapor 188"]);
        assert_eq!(f[0].app_version, "9.4.0");
        assert_eq!(f[2].steps, ["tap Roaming (menu)"]);
        assert!(f[0].evidence.contains(&"evidence/E-1.jpg".to_string()) && f[0].evidence.contains(&"video/C-1.mp4".to_string()));
    }

    #[test]
    fn a_job_with_no_graded_case_is_classified_by_how_it_ended() {
        let dir = tempfile::tempdir().unwrap();
        let f = findings(&job(dir.path(), Status::Error, Some("device 33061JEHN09954 disconnected")));
        assert_eq!((f.len(), f[0].category), (1, Category::InfrastructureIssue));
        let f = findings(&job(dir.path(), Status::Blocked, Some("stopped: the agent tried a refused action")));
        assert_eq!(f[0].category, Category::AutomationError);
        assert!(findings(&job(dir.path(), Status::Passed, None)).is_empty());
    }

    #[test]
    fn cases_are_compared_with_their_last_earlier_run() {
        let dirs: Vec<tempfile::TempDir> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
        let at = |i: usize, id: u32, run: &str, cases: Value| {
            bundle(dirs[i].path(), cases);
            let mut j = job(dirs[i].path(), Status::Failed, None);
            j.id = id;
            j.run_id = run.into();
            j
        };
        let case = |id: &str, v: &str| json!({"id": id, "verdict": v, "surface": "search"});
        let old = at(0, 1, "RUN-A", json!([case("C-1", "pass"), case("C-2", "fail"), case("C-3", "fail")]));
        let mid = at(1, 2, "RUN-B", json!([case("C-1", "pass")]));
        let now = at(2, 5, "RUN-C", json!([case("C-1", "fail"), case("C-2", "pass"), case("C-3", "fail"), case("C-9", "fail")]));
        let changes = compare(std::slice::from_ref(&now), &[old, mid, now.clone()]);
        let got: Vec<(&str, &str, Option<&str>)> =
            changes.iter().map(|c| (c.case_id.as_str(), c.change.as_str(), c.before_run.as_deref())).collect();
        assert_eq!(
            got,
            [
                ("C-1", "regressed", Some("RUN-B")),
                ("C-2", "fixed", Some("RUN-A")),
                ("C-3", "still failing", Some("RUN-A")),
                ("C-9", "new", None),
            ]
        );
    }

    #[test]
    fn a_goal_case_is_matched_by_its_goal_not_its_planner_id() {
        let goal = |id: &str| json!({"id": id, "surface": "goal", "goal": "Buka Roaming Jepang", "expected_keypoints": ["re:Rp :: harga"]});
        let dir = tempfile::tempdir().unwrap();
        let j = job(dir.path(), Status::Failed, None);
        assert_eq!(case_key(&j, &goal("AI-001")), case_key(&j, &goal("AI-004")));
        let other = json!({"id": "AI-001", "surface": "goal", "goal": "Buka menu Kuota", "expected_keypoints": ["re:Rp :: harga"]});
        assert_ne!(case_key(&j, &goal("AI-001")), case_key(&j, &other));
    }

    #[test]
    fn an_investigation_keeps_confidence_in_range_and_most_likely_first() {
        let reply = r#"Here is what I found:
{"hypotheses":[
 {"cause":"The keypoint regex expects Rp but the screen shows IDR","confidence":0.4,"supporting":["run.json: response says IDR"]},
 {"cause":"Veronika answered about a different package","confidence":1.7,"supporting":["logs/C-1.log:12"],"against":[]},
 {"cause":"","confidence":0.9},
 {"cause":"Slow network","confidence":-2},
 {"cause":"Extra","confidence":0.1}],
 "limitations":["no screenshot of the final answer"],"nextSteps":["rerun C-1"]}"#;
        let (h, lim, next) = parse_investigation(reply).unwrap();
        let got: Vec<(&str, f64)> = h.iter().map(|h| (h.cause.as_str(), h.confidence)).collect();
        assert_eq!(
            got,
            [
                ("Veronika answered about a different package", 1.0),
                ("The keypoint regex expects Rp but the screen shows IDR", 0.4),
                ("Extra", 0.1)
            ]
        );
        assert_eq!(lim, ["no screenshot of the final answer"]);
        assert_eq!(next, ["rerun C-1"]);
        assert!(parse_investigation("I could not tell.").is_none());
    }

    #[test]
    fn exports_are_redacted() {
        assert_eq!(redact("Hubungi 081234567890 atau +62 812-3456-7890"), "Hubungi <phone> atau <phone>");
        assert_eq!(redact("kode OTP 482913 jangan dibagikan"), "kode OTP <code> jangan dibagikan");
        assert_eq!(redact("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.abc"), "Authorization: Bearer <token>");
        assert_eq!(redact("Paket 25 GB Rp320.000 7 Hari, lapor 188"), "Paket 25 GB Rp320.000 7 Hari, lapor 188", "prices and short numbers stay");

        let dir = tempfile::tempdir().unwrap();
        bundle(dir.path(), json!([{"id": "C-2", "verdict": "fail", "surface": "veronika", "runs": [run("fail", &["lapor 188"], json!({}))]}]));
        let jobs = vec![job(dir.path(), Status::Failed, None)];
        let md = std::fs::read_to_string(export(dir.path(), "RUN-1-01", &jobs, &[]).unwrap()).unwrap();
        assert!(md.contains("Business-rule defect") && md.contains("lapor 188"));
        assert!(!md.contains("081234567890") && !md.contains("482913"), "no phone number or code in the report");
        let json = std::fs::read_to_string(dir.path().join("RUN-1-01").join("reports").join("findings.json")).unwrap();
        assert!(!json.contains("081234567890"));
    }
}
