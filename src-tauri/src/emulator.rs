//! Android Emulator fleet: SDK discovery, AVD listing, start/stop and state derivation.
//!
//! The emulator is the virtual device runtime; scrcpy only mirrors it. Every adb call here is
//! scoped to an explicit `emulator-<port>` serial.

use crate::adb;
use crate::error::{AppError, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const EXE: &str = if cfg!(windows) { ".exe" } else { "" };
/// Console ports the emulator accepts (even numbers; adb uses port + 1).
const PORTS: std::ops::RangeInclusive<u16> = 5554..=5584;
const STOP_GRACE: Duration = Duration::from_secs(20);

// ---- SDK ----

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sdk {
    pub root: PathBuf,
    pub emulator: PathBuf,
    /// Platform-tools adb, used only when the app's own adb is not installed.
    pub adb: Option<PathBuf>,
}

fn sdk_at(root: &Path) -> Option<Sdk> {
    let emulator = root.join("emulator").join(format!("emulator{EXE}"));
    if !emulator.is_file() {
        return None;
    }
    let adb = root.join("platform-tools").join(format!("adb{EXE}"));
    Some(Sdk {
        root: root.to_path_buf(),
        emulator,
        adb: adb.is_file().then_some(adb),
    })
}

/// Find an Android SDK that has the emulator installed: `ANDROID_HOME`, `ANDROID_SDK_ROOT`,
/// then the Android Studio default location.
pub fn locate_sdk() -> Option<Sdk> {
    let mut roots: Vec<PathBuf> = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .filter_map(|k| std::env::var_os(k))
        .map(PathBuf::from)
        .collect();
    if let Some(local) = dirs::data_local_dir() {
        roots.push(local.join("Android").join("Sdk"));
    }
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Android").join("Sdk"));
        roots.push(home.join("Library").join("Android").join("sdk"));
    }
    roots.iter().find_map(|r| sdk_at(r))
}

// ---- AVDs ----

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Avd {
    pub name: String,
    pub api: Option<String>,
    pub device: Option<String>,
    pub ram: Option<String>,
    pub resolution: Option<String>,
}

pub fn avd_home() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ANDROID_AVD_HOME") {
        return Some(PathBuf::from(p));
    }
    Some(dirs::home_dir()?.join(".android").join("avd"))
}

fn parse_ini(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// `system-images\android-34\google_apis\x86_64\` → `34`.
fn api_from_sysdir(sysdir: &str) -> Option<String> {
    sysdir
        .split(['\\', '/'])
        .find_map(|part| part.strip_prefix("android-"))
        .map(str::to_string)
}

/// Read every `<name>.ini` in the AVD home and the `config.ini` it points to.
pub fn list_avds(home: &Path) -> Vec<Avd> {
    let mut avds: Vec<Avd> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ini"))
        .filter_map(|ini| {
            let name = ini.file_stem()?.to_string_lossy().to_string();
            let top = parse_ini(&std::fs::read_to_string(&ini).ok()?);
            let dir = top
                .get("path")
                .map(PathBuf::from)
                .filter(|p| p.is_dir())
                .unwrap_or_else(|| home.join(format!("{name}.avd")));
            let cfg = parse_ini(&std::fs::read_to_string(dir.join("config.ini")).ok()?);
            let api = cfg
                .get("image.sysdir.1")
                .and_then(|s| api_from_sysdir(s))
                .or_else(|| top.get("target").and_then(|t| api_from_sysdir(t)));
            let resolution = match (cfg.get("hw.lcd.width"), cfg.get("hw.lcd.height")) {
                (Some(w), Some(h)) => Some(format!("{w}x{h}")),
                _ => None,
            };
            Some(Avd {
                name,
                api,
                device: cfg.get("hw.device.name").cloned(),
                ram: cfg.get("hw.ramSize").cloned(),
                resolution,
            })
        })
        .collect();
    avds.sort_by(|a, b| a.name.cmp(&b.name));
    avds
}

// ---- state ----

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Stopped,
    Starting,
    Booting,
    Ready,
    Stopping,
    Offline,
    Failed,
}

/// What we know about the emulator process this app started for an AVD.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Proc {
    /// Not started by this app (it may still be running from elsewhere).
    None,
    Running,
    Exited,
}

