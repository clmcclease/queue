//! Process trees (#6, proc.rs) on every OS: the child runs (so a suspended Windows child really
//! was resumed), and `terminate` stops descendants too: the stdout pipe only closes once every
//! process holding it has exited. CI runs this on Windows as well (build.yml).
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use queue_lib::proc;

/// A shell command: `script` for sh, `cmd` for cmd.exe.
fn shell(script: &str, cmd: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", cmd]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", script]);
        c
    }
}

#[test]
fn a_spawned_tree_runs_and_its_output_is_readable() {
    let mut cmd = shell("echo hello-tree", "echo hello-tree");
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut tree = proc::spawn_tree(&mut cmd).unwrap();
    let mut out = String::new();
    tree.child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    assert!(tree.child.wait().unwrap().success());
    assert_eq!(out.trim(), "hello-tree");
}

#[test]
fn terminate_stops_the_child_and_its_descendants_promptly() {
    // The shell starts a long-running grandchild that inherits stdout, then waits on it.
    let mut cmd = shell(
        "sleep 60 & wait",
        "start /b ping -n 60 127.0.0.1 & ping -n 60 127.0.0.1",
    );
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut tree = proc::spawn_tree(&mut cmd).unwrap();
    let mut stdout = tree.child.stdout.take().unwrap();
    let (done, closed) = mpsc::channel();
    std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = stdout.read_to_end(&mut sink);
        let _ = done.send(());
    });
    std::thread::sleep(Duration::from_millis(500));
    let started = Instant::now();
    tree.terminate();
    assert!(
        closed.recv_timeout(Duration::from_secs(10)).is_ok(),
        "stdout stayed open: a descendant survived terminate"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
}
