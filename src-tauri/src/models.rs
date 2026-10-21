//! The local naming model (N7, #31): which files, where they live, and a consented, resumable,
//! checksum-verified download (D-016, D-029). Nothing here runs the model; that's #30.
//!
//! A model is `<dir>/<file>` once its whole sha256 was checked, with `<file>.sha256` beside it
//! (sha256sum format) as the proof, so nothing is hashed at launch. While downloading it's
//! `<file>.part`. The download is chunked ranged GETs, one request per chunk: ureq has no
//! per-read timeout, so a bounded request is the only way a stalled connection can't hang it,
//! and a request that moved bytes counts as progress even if it timed out.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::proc;

/// The two models Queue offers (D-016). The default, and a smaller one for smaller machines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelId {
    // snake_case alone would say `qwen3_4b_instruct2507` (#31 review).
    #[default]
    #[serde(rename = "qwen3_4b_instruct_2507")]
    Qwen3_4bInstruct2507,
    Qwen3_5_2b,
}

/// A model file pinned by repo commit, size and sha256.
pub struct Pinned {
    pub id: ModelId,
    pub name: &'static str,
    pub repo: &'static str,
    pub commit: &'static str,
    pub file: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// Provisional until #33(b) runs the eval on these files. Verified 2026-10-21 by the Hugging
/// Face API, the resolve URL's `x-linked-etag`/`x-linked-size`, and a 16-byte ranged GET.
pub const PINS: [Pinned; 2] = [
    Pinned {
        id: ModelId::Qwen3_4bInstruct2507,
        name: "Qwen3 4B Instruct",
        repo: "bartowski/Qwen_Qwen3-4B-Instruct-2507-GGUF",
        commit: "ae44f08e1392f39c0e474af10c3ff8355c8b6688",
        file: "Qwen_Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        size: 2_497_280_736,
        sha256: "2fde00ce69dd4899c70d020845e2638353015bba0fdf161b3eb965f2bca4464e",
    },
    Pinned {
        id: ModelId::Qwen3_5_2b,
        name: "Qwen3.5 2B",
        repo: "bartowski/Qwen_Qwen3.5-2B-GGUF",
        commit: "7d26695454df6de5fbcce2e58681e62dae06ce43",
        file: "Qwen_Qwen3.5-2B-Q4_K_M.gguf",
        size: 1_396_198_496,
        sha256: "57a1085840f497d764a7fc5d346922dbde961efb54cc792ea81d694fd846a1d8",
    },
];

pub fn pinned(id: ModelId) -> &'static Pinned {
    PINS.iter()
        .find(|p| p.id == id)
        .expect("every id is pinned")
}

/// What to download: tests build their own against a loopback server, so the real pins are
/// never fetched by `cargo test`.
#[derive(Clone, Debug)]
pub struct Pin {
    pub url: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
}

impl Pinned {
    pub fn pin(&self) -> Pin {
        Pin {
            url: format!(
                "https://huggingface.co/{}/resolve/{}/{}",
                self.repo, self.commit, self.file
            ),
            file: self.file.to_owned(),
            size: self.size,
            sha256: self.sha256.to_owned(),
        }
    }
}

/// A model on disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    Missing,
    /// A download stopped part way; it resumes from `received`.
    Partial {
        received: u64,
    },
    /// Downloaded and checked.
    Ready,
    /// A file with the model's name that Queue didn't verify. Never overwritten or deleted
    /// without the user asking.
    Unverified,
}

fn paths(dir: &Path, file: &str) -> (PathBuf, PathBuf, PathBuf) {
    (
        dir.join(file),
        dir.join(format!("{file}.part")),
        dir.join(format!("{file}.sha256")),
    )
}

fn marker_text(pin: &Pin) -> String {
    format!("{}  {}\n", pin.sha256, pin.file)
}

