//! Shared by the native-executable tests (#41, #65, #67): builds the fixture in
//! `native_sidecar_41.rs` with the pinned rustc, and checks whether a process has exited.
#![allow(dead_code)] // each test binary uses its own subset

use std::{fs, path::PathBuf, process::Command, sync::OnceLock};

static EXECUTABLE: OnceLock<PathBuf> = OnceLock::new();

pub fn executable() -> &'static PathBuf {
    EXECUTABLE.get_or_init(|| {
        let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/qa-native-41");
        fs::create_dir_all(&target).unwrap();
        let output = target.join(format!(
            "fixture-{}{}",
            std::process::id(),
            std::env::consts::EXE_SUFFIX
        ));
        let result = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2024", "--crate-name", "queue_native_fixture"])
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/support/native_sidecar_41.rs"),
            )
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "fixture compilation: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        output
    })
}

/// Whether `pid` is gone (or, on Linux, an unreaped zombie, which has exited too).
pub fn process_has_exited(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 probes the recorded fixture PID without sending it a signal.
        if unsafe { libc::kill(i32::try_from(pid).unwrap(), 0) } == -1 {
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
            return true;
        }
        #[cfg(target_os = "linux")]
        {
            // An orphan can be a zombie until the runner's PID 1 reaps it; it has exited.
            if let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) {
                return stat
                    .rsplit_once(") ")
                    .is_some_and(|(_, rest)| rest.starts_with('Z'));
            }
        }
        #[cfg(target_os = "macos")]
        {
            // Likewise until launchd reaps it (#67: a killed orphan is briefly a zombie).
            // SAFETY: a correctly sized output struct for PROC_PIDTBSDINFO.
            unsafe {
                let mut info: libc::proc_bsdinfo = std::mem::zeroed();
                let size = size_of::<libc::proc_bsdinfo>() as i32;
                let got = libc::proc_pidinfo(
                    i32::try_from(pid).unwrap(),
                    libc::PROC_PIDTBSDINFO,
                    0,
                    (&raw mut info).cast(),
                    size,
                );
                if got == size {
                    return info.pbi_status == libc::SZOMB;
                }
            }
        }
        false
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, GetLastError},
            System::Threading::{
                GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        };
        // SAFETY: query only; the owned handle is closed before returning.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                assert_eq!(GetLastError(), ERROR_INVALID_PARAMETER);
                return true;
            }
            let mut exit_code = 0;
            let ok = GetExitCodeProcess(process, &mut exit_code);
            CloseHandle(process);
            assert_ne!(ok, 0, "couldn't query fixture PID {pid}");
            exit_code != 259 // STILL_ACTIVE
        }
    }
}
