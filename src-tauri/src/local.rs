//! The local model runtime (N6, #30): llama.cpp's `llama-server`, bundled with its libraries
//! in one resource folder, `llama/` (D-029). Upstream's `@loader_path`/`$ORIGIN` rpath and
//! ggml's backend search both look beside the executable, so it isn't an `externalBin`. It
//! lands in `<resource_dir>/llama/` when bundled, and in `target/<profile>/llama/` in dev.
//!
//! Its lifecycle (#30(b), D-029): [`Runtime::endpoint`] starts the server when a row is named
//! and waits for `/health`; Queue's own idle timer ([`Runtime::stop_if_idle`]) stops it after
//! [`IDLE`] without a request; [`Runtime::stop`] stops it at quit and before a model is deleted
//! or switched. It listens on 127.0.0.1 only, with a fresh key per launch (`LLAMA_API_KEY`, so
//! the key is never on a command line other processes can read).
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::ai::{ApiKind, Endpoint};
use crate::proc;

/// The resource folder's name.
pub const DIR: &str = "llama";

/// How long a started server may take to load its model and answer `/health` (D-029).
pub const HEALTH_WAIT: Duration = Duration::from_secs(120);

/// How long the server stays up without a request (D-029): Queue's own timer, not
/// `--sleep-idle-seconds`, so the memory is given back, not just the model unloaded.
pub const IDLE: Duration = Duration::from_secs(120);

/// Why [`Runtime::endpoint_if`] didn't start anything: the model is no longer the one in use.
pub const NOT_IN_USE: &str = "The local model is no longer in use.";

/// How often a start checks `/health`, and whether it should give up.
const POLL: Duration = Duration::from_millis(100);

/// `llama-server` in the `llama/` folder under `resources` (Tauri's resource dir).
pub fn server_in(resources: &Path) -> PathBuf {
    resources
        .join(DIR)
        .join(format!("llama-server{}", std::env::consts::EXE_SUFFIX))
}

/// The server's arguments (D-029): this computer only, a small context, one slot, no web UI,
/// no prompt cache kept in memory, no reasoning, and every layer on the GPU when the build has
/// one (Metal). The key isn't here: it goes in `LLAMA_API_KEY`.
pub fn args(model: &Path, port: u16, gpu: bool) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["-m".into(), model.into()];
    for arg in [
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--cache-ram",
        "0",
        "--no-webui",
        "--no-mmproj",
        "-c",
        "2048",
        "--parallel",
        "1",
        "--reasoning",
        "off",
    ] {
        args.push(arg.into());
    }
    if gpu {
        args.extend(["-ngl".into(), "all".into()]);
    }
    args
}

/// A running server.
struct Server {
    tree: proc::Tree,
    model: PathBuf,
    port: u16,
    key: String,
    used: Instant,
}

impl Server {
    fn endpoint(&self, name: &str) -> Endpoint {
        // llama-server speaks the OpenAI-compatible API.
        Endpoint {
            kind: ApiKind::OpenaiCompatible,
            base_url: format!("http://127.0.0.1:{}/v1", self.port),
            model: name.to_owned(),
            key: Some(self.key.clone()),
        }
    }

    fn alive(&mut self) -> bool {
        matches!(self.tree.child.try_wait(), Ok(None))
    }
}

/// The one local server Queue runs, if any.
pub struct Runtime {
    exe: PathBuf,
    gpu: bool,
    idle: Duration,
    health_wait: Duration,
    server: Mutex<Option<Server>>,
    /// Bumped by [`Runtime::stop`]: a start in progress gives up when it changes, so a delete
    /// or quit never waits for a model to finish loading.
    stops: AtomicU64,
    /// Lifecycle changes under way ([`Runtime::change`]). While any is, no start begins and a
    /// loading one gives up: a job that read the stop counter after a change bumped it, but
    /// took the lock first, would otherwise load the old model while the change waits (#30(b)
    /// review round 2).
    pending: AtomicUsize,
}

/// A lifecycle change under way, from before its stop to after its change ([`Runtime::change`]).
pub struct Change<'a>(&'a Runtime);

