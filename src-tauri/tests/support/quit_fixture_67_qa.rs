//! Independent QA fixture for #67 / Q7, D2. No external network or media tools.
use std::{
    env, fs,
    process::{self, Command, Stdio},
    thread,
    time::Duration,
};

fn main() {
    let exe = env::current_exe().unwrap();
    let root = exe.parent().unwrap();
    if env::args().nth(1).as_deref() == Some("--descendant") {
        fs::write(root.join("descendant.pid"), process::id().to_string()).unwrap();
        thread::sleep(Duration::from_secs(30));
        return;
    }
    fs::write(root.join("leader.pid"), process::id().to_string()).unwrap();
    let scenario = fs::read_to_string(root.join("scenario")).unwrap();
    match scenario.as_str() {
        "closed-stdout" => {
            // A process may close stdout while continuing to work (stderr stays open).
            #[cfg(unix)]
            unsafe {
                unsafe extern "C" {
                    fn close(fd: i32) -> i32;
                }
                assert_eq!(close(1), 0);
            }
            #[cfg(windows)]
            unsafe {
                #[link(name = "kernel32")]
                unsafe extern "system" {
                    fn GetStdHandle(which: u32) -> *mut std::ffi::c_void;
                    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
                }
                assert_ne!(CloseHandle(GetStdHandle(-11i32 as u32)), 0);
            }
            fs::write(root.join("ready"), b"closed").unwrap();
            thread::sleep(Duration::from_secs(30));
        }
        "orphan-stdout" | "orphan-stderr" => {
            let mut child = Command::new(&exe);
            child.arg("--descendant");
            if scenario == "orphan-stdout" {
                child.stderr(Stdio::null());
            } else {
                child.stdout(Stdio::null());
            }
            let _child = child.spawn().unwrap();
            println!("{{\"title\":\"日本語 🎵 CON\"}}");
            // Exit now: the descendant keeps exactly one of our pipes open.
        }
        "ignore-term" => {
            #[cfg(unix)]
            unsafe {
                unsafe extern "C" {
                    fn signal(sig: i32, handler: usize) -> usize;
                }
                signal(15, 1); // SIGTERM, SIG_IGN; inherited by the descendant.
            }
            let _child = Command::new(&exe).arg("--descendant").spawn().unwrap();
            fs::write(root.join("ready"), b"ignoring").unwrap();
            thread::sleep(Duration::from_secs(30));
        }
        _ => panic!("unknown scenario {scenario}"),
    }
}
