//! Scenario mode: the model writes a test set (YAML in the QA skill's schema) that `run.py`
//! then runs with no model in the loop. The model only reads; the tester reviews and edits
//! the YAML; the app checks it with the skill's own loader plus its own safety rules and
//! saves it in the app's data folder (never in the skill checkout).

use crate::agent::AgentConfig;
use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Surfaces `run.py` runs by itself that only read the app or ask Veronika. `steps` (free
/// taps) and `goal` (needs the agent) are left to their own modes.
pub const SCRIPTED_SURFACES: &[&str] = &["veronika", "search", "search-suggest", "search-empty", "home"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub name: String,
    pub yaml: String,
    pub cost_usd: Option<f64>,
}

/// One case as the skill's loader read it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CaseInfo {
    pub id: String,
    pub surface: String,
    #[serde(default)]
    pub question: String,
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub steps: usize,
    #[serde(default)]
    pub allow_state_change: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checked {
    pub name: String,
    pub cases: Vec<CaseInfo>,
}

/// What the model writes: a scripted read-only test set, or a Veronika bomb test set for the
/// standalone web page or for the phone (see the skill's `veronika-bomb*.yaml`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Scripted,
    BombWeb,
    BombPhone,
}

impl Source {
    pub fn parse(s: Option<&str>) -> Source {
        match s {
            Some("bomb-web") => Source::BombWeb,
            Some("bomb-phone") => Source::BombPhone,
            _ => Source::Scripted,
        }
    }
}

/// The best practices of the skill's bomb runners (docs/research/veronika-bomb-test.md), as rules for the writer.
fn bomb_prompt(objective: &str, source: Source) -> String {
    let (where_, shape) = if source == Source::BombWeb {
        (
            "the standalone Veronika web page (https://veronika.telkomsel.com/veronika), run by scripts in skills/veronika-standalone in parallel Chrome sessions, anonymous",
            "Start from testsets/veronika-bomb-web.yaml. Keep its `standalone:` block with ONLY `url:` (never msisdn, otp or any number: no chat may make Veronika send an SMS) and its `web:` block (sessions, timeout, settle). The page's chat box holds 2000 characters.",
        )
    } else {
        (
            "the MyTelkomsel app on a phone, run by scripts/bomb_ui.py through the real chat field",
            "Start from testsets/veronika-bomb.yaml (phone chats are SENT) or testsets/veronika-bomb-field.yaml (`phone: {no_send: true}`: types and counts characters, sends nothing). Keep the `phone:` block. Never write an `api:` block.",
        )
    };
    format!(
        r#"You write a Veronika BOMB TEST set (load and limit test) for {where_}. The current directory is the mytelkomsel-qa skill. Read scripts/_bomb.py (the schema: kinds rate, length, daily) and the example test set named below, and copy their style.

{shape}

Best practices (follow them, and say in a YAML comment why each test exists):
- Cover what the objective asks, in the three kinds: `rate` (rate chats every per seconds, for duration seconds), `length` (one chat per entry of lengths) and `daily` (total chats as fast as rate/per allow).
- Escalate: begin with a small smoke test (about 5 to 10 chats) so a broken setup shows before the heavy test, then raise the rate in steps (for example 10, 50, 100 per minute) so the first sign of a limit shows between two steps, not only at the top.
- Open workload model: the runners send on a schedule whatever the replies do, so give a real rate and per, never a loop. A chat that finds every session busy counts as dropped.
- Web only: `web.sessions` must cover rate x typical latency / 60 (latency 10 to 50 s was seen), so 100 chats per minute needs 24 sessions or more.
- Length tests: include values below, at and above the limit (100, 200, 500, 1000, 2000, 4000, 8000).
- Put `cooldown: 120` (seconds) so a rate limit hit by one test does not spill into the next.
- Test ids are unique and short (RATE-1, LEN-1, DAY-1 ...). No secrets, tokens or numbers in the file.
- These chats go to PRODUCTION and are real traffic: use the volumes the objective states, never more.

Answer with the YAML only, in one ```yaml fence, starting with name: and a comment line saying what it measures.

Objective: {objective}"#
    )
}

fn draft_prompt(objective: &str) -> String {
    format!(
        r#"You write QA test sets for the MyTelkomsel Android app. Turn the objective below into ONE new test set that the skill's runner (scripts/run.py) runs without a model.

The current directory is the mytelkomsel-qa skill. First read scripts/_testset.py (the schema and its rules) and two or three of testsets/*.yaml (for example veronika-complex-5.yaml and search-playbook.yaml) and copy their style.

Rules:
- surface is one of: {surfaces}. Never steps or goal, never allow_state_change.
- Questions are plain ASCII Indonesian (adb cannot type other characters), as a customer would write them.
- Questions may ask how something works (even how to buy or activate), but no personal data (no phone numbers, names, OTP or PIN).
- Each case has a unique id (like AI-S01), a topic, expected_keypoints (plain words or "re:<regex> :: <label>"), and must_not where a wrong answer is harmful.
- 1 to 8 cases. Prefer cases no existing test set already covers.

Answer with the YAML only, in one ```yaml fence, starting with name: and description:.

Objective: {objective}"#,
        surfaces = SCRIPTED_SURFACES.join(", ")
    )
}

