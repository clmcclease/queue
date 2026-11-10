//! yt-dlp updates (#21, D-036, D-042). Queue downloads newer yt-dlp releases into its own data
//! folder, never over the bundled copy in the signed app, at most once a day, and only uses a
//! copy whose signed `SHA2-256SUMS` it verified with the committed key (rPGP, approved by the
//! owner) and that reported the expected version when run. Off means the bundled copy and no
//! requests.
//!
//! Layout: `<app local data>/yt-dlp/<version>/` holds the binary, `SHA2-256SUMS`,
//! `SHA2-256SUMS.sig` and `verified.json`, written last; `<app local data>/yt-dlp/state.json`
//! holds the last check, its result and a rejected version. Only the newest usable copy is
//! kept, and the folder is pruned at launch only.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The pinned sidecars (`scripts/sidecars.json`): the bundled version, the asset names and the
/// key's fingerprint come from here, so there's one pin (D-042).
const MANIFEST: &str = include_str!("../../scripts/sidecars.json");
/// yt-dlp's release key, committed with its source (#44).
const KEY: &str = include_str!("../../scripts/keys/yt-dlp.asc");

/// Where the newest release's sums redirect from, and where a release's files are.
pub const LATEST: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/SHA2-256SUMS";
pub const DOWNLOAD: &str = "https://github.com/yt-dlp/yt-dlp/releases/download/";

const SUMS: &str = "SHA2-256SUMS";
/// Why an attempt stopped: updates were switched off meanwhile.
const OFF: &str = "updates were turned off";
const SIG: &str = "SHA2-256SUMS.sig";
const MARKER: &str = "verified.json";
const STATE: &str = "state.json";
/// The sums and the signature are small (the sums list about 20 files; the signature is 566
/// bytes); the binary is about 35 MB.
const SMALL_CAP: u64 = 64 * 1024;
const ASSET_CAP: u64 = 200 * 1024 * 1024;
const MAX_HOPS: usize = 5;
/// The whole asset download, start to end; a slower connection fails rather than resumes.
const ASSET_BUDGET: Duration = Duration::from_secs(15 * 60);
const SMALL_BUDGET: Duration = Duration::from_secs(60);
/// Running the new binary with `--version`.
const VERSION_LIMIT: Duration = Duration::from_secs(60);
/// At most one attempt a day, failed ones included.
pub const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// The "Keep yt-dlp up to date" setting (settings.json `ytdlpAutoUpdate`): on by default (PRD
/// §6.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AutoUpdate(pub bool);

impl Default for AutoUpdate {
    fn default() -> Self {
        Self(true)
    }
}

/// A yt-dlp release version, `YYYY.MM.DD` or `YYYY.MM.DD.N` (N ≥ 1, no leading zero), digits
/// only. URLs and folder names are always built from this, never from a raw tag (D-042).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    y: u16,
    m: u8,
    d: u8,
    n: Option<u32>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.split('.').collect();
        if !(3..=4).contains(&parts.len()) {
            return None;
        }
        let digits = |p: &str, len: std::ops::RangeInclusive<usize>| {
            len.contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit())
        };
        if !digits(parts[0], 4..=4) || !digits(parts[1], 2..=2) || !digits(parts[2], 2..=2) {
            return None;
        }
        let (y, m, d) = (
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
            parts[2].parse().ok()?,
        );
        if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            return None;
        }
        let n = match parts.get(3) {
            None => None,
            Some(p) if digits(p, 1..=9) && !p.starts_with('0') => Some(p.parse().ok()?),
            Some(_) => return None,
        };
        Some(Self { y, m, d, n })
    }

    fn key(&self) -> (u16, u8, u8, u32) {
        (self.y, self.m, self.d, self.n.unwrap_or(0))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key().cmp(&other.key())
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}.{:02}.{:02}", self.y, self.m, self.d)?;
        if let Some(n) = self.n {
            write!(f, ".{n}")?;
        }
        Ok(())
    }
}

/// What a signature is checked against: an armored public key and the fingerprint its primary
/// key must have. Production uses [`pinned`]; tests pass a throwaway key.
#[derive(Clone, Copy, Debug)]
pub struct Trust<'a> {
    pub key: &'a str,
    pub fingerprint: &'a str,
}

/// The pins from `scripts/sidecars.json` that the updater uses.
#[derive(Debug)]
pub struct Pins {
    pub bundled: Version,
    pub fingerprint: String,
    /// Asset name by target triple.
    pub assets: Vec<(String, String)>,
}

/// The embedded manifest's pins.
pub fn pins() -> &'static Pins {
    static PINS: OnceLock<Pins> = OnceLock::new();
    PINS.get_or_init(|| parse_pins(MANIFEST).expect("scripts/sidecars.json's yt-dlp pins"))
}

