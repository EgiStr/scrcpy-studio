//! Worker processes die with the app. On Windows a child outlives its parent, so a QA
//! worker (run.py, or the AI agent tapping a live phone) would keep going after the app
//! closed or crashed, with no lease, budget or guard backstop. Every worker is put in one
//! Job Object that kills all its processes when the app's handle to it closes.

use std::process::Child;

#[cfg(windows)]
mod imp {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub struct KillOnClose(HANDLE);

    // The handle is only used through thread-safe Win32 calls.
    unsafe impl Send for KillOnClose {}
    unsafe impl Sync for KillOnClose {}

    impl KillOnClose {
        pub fn new() -> Option<Self> {
            unsafe {
                let job = CreateJobObjectW(None, None).ok()?;
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if set.is_err() {
                    let _ = CloseHandle(job);
                    return None;
                }
                Some(Self(job))
            }
        }

        pub fn add(&self, child: &Child) -> bool {
            unsafe { AssignProcessToJobObject(self.0, HANDLE(child.as_raw_handle())).is_ok() }
        }
    }

    impl Drop for KillOnClose {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::process::Child;
    pub struct KillOnClose;
    impl KillOnClose {
        pub fn new() -> Option<Self> {
            Some(Self)
        }
        pub fn add(&self, _child: &Child) -> bool {
            true
        }
    }
}

pub use imp::KillOnClose;

/// The app's one worker job. It is never dropped: Windows closes it when the app exits,
/// however it exits.
static WORKERS: std::sync::OnceLock<Option<KillOnClose>> = std::sync::OnceLock::new();

/// Tie `child` (and the processes it starts from now on) to the app's lifetime.
pub fn adopt(child: &Child) -> bool {
    WORKERS.get_or_init(KillOnClose::new).as_ref().is_some_and(|j| j.add(child))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};

    #[test]
    fn closing_the_job_kills_its_workers_and_their_children() {
        // A worker that starts a grandchild, like run.py starting adb or claude starting the MCP server.
        let script = "import subprocess, sys, time; subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)']); time.sleep(60)";
        let job = KillOnClose::new().expect("job object");
        let mut child = Command::new("python").args(["-c", script]).spawn().unwrap();
        assert!(job.add(&child));
        std::thread::sleep(Duration::from_secs(2));
        let pid = child.id();
        let grandchildren = || {
            let out = Command::new("powershell")
                .args(["-NoProfile", "-Command", &format!("(Get-CimInstance Win32_Process -Filter 'ParentProcessId={pid}' | Measure-Object).Count")])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().parse::<u32>().unwrap_or(0)
        };
        assert_eq!(grandchildren(), 1, "the worker started its own child");

        drop(job);
        let start = Instant::now();
        while child.try_wait().unwrap().is_none() {
            assert!(start.elapsed() < Duration::from_secs(5), "the worker survived the job closing");
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(grandchildren(), 0, "the grandchild died too");
    }
}
