//! Setup check: on a fresh install nothing says why a run fails, so each dependency is
//! checked in turn and a missing one comes with what to do about it.

use crate::qa::QaConfig;
use serde::Serialize;
use std::path::Path;
use std::process::{Command, Output};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    /// What to do when it failed.
    pub fix: Option<String>,
}

fn check(name: &str, result: Result<String, String>, fix: &str) -> Check {
    match result {
        Ok(detail) => Check { name: name.into(), ok: true, detail, fix: None },
        Err(detail) => Check { name: name.into(), ok: false, detail, fix: Some(fix.into()) },
    }
}

fn run(mut cmd: Command) -> Result<Output, String> {
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    cmd.stdin(std::process::Stdio::null()).output().map_err(|e| format!("could not start it: {e}"))
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default().to_string()
}

/// Run every check. `adb` is the app's adb, if it found one.
pub fn checks(cfg: &QaConfig, adb: Option<&Path>) -> Vec<Check> {
    let mut out = Vec::new();

    out.push(check(
        "QA skill folder",
        cfg.run_py().map(|p| p.to_string_lossy().to_string()).map_err(|e| e.to_string()),
        "In QA Runs, set the QA skill folder to the telkomsel-qa-skills checkout and press Save.",
    ));

    out.push(check(
        "Python and the skill's packages",
        match cfg.run_py() {
            Err(_) => Err("needs the QA skill folder first".into()),
            Ok(run_py) => {
                let mut cmd = Command::new(&cfg.python);
                cmd.arg(&run_py).arg("--help");
                run(cmd).and_then(|o| {
                    if o.status.success() {
                        let mut v = Command::new(&cfg.python);
                        v.arg("--version");
                        Ok(run(v).map(|o| first_line(&o.stdout)).unwrap_or_default())
                    } else {
                        Err(format!("run.py --help failed: {}", first_line(&o.stderr)))
                    }
                })
            }
        },
        "Install Python 3 and the skill's requirements (pip install -r requirements.txt in the skill checkout), or set the Python field to the right interpreter.",
    ));

    out.push(check(
        "adb",
        match adb {
            None => Err("no adb found".into()),
            Some(adb) => {
                let mut cmd = Command::new(adb);
                cmd.arg("version");
                run(cmd).and_then(|o| if o.status.success() { Ok(first_line(&o.stdout)) } else { Err(first_line(&o.stderr)) })
            }
        },
        "Install scrcpy from the top bar, or Android platform-tools.",
    ));

    let agent = cfg.agent();
    out.push(check(
        "QA skill MCP server",
        agent.as_ref().map(|a| a.mcp_server.to_string_lossy().to_string()).map_err(|e| e.to_string()),
        "Use a telkomsel-qa-skills checkout that has mcp/telkomsel-qa-mcp/serve.py.",
    ));

    let cli = {
        let mut cmd = crate::agent::base_command(&cfg.ai_command);
        cmd.arg("--version");
        run(cmd).and_then(|o| if o.status.success() { Ok(first_line(&o.stdout)) } else { Err(first_line(&o.stderr)) })
    };
    let cli_ok = cli.is_ok();
    out.push(check("AI command", cli, "Install Claude Code (the claude CLI), or set the AI command on the AI Agent page."));

    let login = if !cli_ok {
        Err("needs the AI command first".into())
    } else {
        let mut cmd = crate::agent::base_command(&cfg.ai_command);
        cmd.args(["auth", "status"]);
        run(cmd).and_then(|o| {
            let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_default();
            if v["loggedIn"].as_bool() == Some(true) {
                Ok(format!("logged in ({})", v["authMethod"].as_str().unwrap_or("?")))
            } else {
                Err("not logged in".into())
            }
        })
    };
    let login_ok = login.is_ok();
    out.push(check("AI login", login, "In a terminal, run: claude auth login"));

    let model = if !login_ok {
        Err("needs the AI login first".into())
    } else {
        let mut cmd = crate::agent::base_command(&cfg.ai_command);
        cmd.args(["-p", "Reply with the word ok.", "--model", &cfg.ai_model, "--output-format", "json"]);
        run(cmd).and_then(|o| {
            let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_default();
            if v["is_error"].as_bool() == Some(false) {
                Ok(cfg.ai_model.clone())
            } else {
                let why = v["result"].as_str().map(str::to_string).unwrap_or_else(|| first_line(&o.stderr));
                Err(format!("{}: {why}", cfg.ai_model))
            }
        })
    };
    out.push(check("AI model", model, "On the AI Agent page, set a model your account can use (for example claude-sonnet-5-5) and press Save."));

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_install_says_what_is_missing() {
        let cfg = QaConfig { skill_dir: r"C:\nowhere".into(), ai_command: "no-such-ai-cli".into(), ..Default::default() };
        let checks = checks(&cfg, None);
        let failed: Vec<&str> = checks.iter().filter(|c| !c.ok).map(|c| c.name.as_str()).collect();
        assert_eq!(
            failed,
            ["QA skill folder", "Python and the skill's packages", "adb", "QA skill MCP server", "AI command", "AI login", "AI model"]
        );
        assert!(checks.iter().all(|c| c.fix.is_some()), "every failed check says what to do");
    }
}