/// Where `pin` stands in `dir`, without reading the model (no hashing at launch).
pub fn state(dir: &Path, pin: &Pin) -> State {
    let (model, part, marker) = paths(dir, &pin.file);
    if let Ok(meta) = fs::metadata(&model) {
        let verified = meta.len() == pin.size
            && fs::read_to_string(&marker).is_ok_and(|m| m == marker_text(pin));
        return if verified {
            State::Ready
        } else {
            State::Unverified
        };
    }
    match fs::metadata(&part) {
        Ok(meta) => State::Partial {
            received: meta.len(),
        },
        Err(_) => State::Missing,
    }
}

/// Removes the model, its proof and any partial download. A missing file isn't an error.
pub fn delete(dir: &Path, pin: &Pin) -> io::Result<()> {
    let (model, part, marker) = paths(dir, &pin.file);
    for path in [model, part, marker] {
        match fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// How a download runs; tests shrink these.
#[derive(Clone, Debug)]
pub struct Options {
    /// Bytes per ranged request.
    pub chunk: u64,
    /// The whole budget for one request, so a stalled read can't hang the download.
    pub budget: Duration,
    /// The pause after an attempt that received nothing.
    pub pause: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            chunk: 16 * 1024 * 1024,
            budget: Duration::from_secs(180),
            pause: Duration::from_secs(2),
        }
    }
}

/// What the download is doing, for the progress events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    Received(u64),
    Verifying,
}

/// How a download ended without an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// Cancelled, or Queue is quitting; the partial file is kept, to resume.
    Stopped,
}

pub const STALLED: &str = "The download stalled. Resume to try again.";
pub const MISMATCH: &str =
    "The downloaded file didn't match its checksum, so it was deleted. Try again.";
pub const NOT_PINNED: &str = "The file at the source isn't the pinned model.";
pub const NO_RANGES: &str =
    "The download source didn't support a partial download, so nothing was saved.";
pub const TAKEN: &str =
    "A file with the model's name is already there, and Queue won't replace it. Delete it first.";

/// Downloads `pin` into `dir`, resuming a partial file, then checks the whole file's sha256
/// before it gets its real name. Stops early (keeping the partial file) when
/// [`proc::should_stop`] says so: a cancel, or Queue quitting.
pub fn download(
    pin: &Pin,
    dir: &Path,
    opts: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Outcome, String> {
    let (model, part, marker) = paths(dir, &pin.file);
    match state(dir, pin) {
        State::Ready => return Ok(Outcome::Done),
        State::Unverified => return Err(TAKEN.to_owned()),
        _ => {}
    }
    fs::create_dir_all(dir).map_err(|e| format!("Couldn't create the models folder: {e}"))?;
    let mut idle = 0;
    loop {
        if proc::should_stop() {
            return Ok(Outcome::Stopped);
        }
        let mut have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if have > pin.size {
            let _ = fs::remove_file(&part);
            have = 0;
        }
        if have == pin.size {
            break;
        }
        progress(Progress::Received(have));
        let end = (have + opts.chunk).min(pin.size) - 1;
        match fetch(pin, &part, have, end, opts, progress) {
            Err(Chunk::Restart) => {
                let _ = fs::remove_file(&part);
                idle += 1;
                if idle >= 2 {
                    return Err(STALLED.to_owned());
                }
            }
            Ok(0) | Err(Chunk::Retry) => {
                idle += 1;
                if idle >= 2 {
                    return Err(STALLED.to_owned());
                }
                proc::sleep_unless_quitting(opts.pause);
            }
            Ok(_) => idle = 0,
            Err(Chunk::Stopped) => return Ok(Outcome::Stopped),
            Err(Chunk::Fatal { message, discard }) => {
                if discard {
                    let _ = fs::remove_file(&part);
                }
                return Err(message);
            }
        }
    }
    progress(Progress::Verifying);
    match sha256_of(&part)? {
        None => return Ok(Outcome::Stopped),
        Some(hex) if hex != pin.sha256 => {
            let _ = fs::remove_file(&part);
            return Err(MISMATCH.to_owned());
        }
        Some(_) => {}
    }
    // Write access: Windows can't flush a read-only handle (#31 review).
    OpenOptions::new()
        .write(true)
        .open(&part)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("Couldn't save the model: {e}"))?;
    if place(&part, &model)? == Outcome::Stopped {
        return Ok(Outcome::Stopped);
    }
    crate::settings::write_atomic(&marker, marker_text(pin).as_bytes())
        .map_err(|e| format!("Couldn't save the model's checksum: {e}"))?;
    Ok(Outcome::Done)
}