fn parse_pins(manifest: &str) -> Option<Pins> {
    let v: serde_json::Value = serde_json::from_str(manifest).ok()?;
    let y = v.get("ytdlp")?;
    let assets = y
        .get("assets")?
        .as_object()?
        .iter()
        .filter_map(|(target, a)| Some((target.clone(), a.get("asset")?.as_str()?.to_owned())))
        .collect();
    Some(Pins {
        bundled: Version::parse(y.get("version")?.as_str()?)?,
        fingerprint: y.get("signing")?.get("fingerprint")?.as_str()?.to_owned(),
        assets,
    })
}

/// The committed key and its pinned fingerprint.
pub fn pinned() -> Trust<'static> {
    Trust {
        key: KEY,
        fingerprint: &pins().fingerprint,
    }
}

/// This build's target triple, for the four `sidecars.json` targets; other targets have no
/// updater (D-042).
pub const fn target() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else if cfg!(all(windows, target_arch = "x86_64", target_env = "msvc")) {
        Some("x86_64-pc-windows-msvc")
    } else if cfg!(all(
        target_os = "linux",
        target_arch = "x86_64",
        target_env = "gnu"
    )) {
        Some("x86_64-unknown-linux-gnu")
    } else {
        None
    }
}

/// The release asset this build runs, if it has an updater.
pub fn asset() -> Option<&'static str> {
    let target = target()?;
    pins()
        .assets
        .iter()
        .find(|(t, _)| t == target)
        .map(|(_, a)| a.as_str())
}

/// Checks the detached signature over `sums` against `trust` (D-042's policy, since rPGP
/// applies none): a v4+ binary-document signature (type 0x00) with SHA-256, SHA-384 or SHA-512,
/// made by the primary key whose fingerprint is the pin. No time-based checks: the key has no
/// expiry and Queue can't learn of a revocation.
pub fn verify_sums(trust: Trust, sums: &[u8], sig: &[u8]) -> Result<(), String> {
    use pgp::composed::{Deserializable, DetachedSignature, SignedPublicKey};
    use pgp::crypto::hash::HashAlgorithm;
    use pgp::packet::SignatureType;
    use pgp::types::KeyDetails;

    const BAD: &str = "the signature on yt-dlp's checksums didn't verify";
    let (key, _) = SignedPublicKey::from_string(trust.key)
        .map_err(|_| "the release key couldn't be read".to_owned())?;
    let fingerprint = format!("{:X}", key.fingerprint());
    if !fingerprint.eq_ignore_ascii_case(trust.fingerprint) {
        return Err("the release key isn't the pinned one".into());
    }
    let sig = DetachedSignature::from_bytes(sig).map_err(|_| BAD.to_owned())?;
    let s = &sig.signature;
    if s.typ() != Some(SignatureType::Binary) {
        return Err(BAD.into());
    }
    if !matches!(
        s.hash_alg(),
        Some(HashAlgorithm::Sha256 | HashAlgorithm::Sha384 | HashAlgorithm::Sha512)
    ) {
        return Err(BAD.into());
    }
    // An issuer fingerprint, if the signature names one, must be the pin.
    if s.issuer_fingerprint()
        .iter()
        .any(|f| !format!("{f:X}").eq_ignore_ascii_case(trust.fingerprint))
    {
        return Err(BAD.into());
    }
    sig.verify(&key.primary_key, sums)
        .map_err(|_| BAD.to_owned())
}

/// The sha256 that `sums` pairs with `asset`: exactly one line `<64 hex>  <asset>` (or with
/// `*` for binary mode). Hashes are lower-cased; anything else on such a line is refused.
pub fn asset_sha256(sums: &[u8], asset: &str) -> Result<[u8; 32], String> {
    let text =
        std::str::from_utf8(sums).map_err(|_| "yt-dlp's checksums aren't text".to_owned())?;
    let mut found = None;
    for line in text.lines() {
        let Some((hash, name)) = line.split_once(' ') else {
            continue;
        };
        let name = name.trim_start_matches(' ').trim_start_matches('*');
        if name != asset {
            continue;
        }
        if found.is_some() {
            return Err(format!("yt-dlp's checksums list {asset} twice"));
        }
        found = Some(
            hex32(hash).ok_or_else(|| format!("yt-dlp's checksum for {asset} isn't a sha256"))?,
        );
    }
    found.ok_or_else(|| format!("yt-dlp's checksums don't list {asset}"))
}

fn hex32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in text.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(out)
}

/// The tag named by the newest release's redirect, which must be exactly
/// `<download><tag>/SHA2-256SUMS` (D-042). The tag is a hint; `--version` proves it later.
pub fn tag_from_location(location: &str, download: &str) -> Result<Version, String> {
    let tag = location
        .strip_prefix(download)
        .and_then(|rest| rest.strip_suffix(&format!("/{SUMS}")))
        .filter(|tag| !tag.contains('/'));
    tag.and_then(Version::parse)
        .ok_or_else(|| "GitHub's answer for the newest yt-dlp release wasn't understood".into())
}

