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
pub fn draft(cfg: &AgentConfig, objective: &str) -> Result<Draft> {
    let objective = objective.trim();
    if objective.is_empty() {
        return Err(AppError::InvalidArgs("write an objective first".into()));
    }
    let mut cmd = crate::agent::base_command(&cfg.command);
    cmd.arg("-p")
        .arg(draft_prompt(objective))
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
fn load_with_skill(python: &Path, skill_root: &Path, file: &Path) -> Result<Checked> {
    let mut cmd = std::process::Command::new(python);
    cmd.arg("-c").arg(LOADER).arg(skill_root.join("scripts")).arg(file).stdin(Stdio::null());
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
    let checked = load_with_skill(python, skill_root, &file);
    let _ = std::fs::remove_file(&file);
    let checked = checked?;
    rules(&checked).map_err(AppError::InvalidArgs)?;
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
