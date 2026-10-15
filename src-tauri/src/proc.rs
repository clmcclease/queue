//! Subprocess trees with bounded output (#6; reused by downloads and cancel, #22).
//!
//! The onefile yt-dlp is a launcher plus the real Python process, and either may start more
//! (ffmpeg, deno). Stopping "yt-dlp" therefore means stopping the whole tree:
//! - Unix: the child leads a new process group; `terminate` sends SIGTERM to the group (so the
//!   onefile launcher can delete its ~70 MB unpack dir), waits up to 3 s, then SIGKILLs it.
//! - Windows: the child is assigned to a Job Object that kills every process in it when the job
//!   is terminated or its handle closes.
use std::io::{self, Read};
use std::process::{Child, Command};
#[cfg(unix)]
use std::time::{Duration, Instant}; // the SIGTERM grace period and spawn retries

/// A spawned child plus whatever is needed to stop everything it started.
pub struct Tree {
    pub child: Child,
    #[cfg(windows)]
    job: windows::Job,
}

/// Spawns `cmd` as the root of a tree that [`Tree::terminate`] can stop completely.
pub fn spawn_tree(cmd: &mut Command) -> io::Result<Tree> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        Ok(Tree {
            child: spawn_retrying_busy(cmd)?,
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_SUSPENDED: u32 = 0x0000_0004;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000; // no console flashing up from the GUI app
        // Start suspended so the child is in the job before it can run (and start anything).
        cmd.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
        let job = windows::Job::new()?;
        let mut child = cmd.spawn()?;
        if let Err(e) = job.assign(&child).and_then(|()| windows::resume(&child)) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        Ok(Tree { child, job })
    }
}

/// Spawns `cmd`, retrying briefly on ETXTBSY ("Text file busy"). An executable that was just
/// written (a test's fake sidecar, or yt-dlp after it updates itself, #21) can still be open for
/// writing in a process another thread is forking at that moment; exec fails until that fork
/// execs and closes it, which takes milliseconds. Cargo retries the same way.
#[cfg(unix)]
fn spawn_retrying_busy(cmd: &mut Command) -> io::Result<Child> {
    let mut attempt = 0;
    loop {
        match cmd.spawn() {
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && attempt < 10 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(5 * attempt));
            }
            result => return result,
        }
    }
}

impl Tree {
    /// Stops the child and everything it started, then reaps the child.
    pub fn terminate(&mut self) {
        #[cfg(unix)]
        {
            let Ok(pgid) = i32::try_from(self.child.id()) else {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return;
            };
            // SAFETY: plain syscalls; a negative pid targets the group created in spawn_tree.
            unsafe { libc::kill(-pgid, libc::SIGTERM) };
            let grace = Instant::now() + Duration::from_secs(3);
            while Instant::now() < grace {
                if matches!(self.child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            // Descendants may outlive the leader; SIGKILL whatever is left in the group.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
        }
        #[cfg(windows)]
        self.job.terminate();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Reads at most `cap` bytes. `Err` if the pipe produced more (the caller stops the tree), so a
/// runaway process can't exhaust memory.
pub fn read_capped(mut pipe: impl Read, cap: usize) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = pipe.read(&mut chunk)?;
        if n == 0 {
            return Ok(buf);
        }
        if buf.len() + n > cap {
            return Err(io::Error::other(format!("more than {cap} bytes of output")));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Reads to the end but keeps only the last `keep` bytes (enough for yt-dlp's final errors).
pub fn read_tail(mut pipe: impl Read, keep: usize) -> io::Result<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = pipe.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 2 * keep {
            buf.drain(..buf.len() - keep);
        }
    }
    if buf.len() > keep {
        buf.drain(..buf.len() - keep);
    }
    Ok(buf)
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    /// A Job Object that kills all its processes when terminated or when the handle closes.
    pub struct Job(HANDLE);

    // SAFETY: a job handle is a kernel object handle; using it from another thread is fine.
    unsafe impl Send for Job {}

    impl Job {
        pub fn new() -> io::Result<Self> {
            // SAFETY: documented Win32 calls with valid arguments; the handle is owned by Job.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let job = Job(handle);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(job)
            }
        }

        /// Called while the child is still suspended, so everything it starts is in the job.
        pub fn assign(&self, child: &Child) -> io::Result<()> {
            // SAFETY: both handles are valid for the duration of the call.
            if unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn terminate(&self) {
            // SAFETY: valid job handle.
            unsafe { TerminateJobObject(self.0, 1) };
        }
    }

    /// Resumes the (single) primary thread of a child created with CREATE_SUSPENDED. std's
    /// `Child` doesn't expose the thread handle, so find it with a Toolhelp thread snapshot.
    pub fn resume(child: &Child) -> io::Result<()> {
        let pid = child.id();
        // SAFETY: documented Win32 calls; every handle opened here is closed here.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = size_of::<THREADENTRY32>() as u32;
            let mut found = Err(io::Error::other("the new process has no thread to resume"));
            let mut more = Thread32First(snapshot, &mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == pid {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    found = if thread.is_null() {
                        Err(io::Error::last_os_error())
                    } else {
                        let resumed = ResumeThread(thread);
                        CloseHandle(thread);
                        if resumed == u32::MAX {
                            Err(io::Error::last_os_error())
                        } else {
                            Ok(())
                        }
                    };
                    break;
                }
                more = Thread32Next(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
            found
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // KILL_ON_JOB_CLOSE: closing the last handle also stops any leftover descendants.
            // SAFETY: we own the handle.
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_capped_returns_everything_up_to_the_cap_and_errors_beyond() {
        assert_eq!(read_capped(&b"hello"[..], 5).unwrap(), b"hello");
        assert!(read_capped(&b"hello!"[..], 5).is_err());
        let big = vec![b'x'; 300_000];
        assert_eq!(read_capped(&big[..], 300_000).unwrap().len(), 300_000);
        assert!(read_capped(&big[..], 299_999).is_err());
    }

    #[test]
    fn read_tail_keeps_only_the_end() {
        let mut data = vec![b'a'; 500_000];
        data.extend_from_slice(b"ERROR: last line\n");
        let tail = read_tail(&data[..], 1024).unwrap();
        assert_eq!(tail.len(), 1024);
        assert!(tail.ends_with(b"ERROR: last line\n"));
        assert_eq!(read_tail(&b"short"[..], 1024).unwrap(), b"short");
    }
}