/// What `--version` printed, as a version: one line, nothing else.
pub fn reported_version(output: &[u8]) -> Option<Version> {
    std::str::from_utf8(output)
        .ok()
        .map(str::trim)
        .and_then(Version::parse)
}

/// A file's size, modified time and permissions when it was verified: a copy whose file later
/// differs (lost its execute bit, too: #21 QA) is no longer used this session (D-042).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    /// The Unix mode bits; on Windows, whether it's read-only.
    permissions: u32,
}

impl Stamp {
    pub fn of(path: &Path) -> Option<Self> {
        let m = fs::metadata(path).ok()?;
        #[cfg(unix)]
        let permissions = {
            use std::os::unix::fs::PermissionsExt;
            m.permissions().mode()
        };
        #[cfg(not(unix))]
        let permissions = u32::from(m.permissions().readonly());
        m.is_file().then(|| Self {
            len: m.len(),
            modified: m.modified().ok(),
            permissions,
        })
    }

    /// Whether the file can be run: on Unix, its owner may execute it.
    pub fn runnable(&self) -> bool {
        cfg!(not(unix)) || self.permissions & 0o100 != 0
    }
}

/// A verified copy in use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adopted {
    pub version: Version,
    pub path: PathBuf,
    pub stamp: Stamp,
}

/// Hashes `path`, giving the stamp taken before reading, if the file stays the same.
fn hash_file(path: &Path) -> Result<([u8; 32], Stamp), String> {
    let stamp = Stamp::of(path).ok_or("the yt-dlp file is missing")?;
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
    if Stamp::of(path) != Some(stamp) {
        return Err("the yt-dlp file changed while it was checked".into());
    }
    Ok((hasher.finalize().into(), stamp))
}

/// The whole chain for a stored copy: signature, the asset's line, the file's hash.
pub fn verify_chain(
    trust: Trust,
    sums: &[u8],
    sig: &[u8],
    asset: &str,
    file: &Path,
) -> Result<Stamp, String> {
    verify_sums(trust, sums, sig)?;
    let want = asset_sha256(sums, asset)?;
    let (got, stamp) = hash_file(file)?;
    if got != want {
        return Err("the yt-dlp file doesn't match its signed checksum".into());
    }
    Ok(stamp)
}

/// `state.json`: written only by the updater.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    /// When the last attempt started, in seconds since 1970.
    pub last_check: Option<u64>,
    /// What it came to, in plain words.
    pub last_result: Option<String>,
    /// A release whose binary didn't report its own version: skipped until a newer tag.
    pub rejected: Option<String>,
}

/// `verified.json`: the commit point of an adoption. Metadata only; launch re-verifies.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Marker {
    version: String,
    asset: String,
}

/// Seconds since 1970.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Whether a check is due: never checked, a day since, or a last check in the future (a
/// changed clock).
pub fn due(state: &State, now: u64) -> bool {
    match state.last_check {
        None => true,
        Some(last) => last > now || now - last >= DAY.as_secs(),
    }
}

/// Runs a binary with `--version` and returns its standard output.
pub type RunVersion = dyn Fn(&Path) -> Result<Vec<u8>, String> + Send + Sync;

/// The updater for one data folder.
pub struct Updater {
    pub root: PathBuf,
    pub bundled: Version,
    pub asset: String,
    pub key: String,
    pub fingerprint: String,
    /// [`LATEST`] and [`DOWNLOAD`]; a loopback server in tests.
    pub latest: String,
    pub download: String,
    pub run_version: Box<RunVersion>,
    /// The largest asset accepted ([`ASSET_CAP`]; smaller in tests).
    pub asset_cap: u64,
}

impl Updater {
    /// The real updater for `root`, if this build has one.
    pub fn real(root: PathBuf) -> Option<Self> {
        Some(Self {
            root,
            bundled: pins().bundled,
            asset: asset()?.to_owned(),
            key: KEY.to_owned(),
            fingerprint: pins().fingerprint.clone(),
            latest: LATEST.into(),
            download: DOWNLOAD.into(),
            run_version: Box::new(run_version),
            asset_cap: ASSET_CAP,
        })
    }