/// The text inside the first ```yaml (or plain ```) fence, else the whole reply.
fn fenced_yaml(text: &str) -> String {
    for open in ["```yaml", "```yml", "```"] {
        if let Some(start) = text.find(open) {
            let body = &text[start + open.len()..];
            let end = body.find("```").unwrap_or(body.len());
            return body[..end].trim().to_string() + "\n";
        }
    }
    text.trim().to_string() + "\n"
}

/// The `name:` line of a YAML test set.
fn yaml_name(yaml: &str) -> Option<String> {
    yaml.lines()
        .find_map(|l| l.strip_prefix("name:"))
        .map(|v| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
        .filter(|v| !v.is_empty())
}

/// Ask the model for a test set. Read-only: it may look at the skill and nothing else.
pub fn draft(cfg: &AgentConfig, objective: &str, source: Source) -> Result<Draft> {
    let objective = objective.trim();
    if objective.is_empty() {
        return Err(AppError::InvalidArgs("write an objective first".into()));
    }
    let prompt = if source == Source::Scripted { draft_prompt(objective) } else { bomb_prompt(objective, source) };
    let mut cmd = crate::agent::base_command(&cfg.command);
    cmd.arg("-p")
        .arg(prompt)
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
        return Err(AppError::Io(format!("the AI could not write a scenario: {text}")));
    }
    let yaml = fenced_yaml(text);
    Ok(Draft { name: yaml_name(&yaml).unwrap_or_else(|| "ai-scenario".into()), yaml, cost_usd: reply["total_cost_usd"].as_f64() })
}

const LOADER: &str = r#"
import json, sys
sys.path.insert(0, sys.argv[1])
import _testset
try:
    t = _testset.load(sys.argv[2])
except Exception as e:
    print(json.dumps({"error": str(e)})); sys.exit(0)
print(json.dumps({"name": t["name"], "cases": [{"id": c["id"], "surface": c["surface"], "question": c["question"],
    "topic": str(c["topic"]), "steps": len(c["steps"]), "allowStateChange": c["allow_state_change"]} for c in t["cases"]]}))
"#;