/// Gives the checked file its real name without ever replacing one: a hard link refuses an
/// existing name, and on a filesystem without links [`copy_new`] does the same.
fn place(part: &Path, model: &Path) -> Result<Outcome, String> {
    match fs::hard_link(part, model) {
        Ok(()) => {
            let _ = fs::remove_file(part);
            Ok(Outcome::Done)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(TAKEN.to_owned()),
        Err(_) => copy_new(part, model),
    }
}

/// Copies `part` to `model`, which is created only if no file has that name (no check-then-
/// rename race, #31 review), then removes `part`. It copies 1 MiB at a time and stops on a
/// cancel or a quit (#31 review round 3), removing the unfinished copy and keeping `part`, so
/// the next try needs no download. A copy cut off by a crash has no checksum marker, so
/// [`state`] calls it Unverified and never uses it.
pub fn copy_new(part: &Path, model: &Path) -> Result<Outcome, String> {
    let saved = |e: io::Error| format!("Couldn't save the model: {e}");
    let mut target = match OpenOptions::new().write(true).create_new(true).open(model) {
        Ok(target) => target,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(TAKEN.to_owned()),
        Err(e) => return Err(saved(e)),
    };
    let copied = (|| -> io::Result<Outcome> {
        let mut source = File::open(part)?;
        let mut block = vec![0; 1024 * 1024];
        loop {
            if proc::should_stop() {
                return Ok(Outcome::Stopped);
            }
            let n = source.read(&mut block)?;
            if n == 0 {
                target.sync_all()?;
                return Ok(Outcome::Done);
            }
            target.write_all(&block[..n])?;
        }
    })();
    match copied {
        Ok(Outcome::Done) => {
            let _ = fs::remove_file(part);
            Ok(Outcome::Done)
        }
        other => {
            drop(target);
            let _ = fs::remove_file(model);
            other.map_err(saved)
        }
    }
}

/// The file's sha256 in lowercase hex, or `None` if the run was stopped meanwhile.
fn sha256_of(path: &Path) -> Result<Option<String>, String> {
    let mut file = File::open(path).map_err(|e| format!("Couldn't check the download: {e}"))?;
    let mut hasher = Sha256::new();
    let mut block = vec![0; 1024 * 1024];
    loop {
        if proc::should_stop() {
            return Ok(None);
        }
        let n = file
            .read(&mut block)
            .map_err(|e| format!("Couldn't check the download: {e}"))?;
        if n == 0 {
            return Ok(Some(format!("{:x}", hasher.finalize())));
        }
        hasher.update(&block[..n]);
    }
}

/// Why a chunk request didn't finish.
enum Chunk {
    /// Nothing usable came back (no connection, 5xx, 429); try again after a pause.
    Retry,
    /// The source says the partial file can't be continued (416): start over.
    Restart,
    Stopped,
    /// A problem trying again won't fix. `discard` deletes the partial file.
    Fatal {
        message: String,
        discard: bool,
    },
}

fn fatal(message: &str, discard: bool) -> Chunk {
    Chunk::Fatal {
        message: message.to_owned(),
        discard,
    }
}

const MAX_HOPS: usize = 5;

/// What the request thread hands the download: first the answer (how many body bytes to
/// expect, or why not), then the body in pieces.
enum Piece {
    Head(Result<u64, Chunk>),
    Data(Vec<u8>),
}

/// Requests bytes `start..=end` and appends what arrives to `part`. Returns how many bytes were
/// written (some, even if the request then timed out).
///
/// The request and its reads run on their own thread, which only reads the network; this
/// thread writes the file and checks for a cancel or a quit every 50 ms, so a stalled
/// connection never holds either up (#31 QA). An abandoned request thread ends at its next
/// read or at its budget, and can't write anything.
fn fetch(
    pin: &Pin,
    part: &Path,
    start: u64,
    end: u64,
    opts: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<u64, Chunk> {
    let (tx, rx) = std::sync::mpsc::sync_channel(4);
    let (pin_, opts_) = (pin.clone(), opts.clone());
    std::thread::spawn(move || {
        let response = match request(&pin_, start, end, &opts_) {
            Ok(response) => response,
            Err(chunk) => {
                let _ = tx.send(Piece::Head(Err(chunk)));
                return;
            }
        };
        let (response, want) = response;
        if tx.send(Piece::Head(Ok(want))).is_err() {
            return;
        }
        let mut body = response.into_body().into_with_config().limit(want).reader();
        let mut buf = vec![0; 64 * 1024];
        loop {
            match body.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    if tx.send(Piece::Data(buf[..n].to_vec())).is_err() {
                        return;
                    }
                }
            }
        }
    });
    let want = match next(&rx)? {
        Some(Piece::Head(head)) => head?,
        _ => return Err(Chunk::Retry),
    };
    let io_error = |e: io::Error| fatal(&format!("Couldn't save the download: {e}"), false);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(part)
        .map_err(io_error)?;
    if file.seek(io::SeekFrom::End(0)).map_err(io_error)? != start {
        return Err(Chunk::Retry);
    }
    let mut moved = 0;
    while moved < want {
        let Some(Piece::Data(bytes)) = next(&rx)? else {
            break;
        };
        let n = bytes.len().min((want - moved) as usize);
        if start == 0 && moved == 0 && n >= 4 && &bytes[..4] != b"GGUF" {
            return Err(fatal(NOT_PINNED, true));
        }
        file.write_all(&bytes[..n]).map_err(io_error)?;
        moved += n as u64;
        progress(Progress::Received(start + moved));
    }
    Ok(moved)
}