impl Drop for Change<'_> {
    fn drop(&mut self) {
        self.0.pending.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Runtime {
    pub fn new(exe: PathBuf, gpu: bool) -> Self {
        Self::with_times(exe, gpu, IDLE, HEALTH_WAIT)
    }

    /// [`Runtime::new`] with its idle time and health wait (tests use short ones).
    pub fn with_times(exe: PathBuf, gpu: bool, idle: Duration, health_wait: Duration) -> Self {
        Self {
            exe,
            gpu,
            idle,
            health_wait,
            server: Mutex::new(None),
            stops: AtomicU64::new(0),
            pending: AtomicUsize::new(0),
        }
    }

    /// Whether this build has the server (a dev build may not have fetched it).
    pub fn exists(&self) -> bool {
        self.exe.is_file()
    }

    fn server(&self) -> MutexGuard<'_, Option<Server>> {
        self.server.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Where to ask `model` (a checked GGUF file), starting the server if it isn't running it.
    /// Blocks while the model loads, up to the health wait. `name` is the model name sent in
    /// each request. Using it resets the idle timer; a request is bounded by `ai::TIMEOUT`,
    /// well inside [`IDLE`], so the server can't be stopped under one.
    pub fn endpoint(&self, model: &Path, name: &str) -> Result<Endpoint, String> {
        self.endpoint_if(model, name, &|| true)
    }

    /// [`Runtime::endpoint`], if `wanted()` still holds once no lifecycle change is running
    /// (#30(b) review): a naming job that read the settings before the user switched or deleted
    /// the model never starts the old one. `wanted` reads the settings and the disk, which every
    /// change updates inside [`Runtime::stop_while`] before this lock is free again.
    pub fn endpoint_if(
        &self,
        model: &Path,
        name: &str,
        wanted: &dyn Fn() -> bool,
    ) -> Result<Endpoint, String> {
        let stops = self.stops.load(Ordering::SeqCst);
        let mut slot = self.server();
        if self.pending.load(Ordering::SeqCst) > 0 || !wanted() {
            return Err(NOT_IN_USE.to_owned());
        }
        if let Some(server) = slot.as_mut()
            && server.model == model
            && server.alive()
        {
            server.used = Instant::now();
            return Ok(server.endpoint(name));
        }
        if let Some(mut old) = slot.take() {
            old.tree.terminate();
        }
        let server = self.start(model, stops)?;
        let endpoint = server.endpoint(name);
        *slot = Some(server);
        Ok(endpoint)
    }

    /// Starts the server and waits until it answers `/health`. A server that exits at once (its
    /// port was taken between choosing and binding it) is tried once more on another port.
    fn start(&self, model: &Path, stops: u64) -> Result<Server, String> {
        let failed = "The local model couldn't start.";
        for attempt in 0..2 {
            let port = free_port().map_err(|_| failed.to_owned())?;
            let key = new_key().map_err(|_| failed.to_owned())?;
            let mut cmd = Command::new(&self.exe);
            cmd.args(args(model, port, self.gpu))
                .env("LLAMA_API_KEY", &key)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let mut tree = proc::spawn_tree(&mut cmd).map_err(|_| failed.to_owned())?;
            match self.wait_healthy(&mut tree, port, stops) {
                Ok(()) => {
                    return Ok(Server {
                        tree,
                        model: model.to_owned(),
                        port,
                        key,
                        used: Instant::now(),
                    });
                }
                Err(Start::Exited) if attempt == 0 => tree.finish(),
                Err(start) => {
                    tree.terminate();
                    return Err(match start {
                        Start::Stopped => "The local model was stopped.".to_owned(),
                        Start::TimedOut => "The local model took too long to load.".to_owned(),
                        Start::Exited => failed.to_owned(),
                    });
                }
            }
        }
        Err(failed.to_owned())
    }

    fn wait_healthy(&self, tree: &mut proc::Tree, port: u16, stops: u64) -> Result<(), Start> {
        let deadline = Instant::now() + self.health_wait;
        let url = format!("http://127.0.0.1:{port}/health");
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(2)))
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(None)
            .build()
            .into();
        loop {
            if proc::stopping()
                || self.stops.load(Ordering::SeqCst) != stops
                || self.pending.load(Ordering::SeqCst) > 0
            {
                return Err(Start::Stopped);
            }
            // The child first: something else answering on this port isn't our server.
            if !matches!(tree.child.try_wait(), Ok(None)) {
                return Err(Start::Exited);
            }
            if agent
                .get(&url)
                .call()
                .is_ok_and(|r| r.status().as_u16() == 200)
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Start::TimedOut);
            }
            std::thread::sleep(POLL);
        }
    }

    /// Stops the server, and any start in progress. At quit.
    pub fn stop(&self) {
        self.stop_while(|| ());
    }

    /// Stops the server and any start in progress, then runs `change` (a model switch or
    /// delete, or leaving Local) before another start can begin: a start waiting on this lock
    /// then sees the change in its `wanted` check. The stop comes first, so the model file is
    /// never deleted while the server has it open. `change` runs under the runtime's lock, so
    /// it must not call back into the runtime.
    pub fn stop_while<T>(&self, change: impl FnOnce() -> T) -> T {
        let _pending = self.change();
        self.stops.fetch_add(1, Ordering::SeqCst);
        let mut slot = self.server();
        if let Some(mut server) = slot.take() {
            server.tree.terminate();
        }
        change()
    }

    /// Marks a lifecycle change as under way until the guard drops: the first step of
    /// [`Runtime::stop_while`], public so tests can hold a change at that point.
    pub fn change(&self) -> Change<'_> {
        self.pending.fetch_add(1, Ordering::SeqCst);
        Change(self)
    }

    /// Stops the server if it hasn't been asked for [`IDLE`], or has exited. Returns whether
    /// one was stopped. Queue's idle timer calls this every few seconds.
    pub fn stop_if_idle(&self) -> bool {
        let mut slot = self.server();
        let Some(server) = slot.as_mut() else {
            return false;
        };
        if server.alive() && server.used.elapsed() < self.idle {
            return false;
        }
        if let Some(mut server) = slot.take() {
            server.tree.terminate();
        }
        true
    }

    /// The running server's process ID, if one is running (tests and the hand check).
    pub fn pid(&self) -> Option<u32> {
        let mut slot = self.server();
        let server = slot.as_mut()?;
        server.alive().then(|| server.tree.child.id())
    }
}