    fn trust(&self) -> Trust<'_> {
        Trust {
            key: &self.key,
            fingerprint: &self.fingerprint,
        }
    }

    pub fn state(&self) -> State {
        fs::read(self.root.join(STATE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_state(&self, state: &State) -> io::Result<()> {
        fs::create_dir_all(&self.root)?;
        let json = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
        crate::settings::write_atomic(&self.root.join(STATE), &json)
    }

    /// At launch: the newest stored copy that verifies again from its stored sums and signature
    /// and is strictly newer than the bundled version. Everything else in the folder is deleted
    /// (older or unusable versions, folders without a marker, `.part` files), at launch only.
    pub fn launch(&self) -> Option<Adopted> {
        let entries = fs::read_dir(&self.root).ok()?;
        let mut versions = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == STATE {
                continue;
            }
            match Version::parse(&name) {
                Some(v) if path.is_dir() && name == v.to_string() => versions.push((v, path)),
                _ => remove(&path),
            }
        }
        versions.sort_by_key(|(v, _)| std::cmp::Reverse(*v));
        let mut chosen = None;
        for (version, dir) in versions {
            if chosen.is_none()
                && version > self.bundled
                && let Ok(adopted) = self.verify_stored(version, &dir)
            {
                chosen = Some(adopted);
                continue;
            }
            remove(&dir);
        }
        chosen
    }

    fn verify_stored(&self, version: Version, dir: &Path) -> Result<Adopted, String> {
        let marker: Marker = fs::read(dir.join(MARKER))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or("no marker")?;
        if marker.version != version.to_string() || marker.asset != self.asset {
            return Err("the marker doesn't match".into());
        }
        let read = |name: &str| fs::read(dir.join(name)).map_err(|e| e.to_string());
        let path = dir.join(&self.asset);
        let stamp = verify_chain(self.trust(), &read(SUMS)?, &read(SIG)?, &self.asset, &path)?;
        if !stamp.runnable() {
            return Err("the stored yt-dlp can't be run".into());
        }
        // Only the files of a verified copy stay.
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if ![SUMS, SIG, MARKER, self.asset.as_str()]
                    .iter()
                    .any(|n| name.as_os_str() == *n)
                {
                    remove(&entry.path());
                }
            }
        }
        Ok(Adopted {
            version,
            path,
            stamp,
        })
    }

    /// One update attempt. The attempt time is saved before the first request, and nothing is
    /// requested if that fails. Returns the newly adopted copy, if any; every outcome is saved
    /// as the last result.
    pub fn check(&self, current: Option<Version>) -> Result<Option<Adopted>, String> {
        self.check_while(current, &|| true)
    }

    /// [`Updater::check`] that stops as soon as `go()` turns false (updates switched off): it's
    /// asked before every request, during the download, before running the new binary and
    /// before adopting it, so off means no further requests (#21 review). A stopped attempt
    /// leaves nothing behind.
    pub fn check_while(
        &self,
        current: Option<Version>,
        go: &dyn Fn() -> bool,
    ) -> Result<Option<Adopted>, String> {
        if !go() {
            return Err(OFF.into());
        }
        let mut state = self.state();
        state.last_check = Some(now_secs());
        state.last_result = Some("Checking…".into());
        self.save_state(&state)
            .map_err(|e| format!("Couldn't save the update state: {e}"))?;
        let outcome = self.attempt(current, &mut state, go);
        state.last_result = Some(match &outcome {
            Ok(Some(a)) => format!("Updated to {}.", a.version),
            Ok(None) => "Up to date.".into(),
            Err(e) => format!("Couldn't update: {e}."),
        });
        let _ = self.save_state(&state);
        outcome
    }

    fn attempt(
        &self,
        current: Option<Version>,
        state: &mut State,
        go: &dyn Fn() -> bool,
    ) -> Result<Option<Adopted>, String> {
        let stop = || if go() { Ok(()) } else { Err(OFF.to_owned()) };
        stop()?;
        let location = redirect_of(&self.latest, go)?;
        let tag = tag_from_location(&location, &self.download)?;
        let floor = current.map_or(self.bundled, |c| c.max(self.bundled));
        if tag <= floor {
            return Ok(None);
        }
        if state.rejected.as_deref() == Some(tag.to_string().as_str()) {
            return Ok(None);
        }
        if state.rejected.is_some() {
            state.rejected = None; // a newer tag clears it
        }
        let base = format!("{}{tag}/", self.download);
        stop()?;
        let sums = get_bytes(&format!("{base}{SUMS}"), SMALL_CAP, go)?;
        stop()?;
        let sig = get_bytes(&format!("{base}{SIG}"), SMALL_CAP, go)?;
        verify_sums(self.trust(), &sums, &sig)?;
        let want = asset_sha256(&sums, &self.asset)?;
        let dir = self.root.join(tag.to_string());
        stop()?;
        remove(&dir);
        let placed = self.place(&base, &dir, &sums, &sig, want, go);
        let path = match placed {
            Ok(path) => path,
            Err(e) => {
                remove(&dir);
                return Err(e);
            }
        };
        if let Err(e) = stop() {
            remove(&dir);
            return Err(e);
        }
        // The signed sums carry no version: the binary must say it's the tag's (D-042). Only a
        // run that answered with something else rejects the release; one that couldn't run
        // (a timeout, a spawn error, quitting) just uses up the day (#21 review).
        let output = match (self.run_version)(&path) {
            Ok(output) => output,
            Err(e) => {
                remove(&dir);
                return Err(format!("the new yt-dlp couldn't be run ({e})"));
            }
        };
        if reported_version(&output) != Some(tag) {
            remove(&dir);
            state.rejected = Some(tag.to_string());
            return Err(format!(
                "the yt-dlp {tag} download didn't report version {tag}"
            ));
        }
        if let Err(e) = stop() {
            remove(&dir);
            return Err(e);
        }
        let marker = Marker {
            version: tag.to_string(),
            asset: self.asset.clone(),
        };
        let json = serde_json::to_vec_pretty(&marker).map_err(|e| e.to_string())?;
        let stamp = Stamp::of(&path).ok_or("the yt-dlp file vanished")?;
        if let Err(e) = crate::settings::write_atomic(&dir.join(MARKER), &json) {
            remove(&dir);
            return Err(e.to_string());
        }
        Ok(Some(Adopted {
            version: tag,
            path,
            stamp,
        }))
    }

    /// Downloads the asset into `dir` as a `.part`, checks its hash, then puts the sums, the
    /// signature and the binary in place.
    fn place(
        &self,
        base: &str,
        dir: &Path,
        sums: &[u8],
        sig: &[u8],
        want: [u8; 32],
        go: &dyn Fn() -> bool,
    ) -> Result<PathBuf, String> {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let part = dir.join(format!("{}.part", self.asset));
        let mut file = fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut hashing = Hashing {
            file: &mut file,
            hasher: Sha256::new(),
            go,
        };
        get_into(
            &format!("{base}{}", self.asset),
            self.asset_cap,
            ASSET_BUDGET,
            go,
            &mut hashing,
        )?;
        let got: [u8; 32] = hashing.hasher.finalize().into();
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        if got != want {
            return Err("the yt-dlp download doesn't match its signed checksum".into());
        }
        fs::write(dir.join(SUMS), sums).map_err(|e| e.to_string())?;
        fs::write(dir.join(SIG), sig).map_err(|e| e.to_string())?;
        let path = dir.join(&self.asset);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&part, fs::Permissions::from_mode(0o755))
                .map_err(|e| e.to_string())?;
        }
        fs::rename(&part, &path).map_err(|e| e.to_string())?;
        Ok(path)
    }
}

