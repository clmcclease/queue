//! N6 / #30(a): bundled runtime paths preserve native installation directory names.
use queue_lib::local;
use std::path::Path;

#[test]
fn unicode_spaces_and_shell_characters_remain_literal_in_resource_paths() {
    for root in [
        "Queue 日本語 🎵/Resources",
        "Queue's files & $(echo nope); `nope`/Resources",
        "CON/Resources",
        "",
    ] {
        let resources = Path::new(root);
        let expected = resources
            .join("llama")
            .join(format!("llama-server{}", std::env::consts::EXE_SUFFIX));
        assert_eq!(local::server_in(resources), expected);
    }
    let absolute = std::env::temp_dir().join("Queue 日本語 & spaces/Resources");
    assert_eq!(
        local::server_in(&absolute).parent(),
        Some(absolute.join("llama").as_path())
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_resource_directory_is_preserved_without_lossy_conversion() {
    use std::ffi::OsString;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::PathBuf;

    let resources = PathBuf::from(OsString::from_vec(b"Queue-\xff/Resources".to_vec()));
    let server = local::server_in(&resources);
    assert_eq!(
        server.as_os_str().as_bytes(),
        b"Queue-\xff/Resources/llama/llama-server"
    );
}