/// Load a test set file with the skill's own loader (`scripts/_testset.py`).
fn load_with_skill(python: &Path, skill_root: &Path, file: &Path, bomb: bool) -> Result<Checked> {
    let mut cmd = std::process::Command::new(python);
    cmd.arg("-c").arg(if bomb { BOMB_LOADER } else { LOADER }).arg(skill_root.join("scripts")).arg(file).stdin(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd.output().map_err(|e| AppError::NotFound(format!("python \"{}\" ({e})", python.display())))?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        AppError::Io(format!("the skill's loader failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
    })?;
    if let Some(e) = v["error"].as_str() {
        return Err(AppError::InvalidArgs(e.to_string()));
    }
    let cases: Vec<CaseInfo> = serde_json::from_value(v["cases"].clone()).map_err(|e| AppError::Io(e.to_string()))?;
    Ok(Checked { name: v["name"].as_str().unwrap_or_default().to_string(), cases })
}

/// A bomb test set has `tests:` (rate, length, daily); a scripted one has `cases:`.
fn is_bomb_yaml(yaml: &str) -> bool {
    yaml.lines().any(|l| l.starts_with("tests:"))
}

const BOMB_LOADER: &str = r#"
import json, sys
sys.path.insert(0, sys.argv[1])
import _bomb
try:
    t = _bomb.load(sys.argv[2])
except Exception as e:
    print(json.dumps({"error": str(e)})); sys.exit(0)
def what(x):
    n = _bomb.count(x)
    if x["kind"] == "length": return f"{n} chats, lengths {x['lengths']}"
    if x["kind"] == "daily": return f"{n} chats at {x['rate']} per {x['per']} s"
    return f"{n} chats: {x['rate']} per {x['per']} s for {x['duration']} s"
print(json.dumps({"name": t.get("name", ""), "cases": [{"id": x["id"], "surface": x["kind"], "question": what(x), "topic": x["kind"]} for x in t["tests"]]}))
"#;

/// The app's rules for an AI-written bomb set. It may only describe load on Veronika: web sets stay anonymous
/// (no number, no OTP, so no SMS is sent), and credentials never go in the file (the API runner is the developer's).
pub fn bomb_rules(yaml: &str, c: &Checked) -> std::result::Result<(), String> {
    if c.cases.is_empty() {
        return Err("the bomb test has no tests".into());
    }
    let web = yaml.lines().any(|l| l.starts_with("standalone:"));
    let phone = yaml.lines().any(|l| l.starts_with("phone:"));
    if web == phone {
        return Err("a bomb test needs exactly one of a `standalone:` block (web) or a `phone:` block".into());
    }
    if yaml.lines().any(|l| l.starts_with("api:")) {
        return Err("an AI-written bomb test cannot have an `api:` block (it would carry an endpoint and a token)".into());
    }
    let low = yaml.to_ascii_lowercase();
    if web {
        if low.contains("msisdn") || low.lines().any(|l| l.trim_start().starts_with("otp")) {
            return Err("a web bomb test is anonymous: no msisdn and no otp (Veronika would send an SMS)".into());
        }
        if !low.contains("veronika.telkomsel.com") {
            return Err("a web bomb test must target https://veronika.telkomsel.com/veronika".into());
        }
    }
    Ok(())
}

/// The app's own rules on top of the skill's schema: read-only surfaces and ASCII
/// questions. On these surfaces run.py only types into the search box or Veronika's chat
/// and presses send; it taps no result or button, so a question that mentions buying
/// ("cara aktivasi roaming?") changes nothing. Free taps (`steps`) and state changes are
/// what is refused.
pub fn rules(c: &Checked) -> std::result::Result<(), String> {
    if c.cases.is_empty() {
        return Err("the test set has no cases".into());
    }
    for k in &c.cases {
        if !SCRIPTED_SURFACES.contains(&k.surface.as_str()) {
            return Err(format!("{}: surface {} is not allowed here (use one of {})", k.id, k.surface, SCRIPTED_SURFACES.join(", ")));
        }
        if k.steps > 0 || k.allow_state_change {
            return Err(format!("{}: steps and allow_state_change are not allowed in AI-written scenarios", k.id));
        }
        if !k.question.is_ascii() {
            return Err(format!("{}: the question has non-ASCII characters, which adb cannot type", k.id));
        }
    }
    Ok(())
}

pub fn scenarios_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("scenarios")
}

fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "ai-scenario".into() } else { s.chars().take(60).collect() }
}

/// Check YAML text without saving it.
pub fn check(python: &Path, skill_root: &Path, data_dir: &Path, yaml: &str) -> Result<Checked> {
    let tmp = scenarios_dir(data_dir).join(".check");
    std::fs::create_dir_all(&tmp)?;
    let file = tmp.join(format!("check-{}.yaml", std::process::id()));
    std::fs::write(&file, yaml)?;
    let bomb = is_bomb_yaml(yaml);
    let checked = load_with_skill(python, skill_root, &file, bomb);
    let _ = std::fs::remove_file(&file);
    let checked = checked?;
    if bomb {
        bomb_rules(yaml, &checked).map_err(AppError::InvalidArgs)?;
    } else {
        rules(&checked).map_err(AppError::InvalidArgs)?;
    }
    Ok(checked)
}

/// Check, then save as `<data>/scenarios/<name>.yaml` (a new file name if one exists).
pub fn save(python: &Path, skill_root: &Path, data_dir: &Path, yaml: &str) -> Result<PathBuf> {
    let checked = check(python, skill_root, data_dir, yaml)?;
    let dir = scenarios_dir(data_dir);
    let base = slug(&checked.name);
    let mut path = dir.join(format!("{base}.yaml"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base}-{n}.yaml"));
        n += 1;
    }
    std::fs::write(&path, yaml)?;
    Ok(path)
}

