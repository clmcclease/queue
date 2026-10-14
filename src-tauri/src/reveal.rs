//! Opens the download folder and reveals (selects) saved files in the OS file manager (D7,
//! D-022). Blocking: callers run these on a worker thread, never the main thread, so a slow
//! network folder or a stuck file manager can't freeze the window. Every launch reports its
//! failure: opener 2.7.0's detached launchers report success before the launcher has even
//! checked the path, so Queue waits for each launcher's exit status instead.
use std::path::Path;

/// How a reveal went.
#[derive(Debug, PartialEq, Eq)]
pub enum Shown {
    /// The file manager opened the folder with the file selected.
    Selected,
    /// Only the folder could be opened (Linux without `org.freedesktop.FileManager1`).
    FolderOnly,
}

fn name(file: &Path) -> String {
    file.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Runs a launcher (`open`, `xdg-open`) with an argument array (no shell) and waits for its
/// verdict: a non-zero exit is reported as `failed`. There's no timeout. These launchers exit as
/// soon as the file manager has the request, and only a confirmed exit counts as success. This
/// runs on a worker thread, so a slow one never blocks the window, and the child is always reaped.
#[cfg(not(windows))]
fn launch(
    program: &str,
    args: &[&std::ffi::OsStr],
    failed: impl FnOnce() -> String,
) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("Couldn't start {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} ({program}: {status})", failed()))
    }
}

/// Opens `dir` in the file manager.
#[cfg(target_os = "macos")]
pub fn open_folder(dir: &Path) -> Result<(), String> {
    launch("/usr/bin/open", &[dir.as_os_str()], || {
        format!("Finder couldn't open {}.", dir.display())
    })
}

/// Reveals `file` in Finder with `open -R`, the system's own "Show in Finder". The path is
/// absolute (Queue saved it), so it can't be read as an option.
#[cfg(target_os = "macos")]
pub fn reveal(file: &Path) -> Result<Shown, String> {
    launch("/usr/bin/open", &["-R".as_ref(), file.as_os_str()], || {
        format!("Finder couldn't show {}.", name(file))
    })?;
    Ok(Shown::Selected)
}

/// Opens `dir` with `xdg-open`, waiting for its verdict (it fails when no file manager is set).
#[cfg(not(any(target_os = "macos", windows)))]
pub fn open_folder(dir: &Path) -> Result<(), String> {
    launch("xdg-open", &[dir.as_os_str()], || {
        format!("Couldn't open {} in your file manager.", dir.display())
    })
}

/// Selects `file` through the file manager's `org.freedesktop.FileManager1.ShowItems` (via the
/// opener plugin, which reports D-Bus errors). If that's unavailable, opens the folder instead
/// and says so: opener 2.7.0's own portal fallback is broken (it swaps the portal's service and
/// interface names), and the portal can't promise a selection anyway.
#[cfg(not(any(target_os = "macos", windows)))]
pub fn reveal(file: &Path) -> Result<Shown, String> {
    if tauri_plugin_opener::reveal_item_in_dir(file).is_ok() {
        return Ok(Shown::Selected);
    }
    open_folder(file.parent().unwrap_or(file))
        .map_err(|_| format!("Couldn't show {} in your file manager.", name(file)))?;
    Ok(Shown::FolderOnly)
}

/// Runs shell calls with COM initialized on this (worker) thread, as the shell requires: folder
/// handlers and shell extensions may be COM objects. COM is uninitialized only if this call
/// initialized it (S_OK or S_FALSE); a thread already in another apartment is used as it is.
#[cfg(windows)]
fn with_com<T>(work: impl FnOnce() -> T) -> T {
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize,
    };
    // SAFETY: plain COM calls, balanced below.
    let com = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
    let result = work();
    if com >= 0 {
        unsafe { CoUninitialize() };
    }
    result
}