/// Process liveness and Android readiness are separate facts: a running process is only
/// READY once adb reports `device` and `sys.boot_completed` is 1.
fn derive_state(proc: Proc, stopping: bool, adb_state: Option<&str>, booted: bool, was_ready: bool) -> State {
    if stopping && proc == Proc::Running {
        return State::Stopping;
    }
    match adb_state {
        Some("device") if booted => State::Ready,
        Some("device") => State::Booting,
        // adb lists a booting emulator as offline for a few seconds.
        Some("offline") if proc == Proc::Running && !was_ready => State::Booting,
        Some(_) => State::Offline,
        None => match proc {
            Proc::Running => State::Starting,
            Proc::Exited => State::Failed,
            Proc::None => State::Stopped,
        },
    }
}

fn serial_port(serial: &str) -> Option<u16> {
    serial.strip_prefix("emulator-")?.parse().ok()
}

fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn free_port(taken: &[u16]) -> Option<u16> {
    PORTS
        .step_by(2)
        .find(|p| !taken.contains(p) && port_is_free(*p) && port_is_free(*p + 1))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmulatorInfo {
    #[serde(flatten)]
    pub avd: Avd,
    pub state: State,
    pub serial: Option<String>,
    /// Started by this app (so we hold its process and log).
    pub managed: bool,
    /// Last line of the emulator log when it failed.
    pub detail: Option<String>,
}

struct Managed {
    port: u16,
    child: Child,
    stopping: bool,
    was_ready: bool,
    log: PathBuf,
}

#[derive(Clone, Default)]
pub struct EmulatorManager {
    items: Arc<Mutex<HashMap<String, Managed>>>,
}

impl EmulatorManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current state of every AVD, merging our process table with what adb reports.
    pub fn snapshot(&self, adb_bin: &Path) -> Result<Vec<EmulatorInfo>> {
        let avds = avd_home().map(|h| list_avds(&h)).unwrap_or_default();
        let running: Vec<adb::Device> = adb::devices(adb_bin)?
            .into_iter()
            .filter(|d| serial_port(&d.serial).is_some())
            .collect();

        // port → AVD name for emulators we started.
        let ours: HashMap<u16, String> = self
            .items
            .lock()
            .unwrap()
            .iter()
            .map(|(name, m)| (m.port, name.clone()))
            .collect();

        // AVD name → (serial, adb state, boot completed)
        let mut live: HashMap<String, (String, String, bool)> = HashMap::new();
        for d in running {
            let port = serial_port(&d.serial).unwrap_or_default();
            let name = match ours.get(&port) {
                Some(n) => Some(n.clone()),
                None => adb::emu_avd_name(adb_bin, &d.serial).ok(),
            };
            let Some(name) = name else { continue };
            let booted = d.state == "device" && adb::boot_completed(adb_bin, &d.serial);
            live.insert(name, (d.serial, d.state, booted));
        }

        let mut items = self.items.lock().unwrap();
        Ok(avds
            .into_iter()
            .map(|avd| {
                let seen = live.get(&avd.name);
                let mut detail = None;
                let (proc, stopping, was_ready, port) = match items.get_mut(&avd.name) {
                    Some(m) => {
                        let proc = match m.child.try_wait() {
                            Ok(None) => Proc::Running,
                            _ => Proc::Exited,
                        };
                        if seen.is_some_and(|s| s.2) {
                            m.was_ready = true;
                        }
                        if proc == Proc::Exited && seen.is_none() {
                            detail = last_line(&m.log);
                        }
                        (proc, m.stopping, m.was_ready, Some(m.port))
                    }
                    None => (Proc::None, false, false, None),
                };
                let state = derive_state(
                    proc,
                    stopping,
                    seen.map(|s| s.1.as_str()),
                    seen.is_some_and(|s| s.2),
                    was_ready,
                );
                let serial = seen
                    .map(|s| s.0.clone())
                    .or_else(|| (proc == Proc::Running).then(|| format!("emulator-{}", port.unwrap_or_default())));
                EmulatorInfo {
                    avd,
                    state,
                    serial,
                    managed: proc != Proc::None,
                    detail,
                }
            })
            .collect())
    }

    /// Boot an AVD on a free console port. Returns its adb serial.
    pub fn start(&self, sdk: &Sdk, adb_bin: &Path, avd: &str, headless: bool, log_dir: &Path) -> Result<String> {
        let current = self.snapshot(adb_bin)?;
        let me = current
            .iter()
            .find(|e| e.avd.name == avd)
            .ok_or_else(|| AppError::NotFound(format!("AVD {avd}")))?;
        if !matches!(me.state, State::Stopped | State::Failed) {
            return Err(AppError::InvalidArgs(format!("{avd} is already running")));
        }

        let mut items = self.items.lock().unwrap();
        items.remove(avd); // a previous run that exited
        let mut taken: Vec<u16> = items.values().map(|m| m.port).collect();
        taken.extend(current.iter().filter_map(|e| serial_port(e.serial.as_deref()?)));
        let port = free_port(&taken)
            .ok_or_else(|| AppError::Session("no free emulator port in 5554-5584".into()))?;

        std::fs::create_dir_all(log_dir)?;
        let log = log_dir.join(format!("{avd}.log"));
        let out = std::fs::File::create(&log)?;
        let mut cmd = Command::new(&sdk.emulator);
        cmd.args(["-avd", avd, "-port", &port.to_string()]);
        if headless {
            cmd.arg("-no-window");
        }
        cmd.stdin(Stdio::null())
            .stdout(out.try_clone()?)
            .stderr(out);
        if let Some(dir) = sdk.emulator.parent() {
            cmd.current_dir(dir);
        }
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        let child = cmd
            .spawn()
            .map_err(|e| AppError::Session(format!("failed to launch emulator: {e}")))?;

        items.insert(
            avd.to_string(),
            Managed { port, child, stopping: false, was_ready: false, log },
        );
        Ok(format!("emulator-{port}"))
    }

    /// Ask the emulator to shut down (`adb emu kill`), then make sure the process is gone.
    pub fn stop(&self, adb_bin: &Path, avd: &str) -> Result<()> {
        let managed_port = {
            let mut items = self.items.lock().unwrap();
            match items.get_mut(avd) {
                Some(m) => {
                    if !matches!(m.child.try_wait(), Ok(None)) {
                        items.remove(avd);
                        return Ok(());
                    }
                    m.stopping = true;
                    Some(m.port)
                }
                None => None,
            }
        };

        let Some(port) = managed_port else {
            // Running, but not started by us: we can only ask it to quit.
            let serial = self
                .snapshot(adb_bin)?
                .into_iter()
                .find(|e| e.avd.name == avd)
                .and_then(|e| e.serial)
                .ok_or_else(|| AppError::InvalidArgs(format!("{avd} is not running")))?;
            return adb::emu_kill(adb_bin, &serial);
        };

        let graceful = adb::emu_kill(adb_bin, &format!("emulator-{port}")).is_ok();
        let items = self.items.clone();
        let avd = avd.to_string();
        std::thread::spawn(move || {
            let deadline = Instant::now() + if graceful { STOP_GRACE } else { Duration::ZERO };
            loop {
                {
                    let mut items = items.lock().unwrap();
                    let Some(m) = items.get_mut(&avd) else { return };
                    if !matches!(m.child.try_wait(), Ok(None)) {
                        items.remove(&avd);
                        return;
                    }
                    if Instant::now() >= deadline {
                        kill_tree(&mut m.child);
                        items.remove(&avd);
                        return;
                    }
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        });
        Ok(())
    }
}

/// `emulator.exe` is a launcher for a qemu child, so kill the whole process tree.
fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn last_line(log: &Path) -> Option<String> {
    let text = String::from_utf8_lossy(&std::fs::read(log).ok()?).to_string();
    text.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_level_from_sysdir() {
        assert_eq!(
            api_from_sysdir("system-images\\android-34\\google_apis_playstore\\x86_64\\").as_deref(),
            Some("34")
        );
        assert_eq!(api_from_sysdir("android-30").as_deref(), Some("30"));
        assert_eq!(api_from_sysdir("system-images/foo"), None);
    }

    #[test]
    fn lists_avds_from_ini_files() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("pixel.avd");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(home.path().join("pixel.ini"), "target=android-33\n").unwrap();
        std::fs::write(
            dir.join("config.ini"),
            "hw.device.name = pixel_7\nhw.ramSize = 2G\nhw.lcd.width = 1080\nhw.lcd.height = 2400\n\
             image.sysdir.1 = system-images\\android-34\\google_apis\\x86_64\\\n",
        )
        .unwrap();
        // An .ini without its .avd directory is not a usable AVD.
        std::fs::write(home.path().join("ghost.ini"), "target=android-30\n").unwrap();

        let avds = list_avds(home.path());
        assert_eq!(
            avds,
            vec![Avd {
                name: "pixel".into(),
                api: Some("34".into()),
                device: Some("pixel_7".into()),
                ram: Some("2G".into()),
                resolution: Some("1080x2400".into()),
            }]
        );
    }

    #[test]
    fn process_running_is_not_ready_until_boot_completes() {
        use Proc::*;
        assert_eq!(derive_state(None, false, Option::None, false, false), State::Stopped);
        assert_eq!(derive_state(Running, false, Option::None, false, false), State::Starting);
        assert_eq!(derive_state(Running, false, Some("offline"), false, false), State::Booting);
        assert_eq!(derive_state(Running, false, Some("device"), false, false), State::Booting);
        assert_eq!(derive_state(Running, false, Some("device"), true, false), State::Ready);
    }

    #[test]
    fn detects_crash_offline_and_stopping() {
        use Proc::*;
        assert_eq!(derive_state(Exited, false, Option::None, false, true), State::Failed);
        assert_eq!(derive_state(Running, false, Some("offline"), false, true), State::Offline);
        assert_eq!(derive_state(Running, true, Some("device"), true, true), State::Stopping);
        // An emulator started outside the app is still discovered through adb.
        assert_eq!(derive_state(None, false, Some("device"), true, false), State::Ready);
        assert_eq!(derive_state(None, false, Some("unauthorized"), false, false), State::Offline);
    }

    #[test]
    fn free_port_skips_taken_ports() {
        let p = free_port(&[5554, 5556]).unwrap();
        assert!(p >= 5558 && p % 2 == 0);
        assert_eq!(serial_port("emulator-5556"), Some(5556));
        assert_eq!(serial_port("192.168.1.2:5555"), Option::None);
    }

    /// Phase 1 acceptance, against the real SDK: boots the first two AVDs headless.
    /// Run with `cargo test --lib fleet_boots -- --ignored --nocapture`.
    #[test]
    #[ignore = "boots two real emulators"]
    fn fleet_boots_two_avds_and_stops_them_independently() {
        let sdk = locate_sdk().expect("Android SDK");
        let adb_bin = sdk.adb.clone().expect("platform-tools adb");
        let logs = tempfile::tempdir().unwrap();
        let mgr = EmulatorManager::new();
        let names: Vec<String> = mgr.snapshot(&adb_bin).unwrap().into_iter().take(2).map(|e| e.avd.name).collect();
        assert_eq!(names.len(), 2, "need two AVDs");

        let state = |name: &str| {
            let all = mgr.snapshot(&adb_bin).unwrap();
            let e = all.into_iter().find(|e| e.avd.name == name).unwrap();
            (e.state, e.serial, e.detail)
        };
        let wait = |name: &str, want: State, secs: u64| {
            let end = Instant::now() + Duration::from_secs(secs);
            loop {
                let (s, serial, detail) = state(name);
                if s == want {
                    println!("{name}: {want:?} {serial:?}");
                    return;
                }
                assert_ne!(s, State::Failed, "{name} failed: {detail:?}");
                assert!(Instant::now() < end, "{name} stuck in {s:?}, wanted {want:?}");
                std::thread::sleep(Duration::from_secs(3));
            }
        };

        let a = mgr.start(&sdk, &adb_bin, &names[0], true, logs.path()).unwrap();
        let b = mgr.start(&sdk, &adb_bin, &names[1], true, logs.path()).unwrap();
        assert_ne!(a, b, "each emulator gets its own serial");
        assert!(mgr.start(&sdk, &adb_bin, &names[0], true, logs.path()).is_err(), "no double start");
        wait(&names[0], State::Ready, 420);
        wait(&names[1], State::Ready, 420);

        mgr.stop(&adb_bin, &names[0]).unwrap();
        wait(&names[0], State::Stopped, 60);
        assert_eq!(state(&names[1]).0, State::Ready, "stopping one must not touch the other");

        mgr.stop(&adb_bin, &names[1]).unwrap();
        wait(&names[1], State::Stopped, 60);
    }
}