/// The next piece from the request thread, or `None` once it's done; stops early on a cancel
/// or a quit.
fn next(rx: &std::sync::mpsc::Receiver<Piece>) -> Result<Option<Piece>, Chunk> {
    loop {
        if proc::should_stop() {
            return Err(Chunk::Stopped);
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(piece) => return Ok(Some(piece)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
        }
    }
}

/// Sends the ranged request, following redirects by hand, and checks the answer. On success,
/// the response and how many body bytes to read from it.
fn request(
    pin: &Pin,
    start: u64,
    end: u64,
    opts: &Options,
) -> Result<(ureq::http::Response<ureq::Body>, u64), Chunk> {
    let mut url = url::Url::parse(&pin.url).map_err(|_| fatal(NOT_PINNED, false))?;
    for hop in 0..=MAX_HOPS {
        if !allowed(&url) {
            return Err(fatal(
                "The download was sent somewhere that isn't https://.",
                false,
            ));
        }
        let mut config = ureq::Agent::config_builder()
            .timeout_global(Some(opts.budget))
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .max_redirects(0);
        // The system proxy is kept for the real source; a local test server never uses it.
        if is_local(&url) {
            config = config.proxy(None);
        }
        let agent: ureq::Agent = config.build().into();
        let response = match agent
            .get(url.as_str())
            .header("Range", &format!("bytes={start}-{end}"))
            .call()
        {
            Ok(response) => response,
            Err(_) => return Err(Chunk::Retry),
        };
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        match status {
            301 | 302 | 303 | 307 | 308 => {
                // Hugging Face names the file it redirects to: a different one stops here,
                // before anything is downloaded.
                let size_differs =
                    header("x-linked-size").is_some_and(|s| s != pin.size.to_string());
                let etag_differs =
                    header("x-linked-etag").is_some_and(|e| e.trim_matches('"') != pin.sha256);
                if size_differs || etag_differs {
                    return Err(fatal(NOT_PINNED, false));
                }
                if hop == MAX_HOPS {
                    return Err(fatal("The download was redirected too many times.", false));
                }
                let next = header("location")
                    .and_then(|l| url.join(&l).ok())
                    .ok_or_else(|| fatal("The download was redirected nowhere.", false))?;
                url = next;
            }
            206 => {
                let range = header("content-range").and_then(|r| content_range(&r));
                let Some((from, to, total)) = range else {
                    return Err(fatal(NO_RANGES, false));
                };
                if total != pin.size {
                    return Err(fatal(NOT_PINNED, true));
                }
                if from != start || to < from {
                    return Err(fatal(
                        "The download source sent the wrong part of the file.",
                        false,
                    ));
                }
                let want = (to.min(end) - from + 1).min(pin.size - start);
                return Ok((response, want));
            }
            200 => return Err(fatal(NO_RANGES, false)),
            416 => return Err(Chunk::Restart),
            429 | 500..=599 => return Err(Chunk::Retry),
            other => {
                return Err(Chunk::Fatal {
                    message: format!("The download source answered with HTTP {other}."),
                    discard: false,
                });
            }
        }
    }
    Err(fatal("The download was redirected too many times.", false))
}

/// `bytes <from>-<to>/<total>`.
pub fn content_range(value: &str) -> Option<(u64, u64, u64)> {
    let rest = value.trim().strip_prefix("bytes ")?;
    let (span, total) = rest.split_once('/')?;
    let (from, to) = span.split_once('-')?;
    Some((
        from.trim().parse().ok()?,
        to.trim().parse().ok()?,
        total.trim().parse().ok()?,
    ))
}

/// `https://`, or plain HTTP to this computer (tests), the same rule as naming requests.
fn allowed(url: &url::Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => is_local(url),
        _ => false,
    }
}