/// Saved scenarios, newest name order.
pub fn list(data_dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(scenarios_dir(data_dir))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
                .map(|p| p.to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(id: &str, surface: &str, q: &str) -> CaseInfo {
        CaseInfo { id: id.into(), surface: surface.into(), question: q.into(), topic: String::new(), steps: 0, allow_state_change: false }
    }

    #[test]
    fn yaml_is_taken_from_the_fence_and_named() {
        let reply = "Here it is:\n```yaml\nname: roaming-ai\ndescription: x\ncases: []\n```\nDone.";
        let y = fenced_yaml(reply);
        assert!(y.starts_with("name: roaming-ai"));
        assert!(!y.contains("```") && !y.contains("Done"));
        assert_eq!(yaml_name(&y).as_deref(), Some("roaming-ai"));
        assert_eq!(slug("Roaming Jepang / AI"), "roaming-jepang---ai");
    }

    #[test]
    fn only_read_only_ascii_scenarios_pass() {
        let ok = |cases| rules(&Checked { name: "x".into(), cases });
        assert!(ok(vec![case("A", "veronika", "Paket roaming Jepang apa saja?"), case("B", "search", "kuota 10 GB")]).is_ok());
        assert!(ok(vec![]).is_err());
        assert!(ok(vec![case("A", "steps", "")]).unwrap_err().contains("not allowed"));
        assert!(ok(vec![case("A", "goal", "")]).is_err());
        // Asking about a purchase is fine: the runner only types and sends the question.
        assert!(ok(vec![case("A", "veronika", "Bagaimana cara aktivasi paket roaming?")]).is_ok());
        assert!(ok(vec![case("A", "veronika", "Apa itu kuota “nasional”?")]).unwrap_err().contains("ASCII"));
        let mut taps = case("A", "veronika", "halo");
        taps.steps = 2;
        assert!(ok(vec![taps]).is_err());
    }

    #[test]
    fn bomb_sets_stay_anonymous_and_credential_free() {
        let c = |id: &str| Checked { name: "b".into(), cases: vec![case(id, "rate", "100 chats")] };
        let web = "name: b\nstandalone:\n  url: https://veronika.telkomsel.com/veronika\nweb:\n  sessions: 24\ntests:\n  - id: R\n";
        assert!(is_bomb_yaml(web) && !is_bomb_yaml("name: x\ncases:\n"));
        assert!(bomb_rules(web, &c("R")).is_ok());
        assert!(bomb_rules(&web.replace("url:", "msisdn: \"0813\"\n  url:"), &c("R")).unwrap_err().contains("anonymous"));
        assert!(bomb_rules(&web.replace("sessions", "otp: 1\n  sessions"), &c("R")).unwrap_err().contains("anonymous"));
        assert!(bomb_rules(&web.replace("veronika.telkomsel.com", "example.com"), &c("R")).unwrap_err().contains("veronika.telkomsel.com"));
        assert!(bomb_rules(&format!("{web}api:\n  url: x\n"), &c("R")).unwrap_err().contains("api"));
        assert!(bomb_rules("name: b\nphone:\n  no_send: true\ntests:\n", &c("L")).is_ok());
        assert!(bomb_rules("name: b\ntests:\n", &c("L")).unwrap_err().contains("exactly one"));
        assert!(bomb_rules(web, &Checked { name: "b".into(), cases: vec![] }).is_err());
    }

    #[test]
    fn the_source_picks_the_prompt() {
        assert_eq!(Source::parse(Some("bomb-web")), Source::BombWeb);
        assert_eq!(Source::parse(Some("bomb-phone")), Source::BombPhone);
        assert_eq!(Source::parse(None), Source::Scripted);
        let web = bomb_prompt("100 chats a minute", Source::BombWeb);
        assert!(web.contains("veronika.telkomsel.com") && web.contains("smoke test") && web.contains("100 chats a minute"));
        assert!(bomb_prompt("x", Source::BombPhone).contains("bomb_ui.py"));
    }

    /// Runs the real skill loader: `cargo test --lib scenario -- --ignored` with the skill checkout.
    #[test]
    #[ignore = "needs python, PyYAML and the QA skill checkout"]
    fn the_skill_loader_reads_a_saved_scenario() {
        let root = PathBuf::from(r"D:\programming\telkomsel\sharing-session\telkomsel-qa-skills\skills\mytelkomsel-qa");
        let data = tempfile::tempdir().unwrap();
        let yaml = "name: ai-check\ndescription: t\nsurface: veronika\ncases:\n  - id: AI-S01\n    topic: roaming\n    question: \"Paket roaming Jepang apa saja?\"\n    expected_keypoints: [\"Jepang\"]\n";
        let p = save(Path::new("python"), &root, data.path(), yaml).unwrap();
        assert!(p.ends_with("ai-check.yaml"));
        assert_eq!(list(data.path()), vec![p.to_string_lossy().to_string()]);
        let bad = yaml.replace("veronika", "steps");
        assert!(check(Path::new("python"), &root, data.path(), &bad).is_err());
    }
}
