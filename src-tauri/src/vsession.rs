//! Test sessions on hidden virtual displays, so one phone runs two tests at once while its
//! own screen stays free for the user.
//!
//! A session is a scrcpy process with `--new-display --no-window`: the display lives exactly
//! as long as that process, and scrcpy records it to the job's evidence folder. MyTelkomsel
//! (one process, one login) gives at most two tasks per phone, and only in this order (probed
//! on a Pixel 7a, Android 17):
//! 1. the first session cold-starts the app through its launcher on its display;
//! 2. the second starts the first task's root activity directly on its own display, which
//!    makes a second task. A third start moves an existing task instead of making one.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The scrcpy server arguments every session has; a server with all of them and no live
/// session is one a crashed app left behind.
const SIGNATURE: [&str; 3] = ["new_display=", "display_ime_policy=hide", "vd_system_decorations=false"];
const SERVER: &str = "com.genymobile.scrcpy.Server";

pub struct Session {
    pub display: u32,
    serial: String,
    adb: PathBuf,
    child: Child,
    /// Device-side server pids. Ending them ends the session cleanly: the client sees the
    /// stream close, finishes the recording and exits, and the display is removed.
    servers: Vec<String>,
}

fn adb_cmd(adb: &Path, serial: &str) -> Command {
    let mut c = Command::new(adb);
    c.args(["-s", serial]);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

fn shell(adb: &Path, serial: &str, script: &str) -> String {
    adb_cmd(adb, serial)
        .args(["shell", script])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).replace('\r', ""))
        .unwrap_or_default()
}

/// Pids of scrcpy servers on the phone whose arguments contain every one of `needles`.
fn servers(adb: &Path, serial: &str, needles: &[&str]) -> Vec<String> {
    parse_servers(&shell(adb, serial, "ps -A -o PID,ARGS"), needles)
}

fn parse_servers(ps: &str, needles: &[&str]) -> Vec<String> {
    ps.lines()
        .filter(|l| l.contains(SERVER) && !l.contains("sh -c") && needles.iter().all(|n| l.contains(n)))
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect()
}

/// The phone's own screen size and density, so the app lays out as it does for the user.
fn screen(adb: &Path, serial: &str) -> String {
    parse_screen(&shell(adb, serial, "wm size; wm density")).unwrap_or_else(|| "1080x2400/420".into())
}

fn parse_screen(out: &str) -> Option<String> {
    let last = |key: &str| {
        out.lines()
            .filter(|l| l.contains(key))
            .filter_map(|l| l.rsplit(':').next().map(|v| v.trim().to_string()))
            .last()
    };
    Some(format!("{}/{}", last("size:")?, last("density:")?))
}

/// The task section of `dumpsys activity activities` for one display.
fn display_section(dump: &str, display: u32) -> Option<&str> {
    let head = format!("Display #{display} ");
    let start = dump.find(&head)?;
    let rest = &dump[start + head.len()..];
    Some(rest.find("\nDisplay #").map_or(rest, |end| &rest[..end]))
}

fn has_task(dump: &str, display: u32, app: &str) -> bool {
    display_section(dump, display).is_some_and(|s| s.lines().any(|l| l.trim_start().starts_with("* Task{") && l.contains(&format!(":{app} "))))
}

/// The root activity of the app's task on a display (`Hist #0`), as `pkg/class`.
fn root_activity(dump: &str, display: u32, app: &str) -> Option<String> {
    let section = display_section(dump, display)?;
    section.lines().filter(|l| l.contains("Hist") && l.contains("#0:")).find_map(|l| {
        l.split_whitespace().find(|w| w.starts_with(&format!("{app}/"))).map(str::to_string)
    })
}

fn wait_for(deadline: Duration, mut ok: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + deadline;
    while Instant::now() < end {
        if ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(700));
    }
    false
}

