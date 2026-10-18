//! Test-only executable fixture for #41 / cross-platform NFR. Compiled with the pinned rustc.
use std::{
    env, fs,
    io::{self, Write},
    path::PathBuf,
    process::{self, Command},
    thread,
    time::Duration,
};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let root = env::current_exe().unwrap().parent().unwrap().to_owned();
    if args.first().is_some_and(|s| s == "--hold-pipe") {
        fs::write(root.join("descendant.pid"), process::id().to_string()).unwrap();
        // A heartbeat for 30 s: the test checks it stops, proving this descendant was killed.
        for beat in 0..600u32 {
            fs::write(root.join("heartbeat"), beat.to_string()).unwrap();
            thread::sleep(Duration::from_millis(50));
        }
        return;
    }
    let scenario = fs::read_to_string(root.join("scenario.txt")).unwrap();
    let attempt_path = root.join("attempt.txt");
    let attempt = fs::read_to_string(&attempt_path)
        .unwrap_or_default()
        .parse::<u32>()
        .unwrap_or(0)
        + 1;
    fs::write(&attempt_path, attempt.to_string()).unwrap();
    fs::write(root.join(format!("args-{attempt}.txt")), args.join("\n")).unwrap();
    // #65 QA: mimic startup work that exceeds the old 300/500 ms deadlines.
    if scenario.starts_with("slow-") {
        thread::sleep(Duration::from_millis(900));
    }
    if scenario == "timeout" || scenario == "slow-timeout" {
        fs::write(root.join("leader.pid"), process::id().to_string()).unwrap();
        fs::write(root.join("started"), b"ready").unwrap();
        thread::sleep(Duration::from_secs(30));
        return;
    }
    if scenario == "detached" {
        // #67: a descendant that lets go of both pipes, so the run ends while it's alive.
        let _child = Command::new(env::current_exe().unwrap())
            .arg("--hold-pipe")
            .stdout(process::Stdio::null())
            .stderr(process::Stdio::null())
            .spawn()
            .unwrap();
        while !root.join("descendant.pid").exists() {
            thread::sleep(Duration::from_millis(10));
        }
        println!("{{\"title\":\"日本語\"}}");
        return;
    }
    if scenario == "orphan" || scenario == "slow-orphan" {
        // Inherit the parent's pipes, testing the same process group / Windows Job Object.
        let _child = Command::new(env::current_exe().unwrap())
            .arg("--hold-pipe")
            .spawn()
            .unwrap();
        println!("{{\"title\":\"日本語\"}}");
        return;
    }
    let p = args
        .iter()
        .position(|a| a == "-P")
        .expect("download must pass -P");
    let work = PathBuf::from(&args[p + 1]);
    if scenario == "hang" {
        // #67: a download that never finishes, with a descendant holding its pipes (like
        // ffmpeg under yt-dlp), until Queue quits.
        fs::write(work.join("Title.webm.part"), b"partial").unwrap();
        fs::write(root.join("leader.pid"), process::id().to_string()).unwrap();
        let _child = Command::new(env::current_exe().unwrap())
            .arg("--hold-pipe")
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_secs(30));
        return;
    }
    if scenario == "retry" && attempt == 1 {
        fs::write(work.join("kept.part"), b"partial").unwrap();
        eprintln!("ERROR: HTTP Error 403: Forbidden");
        process::exit(1);
    }
    if scenario == "retry" {
        assert_eq!(fs::read(work.join("kept.part")).unwrap(), b"partial");
    }
    if scenario == "malformed" {
        // Fill both pipes past their bounds, then valid CRLF output must still be read.
        println!("{}", "x".repeat(70000));
        eprintln!("{}", "y".repeat(70000));
        println!("QUEUE-PROGRESS not-json");
        println!("QUEUE-PROGRESS {{\"status\":\"finished\",\"downloaded\":\"bad\"}}");
        print!(
            "QUEUE-PROGRESS {{\"status\":\"downloading\",\"downloaded\":3,\"total\":4,\"speed\":5,\"eta\":1}}\r\n"
        );
        println!("QUEUE-POST {{\"status\":\"started\",\"postprocessor\":\"ExtractAudio\"}}");
        io::stdout().flush().unwrap();
        eprintln!("ERROR: [generic] fixture: extraction failed 日本語");
        process::exit(1);
    }
    let name = fs::read_to_string(root.join("name.txt")).unwrap();
    let path = work.join(name);
    fs::write(&path, b"fixture media").unwrap();
    // Real yt-dlp writes a UTF-8 path independently of the console codepage.
    fs::write(
        work.join("queue-result.txt"),
        format!("{}\n", path.display()),
    )
    .unwrap();
}