fn is_local(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// Which model suits this computer (D-016): the small one with under 7.5 GiB of memory (so an
/// 8 GB machine reporting a little less still counts) or without a GPU backend.
pub fn suggested(memory: Option<u64>, gpu: bool) -> Option<ModelId> {
    let small = memory.is_some_and(|m| m < 7 * 1024 * 1024 * 1024 + 512 * 1024 * 1024);
    match (memory, gpu) {
        (_, false) => Some(ModelId::Qwen3_5_2b),
        (Some(_), true) if small => Some(ModelId::Qwen3_5_2b),
        (Some(_), true) => Some(ModelId::Qwen3_4bInstruct2507),
        (None, true) => None,
    }
}

/// Whether llama.cpp gets a GPU backend here. It follows the pinned builds (#30(a), D-029):
/// Metal on macOS arm64, CPU on macOS x64, Windows and Linux. So no runtime probe is needed
/// until a Vulkan build ships (an M5 follow-up).
pub const GPU: bool = cfg!(all(target_os = "macos", target_arch = "aarch64"));

/// This computer's total physical memory, if it can be read.
pub fn total_memory() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let mut bytes: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        // SAFETY: a fixed, NUL-terminated name; `bytes` and `len` describe a valid u64.
        let rc = unsafe {
            libc::sysctlbyname(
                c"hw.memsize".as_ptr(),
                (&mut bytes as *mut u64).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (rc == 0 && bytes > 0).then_some(bytes)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        // SAFETY: a zeroed MEMORYSTATUSEX with its length set, as the API requires.
        unsafe {
            let mut status: MEMORYSTATUSEX = std::mem::zeroed();
            status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
            (GlobalMemoryStatusEx(&mut status) != 0).then_some(status.ullTotalPhys)
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|m| meminfo_total(&m))
    }
}

/// `MemTotal` from `/proc/meminfo`, in bytes.
pub fn meminfo_total(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line
        .trim_start_matches("MemTotal:")
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(kib * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("queue-models-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_pins_are_well_formed_and_point_at_their_commits() {
        assert_eq!(PINS.len(), 2);
        let hex = |s: &str, n: usize| {
            s.len() == n
                && s.chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        };
        for p in &PINS {
            assert!(hex(p.commit, 40), "{}", p.commit);
            assert!(hex(p.sha256, 64), "{}", p.sha256);
            assert!(p.size > 1_000_000_000);
            assert!(p.file.ends_with(".gguf"));
            assert_eq!(
                p.pin().url,
                format!(
                    "https://huggingface.co/{}/resolve/{}/{}",
                    p.repo, p.commit, p.file
                )
            );
        }
        assert_ne!(PINS[0].file, PINS[1].file);
        assert_eq!(pinned(ModelId::default()).id, ModelId::Qwen3_4bInstruct2507);
        assert_eq!(
            serde_json::to_string(&ModelId::Qwen3_5_2b).unwrap(),
            r#""qwen3_5_2b""#
        );
        // The ids the window sends back (src/models.ts) must read as the same models.
        for (id, text) in [
            (ModelId::Qwen3_4bInstruct2507, r#""qwen3_4b_instruct_2507""#),
            (ModelId::Qwen3_5_2b, r#""qwen3_5_2b""#),
        ] {
            assert_eq!(serde_json::to_string(&id).unwrap(), text);
            assert_eq!(serde_json::from_str::<ModelId>(text).unwrap(), id);
        }
    }

    #[test]
    fn state_reads_the_disk_without_hashing() {
        let dir = temp("state");
        let pin = Pin {
            url: "https://x.test/m.gguf".into(),
            file: "m.gguf".into(),
            size: 4,
            sha256: "ab".repeat(32),
        };
        assert_eq!(state(&dir, &pin), State::Missing);
        fs::write(dir.join("m.gguf.part"), b"GG").unwrap();
        assert_eq!(state(&dir, &pin), State::Partial { received: 2 });
        fs::write(dir.join("m.gguf"), b"GGUF").unwrap();
        assert_eq!(state(&dir, &pin), State::Unverified, "no proof yet");
        fs::write(dir.join("m.gguf.sha256"), marker_text(&pin)).unwrap();
        assert_eq!(state(&dir, &pin), State::Ready);
        fs::write(dir.join("m.gguf"), b"GGUF!").unwrap();
        assert_eq!(state(&dir, &pin), State::Unverified, "the wrong size");
        delete(&dir, &pin).unwrap();
        delete(&dir, &pin).unwrap();
        assert_eq!(state(&dir, &pin), State::Missing);
        fs::remove_dir_all(dir).unwrap();
    }

    /// #31 review: without hard links, the model is copied into a file created only if none
    /// has that name, so a file that appears at the last moment is never replaced.
    #[test]
    fn the_copy_fallback_never_replaces_a_file() {
        let dir = temp("copy");
        let (part, model) = (dir.join("m.gguf.part"), dir.join("m.gguf"));
        fs::write(&part, b"GGUF verified").unwrap();
        fs::write(&model, b"someone else's").unwrap();
        assert_eq!(copy_new(&part, &model), Err(TAKEN.to_owned()));
        assert_eq!(fs::read(&model).unwrap(), b"someone else's");
        assert!(part.exists(), "kept, to try again");
        fs::remove_file(&model).unwrap();
        assert_eq!(copy_new(&part, &model), Ok(Outcome::Done));
        assert_eq!(fs::read(&model).unwrap(), b"GGUF verified");
        assert!(!part.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    /// #31 review round 3: the copy stops on a cancel (and so on a quit, the same check),
    /// removes its unfinished file and keeps the partial download, so a retry copies again
    /// without downloading.
    #[test]
    fn the_copy_fallback_stops_on_cancel_and_keeps_the_partial_file() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        let dir = temp("copy-cancel");
        let (part, model) = (dir.join("m.gguf.part"), dir.join("m.gguf"));
        let data = vec![7u8; 5 * 1024 * 1024];
        fs::write(&part, &data).unwrap();
        let cancelled = Arc::new(AtomicBool::new(true));
        let stopped = proc::with_cancel(cancelled, || copy_new(&part, &model));
        assert_eq!(stopped, Ok(Outcome::Stopped));
        assert!(!model.exists(), "the unfinished copy is removed");
        assert_eq!(
            fs::read(&part).unwrap(),
            data,
            "the partial download is kept"
        );
        assert_eq!(
            copy_new(&part, &model),
            Ok(Outcome::Done),
            "and a retry finishes"
        );
        assert_eq!(fs::read(&model).unwrap(), data);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_small_model_is_suggested_for_small_or_gpu_less_machines() {
        let gib = 1024 * 1024 * 1024;
        assert_eq!(
            suggested(Some(16 * gib), true),
            Some(ModelId::Qwen3_4bInstruct2507)
        );
        assert_eq!(
            suggested(Some(8 * gib), true),
            Some(ModelId::Qwen3_4bInstruct2507)
        );
        assert_eq!(suggested(Some(7 * gib), true), Some(ModelId::Qwen3_5_2b));
        assert_eq!(suggested(Some(32 * gib), false), Some(ModelId::Qwen3_5_2b));
        assert_eq!(suggested(None, false), Some(ModelId::Qwen3_5_2b));
        assert_eq!(suggested(None, true), None, "unknown memory: no suggestion");
    }

    #[test]
    fn meminfo_and_content_range_parse() {
        let meminfo = "MemFree:  1000 kB\nMemTotal:       16303932 kB\nSwap: 0 kB\n";
        assert_eq!(meminfo_total(meminfo), Some(16_303_932 * 1024));
        assert_eq!(meminfo_total("nothing"), None);
        assert_eq!(
            content_range("bytes 0-15/2497280736"),
            Some((0, 15, 2_497_280_736))
        );
        assert_eq!(content_range("bytes */100"), None);
        assert_eq!(content_range("items 0-1/2"), None);
        assert!(total_memory().is_some_and(|m| m > 1024 * 1024 * 1024));
    }

    /// The real pins, checked by hand without downloading (D-029): `QUEUE_QA_REAL_PINS=1 cargo
    /// test --lib real_pins -- --ignored`. 16 bytes per model.
    #[test]
    #[ignore]
    fn real_pins_match_the_source() {
        if std::env::var_os("QUEUE_QA_REAL_PINS").is_none() {
            return;
        }
        for p in &PINS {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .http_status_as_error(false)
                .max_redirects(0)
                .build()
                .into();
            let first = agent.get(&p.pin().url).call().unwrap();
            assert_eq!(first.status().as_u16(), 302, "{}", p.file);
            let h = |n: &str| first.headers().get(n).unwrap().to_str().unwrap().to_owned();
            assert_eq!(h("x-linked-size"), p.size.to_string(), "{}", p.file);
            assert_eq!(h("x-linked-etag").trim_matches('"'), p.sha256, "{}", p.file);
            let cdn = h("location");
            let mut part = agent
                .get(&cdn)
                .header("Range", "bytes=0-15")
                .call()
                .unwrap();
            assert_eq!(part.status().as_u16(), 206);
            let head = part.body_mut().read_to_vec().unwrap();
            assert_eq!(&head[..4], b"GGUF", "{}", p.file);
            println!("{}: size, sha256 and GGUF magic match", p.file);
        }
    }
}