/// Why a start gave up.
enum Start {
    /// [`Runtime::stop`] was called, or Queue is quitting.
    Stopped,
    TimedOut,
    Exited,
}

/// A port nothing listens on now. Another process could take it before the server binds it;
/// the server then exits, and [`Runtime::start`] tries another.
fn free_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind(("127.0.0.1", 0))?
        .local_addr()?
        .port())
}

/// 32 random bytes from the OS, as hex: the server's key for one launch, so another program
/// on this computer can't use the model while Queue runs it.
fn new_key() -> Result<String, getrandom::Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_lives_in_the_llama_resource_folder() {
        let path = server_in(Path::new("res"));
        assert_eq!(path.parent(), Some(Path::new("res/llama")));
        let name = path.file_name().unwrap().to_str().unwrap();
        if cfg!(windows) {
            assert_eq!(name, "llama-server.exe");
        } else {
            assert_eq!(name, "llama-server");
        }
    }

    #[test]
    fn the_arguments_follow_d029() {
        let strs = |gpu| -> Vec<String> {
            args(Path::new("/m dir/model.gguf"), 4321, gpu)
                .into_iter()
                .map(|a| a.into_string().unwrap())
                .collect()
        };
        let cpu = strs(false);
        assert_eq!(
            cpu,
            [
                "-m",
                "/m dir/model.gguf",
                "--host",
                "127.0.0.1",
                "--port",
                "4321",
                "--cache-ram",
                "0",
                "--no-webui",
                "--no-mmproj",
                "-c",
                "2048",
                "--parallel",
                "1",
                "--reasoning",
                "off"
            ]
        );
        let gpu = strs(true);
        assert_eq!(gpu[..cpu.len()], cpu[..]);
        assert_eq!(gpu[cpu.len()..], ["-ngl", "all"]);
        assert!(
            !cpu.iter().chain(&gpu).any(|a| a.contains("key")),
            "the key is never an argument"
        );
    }

    #[test]
    fn keys_are_fresh_and_long() {
        let (a, b) = (new_key().unwrap(), new_key().unwrap());
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn a_missing_server_is_reported_not_started() {
        let runtime = Runtime::new(PathBuf::from("/nowhere/llama-server"), false);
        assert!(!runtime.exists());
        assert!(runtime.endpoint(Path::new("/m.gguf"), "m").is_err());
        assert_eq!(runtime.pid(), None);
        assert!(!runtime.stop_if_idle());
    }
}
