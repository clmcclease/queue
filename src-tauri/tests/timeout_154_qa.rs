//! #154 / NFR (D-047): the exit probe must accept an unreaped exit without accepting a
//! live descendant. The real version runner must still kill a stubborn pipe holder.
#![cfg(unix)]

#[path = "support/native.rs"]
mod native;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct SleepingChild(Child);

impl SleepingChild {
    fn start() -> Self {
        Self(
            Command::new("sleep")
                .arg("120")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }
}

impl Drop for SleepingChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn exit_probe_does_not_accept_a_running_or_stopped_process() {
    let child = SleepingChild::start();
    let pid = child.0.id();
    assert!(!native::process_has_exited(pid));
    // A stopped descendant is still alive and must not make the timeout test pass.
    // SAFETY: this PID belongs to our unreaped child; waitpid writes to a valid status.
    unsafe {
        assert_eq!(libc::kill(pid as i32, libc::SIGSTOP), 0);
        let mut status = 0;
        assert_eq!(
            libc::waitpid(pid as i32, &mut status, libc::WUNTRACED),
            pid as i32
        );
        assert!(libc::WIFSTOPPED(status));
    }
    assert!(!native::process_has_exited(pid));
}

#[test]
fn exit_probe_accepts_a_zombie_before_its_parent_reaps_it() {
    let mut child = SleepingChild::start();
    let pid = child.0.id();
    child.0.kill().unwrap();
    // WNOWAIT deliberately leaves an exited child unreaped. This deterministically exercises
    // the state that the old signal-0-only assertion mistook for a live descendant.
    // SAFETY: our own child PID, a correctly sized siginfo_t, and documented waitid flags.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        assert_eq!(
            libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT),
            0
        );
        assert_eq!(
            libc::kill(pid as i32, 0),
            0,
            "the zombie still answers signal 0"
        );
    }
    // Allow the same observation window as #154, without reaping the child ourselves.
    let deadline = Instant::now() + Duration::from_secs(2);
    while !native::process_has_exited(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    #[cfg(target_os = "macos")]
    // SAFETY: our own child PID and a correctly sized output struct for PROC_PIDTBSDINFO.
    let diagnostic = unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = size_of::<libc::proc_bsdinfo>() as i32;
        let got = libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast(),
            size,
        );
        format!(
            "proc_pidinfo: got={got}, size={size}, status={}, error={}",
            info.pbi_status,
            std::io::Error::last_os_error()
        )
    };
    #[cfg(not(target_os = "macos"))]
    let diagnostic = "unreaped exited child";
    assert!(native::process_has_exited(pid), "{diagnostic}");
    assert!(!child.0.wait().unwrap().success());
    assert!(native::process_has_exited(pid));
}

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        // Also clean up on an assertion failure, including a real descendant leak.
        if let Ok(text) = fs::read_to_string(self.0.join("child.pid"))
            && let Ok(pid) = text.parse::<u32>()
            && !native::process_has_exited(pid)
        {
            // SAFETY: the PID was recorded by this fixture and has not exited.
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn version_timeout_kills_a_term_ignoring_descendant_even_after_valid_output() {
    let root = Fixture(std::env::temp_dir().join(format!(
        "queue-154-qa-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    )));
    fs::create_dir(&root.0).unwrap();
    let path = root.0.join("雪 ; $(echo unsafe) yt-dlp");
    fs::write(
        &path,
        r#"#!/bin/sh
trap '' TERM
sleep 120 &
printf '%s' "$!" > "$(dirname "$0")/child.pid"
printf '2026.10.26\n'
exit 0
"#,
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();

    let started = Instant::now();
    let result = queue_lib::ytdlp_update::run_version(&path);
    let elapsed = started.elapsed(); // Preserve the runner's bound independently of the probe.
    let pid: u32 = fs::read_to_string(root.0.join("child.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !native::process_has_exited(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let exited = native::process_has_exited(pid);
    assert!(
        result.is_err(),
        "valid output must not bypass a pipe timeout"
    );
    assert!(elapsed >= Duration::from_secs(59), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(68), "{elapsed:?}");
    assert!(
        exited,
        "a descendant ignoring SIGTERM must still exit on timeout"
    );
}