impl Session {
    /// Open a session display on `serial` with `app` on it. `sibling` is the display of the
    /// phone's first session; None opens the first one (the app is restarted cold).
    pub fn open(adb: &Path, scrcpy: &Path, serial: &str, app: &str, sibling: Option<u32>, dir: &Path) -> Result<Session, String> {
        if sibling.is_none() {
            // No session is alive on this phone, so any session server still there was left
            // by a crash, with its display and task. End them before the cold start.
            for pid in servers(adb, serial, &SIGNATURE) {
                shell(adb, serial, &format!("kill {pid}"));
            }
            shell(adb, serial, &format!("am force-stop {app}"));
        }
        let before = servers(adb, serial, &SIGNATURE);
        let mut cmd = Command::new(scrcpy);
        cmd.args(["-s", serial])
            .arg(format!("--new-display={}", screen(adb, serial)))
            .args(["--no-vd-system-decorations", "--display-ime-policy=hide", "--no-window", "--no-audio"])
            .arg(format!("--record={}", dir.join("session.mkv").to_string_lossy()));
        if sibling.is_none() {
            // scrcpy starts the app through its control channel, so session 1 keeps control on
            // (nothing sends input through it); the phone's clipboard stays on the phone.
            cmd.arg(format!("--start-app={app}")).arg("--no-clipboard-autosync");
        } else {
            cmd.arg("--no-control");
        }
        cmd.env("ADB", adb).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(d) = scrcpy.parent() {
            cmd.current_dir(d);
        }
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        let mut child = cmd.spawn().map_err(|e| format!("could not start scrcpy: {e}"))?;
        if !crate::workers::adopt(&child) {
            crate::emulator::kill_tree(&mut child);
            return Err("could not tie the session to the app, so it could outlive it".into());
        }

        // scrcpy prints "New display: WxH/dpi (id=N)"; keep its output in the job's folder.
        let (tx, rx) = mpsc::channel::<String>();
        let mut log = std::fs::File::create(dir.join("session.log")).ok();
        for out in [child.stdout.take().map(|o| Box::new(o) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|e| Box::new(e) as Box<dyn std::io::Read + Send>)].into_iter().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(|l| l.ok()) {
                    let _ = tx.send(line);
                }
            });
        }
        drop(tx);
        let mut seen = Vec::new();
        let display = loop {
            match rx.recv_timeout(Duration::from_secs(25)) {
                Ok(line) => {
                    if let Some(f) = log.as_mut() {
                        let _ = writeln!(f, "{line}");
                    }
                    if let Some(id) = new_display_id(&line) {
                        break Some(id);
                    }
                    seen.push(line);
                }
                Err(_) => break None,
            }
        };
        // Keep logging the rest in the background.
        std::thread::spawn(move || {
            for line in rx {
                if let Some(f) = log.as_mut() {
                    let _ = writeln!(f, "{line}");
                }
            }
        });
        let Some(display) = display else {
            crate::emulator::kill_tree(&mut child);
            let why = seen.iter().rev().find(|l| l.contains("ERROR")).cloned().unwrap_or_else(|| "no display after 25 s".into());
            return Err(format!("scrcpy could not open a display: {why}"));
        };
        let after = servers(adb, serial, &SIGNATURE);
        let mine = after.into_iter().filter(|p| !before.contains(p)).collect();
        let session = Session { display, serial: serial.into(), adb: adb.into(), child, servers: mine };

        let dump = || shell(adb, serial, "dumpsys activity activities");
        let opened = match sibling {
            None => wait_for(Duration::from_secs(30), || has_task(&dump(), display, app)),
            Some(first) => match root_activity(&dump(), first, app) {
                Some(root) => {
                    shell(adb, serial, &format!("am start --display {display} -n {root}"));
                    wait_for(Duration::from_secs(20), || {
                        let d = dump();
                        has_task(&d, display, app) && has_task(&d, first, app)
                    })
                }
                None => false,
            },
        };
        if !opened {
            let why = match sibling {
                None => format!("{app} did not open on display {display}"),
                Some(first) => format!("{app} could not open a second task on display {display} (first session on display {first})"),
            };
            session.close();
            return Err(why);
        }
        Ok(session)
    }

    /// End the session: stop its server on the phone, give the client time to finish the
    /// recording, then make sure it is gone.
    pub fn close(mut self) {
        // Without known pids only the client is stopped; the next cold start on this phone
        // ends any server it left behind.
        if !self.servers.is_empty() {
            shell(&self.adb, &self.serial, &format!("kill {}", self.servers.join(" ")));
        }
        let ended = wait_for(Duration::from_secs(6), || matches!(self.child.try_wait(), Ok(Some(_))));
        if !ended {
            crate::emulator::kill_tree(&mut self.child);
        }
    }
}

