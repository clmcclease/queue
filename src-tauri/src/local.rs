//! The local model runtime (N6, #30): llama.cpp's `llama-server`, bundled with its libraries
//! in one resource folder, `llama/` (D-029). Upstream's `@loader_path`/`$ORIGIN` rpath and
//! ggml's backend search both look beside the executable, so it isn't an `externalBin`. It
//! lands in `<resource_dir>/llama/` when bundled, and in `target/<profile>/llama/` in dev.
//! Starting, health checks and stopping it are #30(b); nothing here runs it yet.
use std::path::{Path, PathBuf};

/// The resource folder's name.
pub const DIR: &str = "llama";

/// `llama-server` in the `llama/` folder under `resources` (Tauri's resource dir).
pub fn server_in(resources: &Path) -> PathBuf {
    resources
        .join(DIR)
        .join(format!("llama-server{}", std::env::consts::EXE_SUFFIX))
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
}