#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Opens `dir` in File Explorer with `ShellExecuteExW`, which fails (rather than launching a
/// helper that fails later) when the folder can't be opened.
#[cfg(windows)]
pub fn open_folder(dir: &Path) -> Result<(), String> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    let file = wide(dir);
    let verb: Vec<u16> = "explore\0".encode_utf16().collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        // No error dialogs (the reason shows in Queue); finish before returning.
        fMask: SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        nShow: 1, // SW_SHOWNORMAL
        ..Default::default()
    };
    // SAFETY: `info` is fully initialized, and `file`/`verb` are NUL-terminated and outlive
    // the call; the error is read on the same thread right after it.
    let opened = with_com(|| unsafe {
        if ShellExecuteExW(&mut info) != 0 {
            Ok(())
        } else {
            Err(GetLastError())
        }
    });
    opened.map_err(|code| {
        format!(
            "File Explorer couldn't open {} (error {code}).",
            dir.display()
        )
    })
}

/// Selects `file` with `SHOpenFolderAndSelectItems`, the call File Explorer uses itself, with
/// its error reported (opener 2.7.0 drops most of them). With no children, it opens the file's
/// folder and selects the file.
#[cfg(windows)]
pub fn reveal(file: &Path) -> Result<Shown, String> {
    use windows_sys::Win32::UI::Shell::{ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems};

    let path = wide(file);
    // SAFETY: `path` is NUL-terminated and outlives the calls; the ITEMIDLIST is freed once.
    let hr = with_com(|| unsafe {
        let item = ILCreateFromPathW(path.as_ptr());
        if item.is_null() {
            return None;
        }
        let hr = SHOpenFolderAndSelectItems(item, 0, std::ptr::null(), 0);
        ILFree(item);
        Some(hr)
    });
    match hr {
        None => Err(format!("File Explorer couldn't find {}.", name(file))),
        Some(hr) if hr < 0 => Err(format!(
            "File Explorer couldn't show {} (error 0x{:08X}).",
            name(file),
            hr as u32
        )),
        Some(_) => Ok(Shown::Selected),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_launcher_that_fails_is_reported_and_one_that_succeeds_is_not() {
        let failed = launch("/bin/sh", &["-c".as_ref(), "exit 3".as_ref()], || {
            "It failed.".into()
        });
        let err = failed.unwrap_err();
        assert!(
            err.starts_with("It failed. (/bin/sh: exit status: 3"),
            "{err}"
        );
        assert_eq!(
            launch(
                "/bin/sh",
                &["-c".as_ref(), "exit 0".as_ref()],
                || unreachable!()
            ),
            Ok(())
        );
        let missing = launch("/no/such/launcher", &[], || unreachable!()).unwrap_err();
        assert!(
            missing.starts_with("Couldn't start /no/such/launcher"),
            "{missing}"
        );
    }

    #[test]
    fn a_slow_launcher_that_fails_is_never_reported_as_success() {
        let slow = launch(
            "/bin/sh",
            &["-c".as_ref(), "sleep 1; exit 4".as_ref()],
            || "Late failure.".into(),
        );
        assert!(
            slow.unwrap_err()
                .starts_with("Late failure. (/bin/sh: exit status: 4")
        );
    }

    #[test]
    fn arguments_are_passed_as_is_never_through_a_shell() {
        let hostile = "$(touch ran); `touch ran`; 日本語 'q' \"d\"";
        // The script exits 0 only if it got exactly one argument, byte for byte.
        let script = format!(
            "[ \"$#\" = 1 ] && [ \"$1\" = '{}' ]",
            hostile.replace('\'', "'\\''")
        );
        let args = [
            "-c".as_ref(),
            script.as_ref(),
            "sh".as_ref(),
            hostile.as_ref(),
        ];
        assert_eq!(launch("/bin/sh", &args, || "mangled".into()), Ok(()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_failed_reveal_or_open_is_reported_not_swallowed() {
        // Nothing opens: `open` exits non-zero for a path that doesn't exist.
        let missing =
            std::env::temp_dir().join(format!("queue-no-such-file-{}.webm", std::process::id()));
        let err = reveal(&missing).unwrap_err();
        assert!(
            err.starts_with("Finder couldn't show queue-no-such-file-"),
            "{err}"
        );
        let err = open_folder(&missing).unwrap_err();
        assert!(err.starts_with("Finder couldn't open "), "{err}");
    }
}