fn new_display_id(line: &str) -> Option<u32> {
    let rest = &line[line.find("New display:")?..];
    let id = &rest[rest.find("(id=")? + 4..];
    id[..id.find(')')?].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = "ACTIVITY MANAGER ACTIVITIES (dumpsys activity activities)
Display #12 (activities from top to bottom):
  * Task{97fed5e #1505 type=standard A=10315:com.telkomsel.telkomselcm U=0 visible=true}
    * Hist  #1: ActivityRecord{1 u0 com.telkomsel.telkomselcm/com.x.Search t1505}
    * Hist  #0: ActivityRecord{2 u0 com.telkomsel.telkomselcm/com.x.InitMainActivity t1505}
Display #11 (activities from top to bottom):
  * Task{98944e7 #1504 type=standard A=10315:com.telkomsel.telkomselcm U=0 visible=true}
    * Hist  #0: ActivityRecord{3 u0 com.telkomsel.telkomselcm/com.x.InitMainActivity t1504}
Display #0 (activities from top to bottom):
  * Task{1 #1474 type=standard A=10200:com.whatsapp U=0 visible=true}
";

    #[test]
    fn reads_the_display_id_scrcpy_prints() {
        assert_eq!(new_display_id("[server] INFO: New display: 1080x2400/420 (id=12)"), Some(12));
        assert_eq!(new_display_id("INFO: Recording started"), None);
    }

    #[test]
    fn finds_tasks_and_their_root_per_display() {
        let app = "com.telkomsel.telkomselcm";
        assert!(has_task(DUMP, 12, app) && has_task(DUMP, 11, app));
        assert!(!has_task(DUMP, 0, app) && !has_task(DUMP, 13, app));
        assert_eq!(root_activity(DUMP, 12, app).as_deref(), Some("com.telkomsel.telkomselcm/com.x.InitMainActivity"));
    }

    #[test]
    fn picks_session_servers_by_their_arguments() {
        let ps = " 30813 sh -c CLASSPATH=x app_process / com.genymobile.scrcpy.Server 5.0.1 new_display=1080x2400/420 display_ime_policy=hide vd_system_decorations=false
 30815 app_process / com.genymobile.scrcpy.Server 5.0.1 new_display=1080x2400/420 display_ime_policy=hide vd_system_decorations=false
 30826 app_process / com.genymobile.scrcpy.CleanUp -1
 31000 app_process / com.genymobile.scrcpy.Server 5.0.1 video_bit_rate=8000000";
        assert_eq!(parse_servers(ps, &SIGNATURE), vec!["30815"]);
        assert_eq!(parse_servers(ps, &[]).len(), 2, "a mirror's server is not a session's");
    }

    #[test]
    fn uses_the_phone_screen_size() {
        assert_eq!(parse_screen("Physical size: 1080x2400\nPhysical density: 420").as_deref(), Some("1080x2400/420"));
        assert_eq!(parse_screen("Physical size: 1080x2400\nOverride size: 720x1600\nPhysical density: 420\nOverride density: 320").as_deref(), Some("720x1600/320"));
    }
}