struct Hashing<'a> {
    file: &'a mut fs::File,
    hasher: Sha256,
    /// Turned false: the download stops.
    go: &'a dyn Fn() -> bool,
}

impl Write for Hashing<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !(self.go)() {
            return Err(io::Error::other(OFF));
        }
        let n = self.file.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn remove(path: &Path) {
    if path.is_dir() {
        let _ = fs::remove_dir_all(path);
    } else {
        let _ = fs::remove_file(path);
    }
}

/// `https://`, or plain HTTP to this machine (tests' loopback servers).
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

fn agent(url: &url::Url, budget: Duration) -> ureq::Agent {
    let mut config = ureq::Agent::config_builder()
        .timeout_global(Some(budget))
        .timeout_connect(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .max_redirects(0);
    // The system proxy is kept for GitHub; a local test server never uses it.
    if is_local(url) {
        config = config.proxy(None);
    }
    config.build().into()
}

fn parse_url(text: &str) -> Result<url::Url, String> {
    let url =
        url::Url::parse(text).map_err(|_| "a yt-dlp update address wasn't valid".to_owned())?;
    if !allowed(&url) {
        return Err("a yt-dlp update was sent somewhere that isn't https://".into());
    }
    Ok(url)
}

/// The one place the updater sends a request (D-042, "Off, precisely"): `go()` is read
/// immediately before each send, redirect hops included, and a request whose read saw "off" is
/// never sent. A request already on its way isn't interrupted; its answer is discarded by the
/// caller's next read.
fn send(
    url: &url::Url,
    budget: Duration,
    go: &dyn Fn() -> bool,
    unreachable: &str,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    if !go() {
        return Err(OFF.into());
    }
    agent(url, budget)
        .get(url.as_str())
        .call()
        .map_err(|_| unreachable.to_owned())
}

/// The Location of the one redirect `url` answers with (redirects aren't followed).
fn redirect_of(url: &str, go: &dyn Fn() -> bool) -> Result<String, String> {
    let url = parse_url(url)?;
    let response = send(&url, SMALL_BUDGET, go, "GitHub couldn't be reached")?;
    if !go() {
        return Err(OFF.into());
    }
    match response.status().as_u16() {
        301 | 302 | 303 | 307 | 308 => response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .ok_or_else(|| {
                "GitHub's answer for the newest yt-dlp release wasn't understood".into()
            }),
        other => Err(format!("GitHub answered with HTTP {other}")),
    }
}

/// GETs `url` into `sink`, following up to [`MAX_HOPS`] redirects by hand (each to `https://`,
/// or loopback HTTP), and failing past `cap` bytes or `budget`.
fn get_into(
    url: &str,
    cap: u64,
    budget: Duration,
    go: &dyn Fn() -> bool,
    sink: &mut dyn Write,
) -> Result<u64, String> {
    let mut url = parse_url(url)?;
    let started = std::time::Instant::now();
    for _ in 0..=MAX_HOPS {
        let left = budget.saturating_sub(started.elapsed());
        if left.is_zero() {
            return Err("the yt-dlp download took too long".into());
        }
        let response = send(&url, left, go, "the yt-dlp download failed")?;
        match response.status().as_u16() {
            301 | 302 | 303 | 307 | 308 => {
                let next = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|l| url.join(l).ok())
                    .ok_or("the yt-dlp download was redirected nowhere")?;
                if !allowed(&next) {
                    return Err("a yt-dlp update was sent somewhere that isn't https://".into());
                }
                url = next;
            }
            200 => {
                let mut body = response
                    .into_body()
                    .into_with_config()
                    .limit(cap + 1)
                    .reader();
                let mut limited = (&mut body).take(cap + 1);
                let n = io::copy(&mut limited, sink).map_err(|e| {
                    // Switched off mid-download says so; anything else is a failed download.
                    if e.to_string() == OFF {
                        OFF.to_owned()
                    } else {
                        "the yt-dlp download failed".to_owned()
                    }
                })?;
                if n > cap {
                    return Err("the yt-dlp download was larger than expected".into());
                }
                return Ok(n);
            }
            other => return Err(format!("GitHub answered with HTTP {other}")),
        }
    }
    Err("the yt-dlp download was redirected too many times".into())
}

