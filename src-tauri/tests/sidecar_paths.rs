//! Independent QA for #2 / D6 / D-012: paths remain single OS-string arguments.
use queue_lib::sidecar::{self, Sidecar};
use std::ffi::OsString;
use std::path::Path;

#[test]
fn unicode_and_shell_metacharacters_are_preserved_in_arguments() {
    for name in [
        "Queue 日本語 🎵",
        "Queue's files & $(echo nope); `nope`",
        "CON",
    ] {
        let dir = Path::new(name);
        let mut deno = OsString::from("deno:");
        deno.push(Sidecar::Deno.path_in(dir));
        assert_eq!(
            sidecar::ytdlp_base_args(dir),
            vec![
                OsString::from("--ignore-config"),
                OsString::from("--no-update"),
                OsString::from("--ffmpeg-location"),
                Sidecar::Ffmpeg.path_in(dir).into_os_string(),
                OsString::from("--no-js-runtimes"),
                OsString::from("--js-runtimes"),
                deno,
            ]
        );
    }
}

#[test]
fn sidecar_directory_is_the_running_executables_parent() {
    let exe = std::env::current_exe().unwrap();
    assert_eq!(sidecar::dir().unwrap(), exe.parent().unwrap());
}

#[cfg(unix)]
#[test]
fn non_utf8_install_paths_are_preserved_without_lossy_conversion() {
    use std::os::unix::ffi::OsStringExt;
    let dir = std::path::PathBuf::from(OsString::from_vec(b"Queue-\xff".to_vec()));
    let args = sidecar::ytdlp_base_args(&dir);
    assert_eq!(args[3], Sidecar::Ffmpeg.path_in(&dir).into_os_string());
    let mut deno = OsString::from("deno:");
    deno.push(Sidecar::Deno.path_in(&dir));
    assert_eq!(args[6], deno);
}