fn get_bytes(url: &str, cap: u64, go: &dyn Fn() -> bool) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    get_into(url, cap, SMALL_BUDGET, go, &mut out)?;
    Ok(out)
}

/// Runs `path --ignore-config --no-update --version` within [`VERSION_LIMIT`], output capped.
pub fn run_version(path: &Path) -> Result<Vec<u8>, String> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(path);
    cmd.args(["--ignore-config", "--no-update", "--version"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut tree = crate::proc::spawn_tree(&mut cmd).map_err(|e| e.to_string())?;
    let stdout = tree.child.stdout.take().expect("piped");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(crate::proc::read_capped(stdout, 4096));
    });
    let deadline = std::time::Instant::now() + VERSION_LIMIT;
    let out = match crate::proc::recv_until(&rx, deadline) {
        crate::proc::Waited::Got(Ok(out)) => out,
        _ => {
            tree.terminate();
            return Err("yt-dlp didn't report its version".into());
        }
    };
    loop {
        match tree.child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => {
                tree.finish();
                return Err("yt-dlp didn't report its version".into());
            }
            Ok(None) if std::time::Instant::now() >= deadline || crate::proc::should_stop() => {
                tree.terminate();
                return Err("yt-dlp didn't report its version".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
    tree.finish();
    Ok(out)
}

/// One wake of the updater thread (D-042). The resolver's on/off is the one source of truth,
/// set by the setting command, so a stale reading can't switch updates back on (#21 review).
/// Off: nothing is requested or touched. On: the stored copy is verified once (pruning the
/// folder), then a check runs if a day has passed since the last attempt, stopping as soon as
/// updates are switched off. `launched` remembers the launch verification across wakes.
pub fn tick(up: &Updater, launched: &mut bool, resolver: &Resolver) {
    if !resolver.enabled() {
        return;
    }
    if !*launched {
        *launched = true;
        if let Some(adopted) = up.launch() {
            resolver.adopt(adopted);
        }
    }
    if due(&up.state(), now_secs())
        && let Ok(Some(adopted)) = up.check_while(resolver.adopted(), &|| resolver.enabled())
    {
        resolver.adopt(adopted);
    }
}

/// Which yt-dlp runs use (D-042): the adopted copy while the setting is on and its file is
/// unchanged since it was verified; otherwise the bundled one. The app has one ([`global`]).
#[derive(Default)]
pub struct Resolver(Mutex<Active>);

#[derive(Default)]
struct Active {
    enabled: bool,
    adopted: Option<Adopted>,
    /// The adopted file changed or vanished: the bundled copy for the rest of the session.
    broken: bool,
}

/// The app's resolver.
pub fn global() -> &'static Resolver {
    static GLOBAL: OnceLock<Resolver> = OnceLock::new();
    GLOBAL.get_or_init(Resolver::default)
}

impl Resolver {
    /// Turns the updated copy on or off for runs from now on.
    pub fn set_enabled(&self, enabled: bool) {
        if let Ok(mut a) = self.0.lock() {
            a.enabled = enabled;
        }
    }

    /// Uses `adopted` for runs from now on.
    pub fn adopt(&self, adopted: Adopted) {
        if let Ok(mut a) = self.0.lock() {
            a.adopted = Some(adopted);
            a.broken = false;
        }
    }

    /// Whether updates are on.
    pub fn enabled(&self) -> bool {
        self.0.lock().is_ok_and(|a| a.enabled)
    }

    /// The copy in use now, if it's an update.
    pub fn in_use(&self) -> Option<Version> {
        let a = self.0.lock().ok()?;
        (a.enabled && !a.broken).then_some(a.adopted.as_ref()?.version)
    }

    /// The adopted copy, whether or not it's in use (the next check compares against it).
    pub fn adopted(&self) -> Option<Version> {
        self.0.lock().ok()?.adopted.as_ref().map(|a| a.version)
    }

    /// Whether a copy was adopted (verified) this session.
    pub fn has_adopted(&self) -> bool {
        self.0.lock().is_ok_and(|a| a.adopted.is_some())
    }

    /// The yt-dlp to run: the adopted copy if usable, else `bundled`.
    pub fn resolve(&self, bundled: PathBuf) -> PathBuf {
        let Ok(mut a) = self.0.lock() else {
            return bundled;
        };
        if !a.enabled || a.broken {
            return bundled;
        }
        let Some(adopted) = a.adopted.clone() else {
            return bundled;
        };
        if Stamp::of(&adopted.path) == Some(adopted.stamp) && adopted.stamp.runnable() {
            adopted.path
        } else {
            a.broken = true;
            bundled
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../qa/fixtures/signatures/yt-dlp-2026.08.19"
    );

    fn fixture(name: &str) -> Vec<u8> {
        fs::read(format!("{FIXTURE}/{name}")).unwrap()
    }

    #[test]
    fn versions_parse_strictly_and_compare_numerically() {
        let v = |s| Version::parse(s);
        assert!(v("2026.08.19").is_some());
        assert_eq!(v("2026.08.19.1").unwrap().to_string(), "2026.08.19.1");
        for bad in [
            "",
            "2026.8.19",
            "2026.08.9",
            "26.08.19",
            "2026.13.01",
            "2026.00.01",
            "2026.08.32",
            "2026.08.19.0",
            "2026.08.19.01",
            "2026.08.19.x",
            "2026.08.19.1.2",
            "v2026.08.19",
            "2026.08.19 ",
            "２０２６.08.19",
            "2026.08.19/..",
            "../2026.08.19",
        ] {
            assert_eq!(v(bad), None, "{bad:?}");
        }
        assert!(v("2026.10.26").unwrap() > v("2026.08.19").unwrap());
        assert!(v("2026.08.19.1").unwrap() > v("2026.08.19").unwrap());
        assert!(v("2026.08.19.10").unwrap() > v("2026.08.19.9").unwrap());
        assert!(v("2027.01.01").unwrap() > v("2026.12.31.5").unwrap());
    }

    #[test]
    fn the_pins_come_from_sidecars_json() {
        let p = pins();
        assert_eq!(p.bundled.to_string(), "2026.08.19");
        assert_eq!(p.fingerprint, "AC0CBBE6848D6A873464AF4E57CF65933B5A7581");
        let names: Vec<_> = p
            .assets
            .iter()
            .map(|(t, a)| (t.as_str(), a.as_str()))
            .collect();
        for want in [
            ("aarch64-apple-darwin", "yt-dlp_macos"),
            ("x86_64-apple-darwin", "yt-dlp_macos"),
            ("x86_64-pc-windows-msvc", "yt-dlp.exe"),
            ("x86_64-unknown-linux-gnu", "yt-dlp_linux"),
        ] {
            assert!(names.contains(&want), "{want:?}");
        }
        if let Some(t) = target() {
            assert!(asset().is_some(), "{t} has an asset");
        }
    }

    #[test]
    fn the_real_signed_sums_verify_and_tampering_fails() {
        let (sums, sig) = (fixture(SUMS), fixture(SIG));
        verify_sums(pinned(), &sums, &sig).unwrap();
        let mut bad = sums.clone();
        bad[10] ^= 1;
        assert!(verify_sums(pinned(), &bad, &sig).is_err());
        let mut bad_sig = sig.clone();
        let last = bad_sig.len() - 3;
        bad_sig[last] ^= 1;
        assert!(verify_sums(pinned(), &sums, &bad_sig).is_err());
        assert!(verify_sums(pinned(), &sums, b"not a signature").is_err());
        let other = Trust {
            key: KEY,
            fingerprint: "0000000000000000000000000000000000000000",
        };
        assert!(
            verify_sums(other, &sums, &sig).is_err(),
            "the pin is checked"
        );
        // The real sums pair the pinned asset with the pinned sha256.
        let sha = asset_sha256(&sums, "yt-dlp_linux").unwrap();
        assert_eq!(
            sha.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "58162f9bfdc27458ea47bfcb311cf47028f17d8154a8bf7d689861d46399230a"
        );
    }

    #[test]
    fn the_asset_line_must_be_exactly_one_sha256() {
        let h = "a".repeat(64);
        let sums = format!("{h}  yt-dlp_linux\n{}  yt-dlp.exe\n", "b".repeat(64));
        assert_eq!(
            asset_sha256(sums.as_bytes(), "yt-dlp_linux").unwrap(),
            [0xaa; 32]
        );
        assert_eq!(
            asset_sha256(format!("{h} *yt-dlp_linux").as_bytes(), "yt-dlp_linux").unwrap(),
            [0xaa; 32]
        );
        for (text, why) in [
            (format!("{h}  yt-dlp_macos\n"), "missing"),
            (format!("{h}  yt-dlp_linux\n{h}  yt-dlp_linux\n"), "twice"),
            (format!("{}  yt-dlp_linux\n", "a".repeat(63)), "short"),
            (format!("{}  yt-dlp_linux\n", "g".repeat(64)), "not hex"),
            (format!("{h}  yt-dlp_linux_aarch64\n"), "another asset"),
            (format!("{h}  yt-dlp_linux.zip\n"), "another asset"),
        ] {
            assert!(
                asset_sha256(text.as_bytes(), "yt-dlp_linux").is_err(),
                "{why}"
            );
        }
        assert!(asset_sha256(&[0xff, 0xfe], "yt-dlp_linux").is_err());
    }

    #[test]
    fn the_redirect_must_name_exactly_a_tag() {
        let ok = format!("{DOWNLOAD}2026.10.26/{SUMS}");
        assert_eq!(
            tag_from_location(&ok, DOWNLOAD).unwrap().to_string(),
            "2026.10.26"
        );
        for bad in [
            format!("{DOWNLOAD}2026.10.26/{SIG}"),
            format!("{DOWNLOAD}nightly/{SUMS}"),
            format!("{DOWNLOAD}2026.10.26/x/{SUMS}"),
            format!("{DOWNLOAD}../2026.10.26/{SUMS}"),
            format!("https://example.com/yt-dlp/releases/download/2026.10.26/{SUMS}"),
            format!("http://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/{SUMS}"),
            "/yt-dlp/yt-dlp/releases/download/2026.10.26/SHA2-256SUMS".into(),
        ] {
            assert!(tag_from_location(&bad, DOWNLOAD).is_err(), "{bad}");
        }
    }

    #[test]
    fn reported_versions_are_one_line() {
        assert_eq!(
            reported_version(b"2026.10.26\n").unwrap().to_string(),
            "2026.10.26"
        );
        assert_eq!(reported_version(b"2026.10.26\nextra"), None);
        assert_eq!(reported_version(b"stable@2026.10.26"), None);
        assert_eq!(reported_version(&[0xff]), None);
    }

    #[test]
    fn a_check_is_due_after_a_day_or_with_a_future_clock() {
        let s = |t| State {
            last_check: t,
            ..Default::default()
        };
        assert!(due(&s(None), 1000));
        assert!(!due(&s(Some(1000)), 1000 + DAY.as_secs() - 1));
        assert!(due(&s(Some(1000)), 1000 + DAY.as_secs()));
        assert!(due(&s(Some(5000)), 1000), "a last check in the future");
    }

    #[test]
    fn resolution_uses_the_adopted_copy_only_while_on_and_unchanged() {
        let dir = std::env::temp_dir().join(format!("queue-ytdlp-resolve-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("yt-dlp");
        fs::write(&file, b"v1").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let bundled = PathBuf::from("/bundled/yt-dlp");
        let r = Resolver::default();
        assert_eq!(r.resolve(bundled.clone()), bundled, "nothing adopted");
        r.adopt(Adopted {
            version: Version::parse("2026.10.26").unwrap(),
            path: file.clone(),
            stamp: Stamp::of(&file).unwrap(),
        });
        assert_eq!(r.resolve(bundled.clone()), bundled, "off");
        r.set_enabled(true);
        assert_eq!(r.resolve(bundled.clone()), file);
        assert_eq!(r.in_use().unwrap().to_string(), "2026.10.26");
        fs::write(&file, b"changed!").unwrap();
        assert_eq!(r.resolve(bundled.clone()), bundled, "a changed file");
        fs::write(&file, b"v1").unwrap();
        assert_eq!(
            r.resolve(bundled.clone()),
            bundled,
            "still bundled this session"
        );
        assert_eq!(r.in_use(), None);
        fs::remove_dir_all(dir).unwrap();
    }
}
