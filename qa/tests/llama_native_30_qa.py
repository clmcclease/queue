"""N6 / #30(a): opt-in, offline checks of cached macOS release archives.

Run on macOS arm64 after cargo test and fetching both macOS archives:
    python3 qa/tests/llama_native_30_qa.py

Uses disposable directories only. Verifies the archive hashes independently, checks
every bundled arm64 file against the release archive, and loads each architecture's
runtime from a Unicode/spaced path (x64 through Rosetta). No model is downloaded.
"""

import hashlib
import json
from pathlib import Path
import platform
import subprocess
import tarfile
import tempfile


def main():
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        raise SystemExit("This opt-in native check requires macOS arm64 with Rosetta.")
    root = Path(__file__).resolve().parents[2]
    pins = json.loads((root / "scripts/sidecars.json").read_text())["llama"]
    for target in ["aarch64-apple-darwin", "x86_64-apple-darwin"]:
        pin = pins["assets"][target]
        archive = root / "src-tauri/binaries/.cache" / f"llama-{pins['version']}-{pin['asset']}"
        assert hashlib.sha256(archive.read_bytes()).hexdigest() == pin["sha256"], target
        with tarfile.open(archive, "r:gz") as tar, tempfile.TemporaryDirectory(prefix="queue-30-native-") as temp:
            folder = Path(temp) / "Queue 日本語 & spaces" / "llama"
            folder.mkdir(parents=True)
            for name, entry in pin["files"].items():
                content = tar.extractfile(pin["prefix"] + entry).read()
                dest = folder / name
                dest.write_bytes(content)
                dest.chmod(0o755)
                if target == "aarch64-apple-darwin":
                    bundled = root / "src-tauri/target/debug/llama" / name
                    assert content == bundled.read_bytes(), name
            (folder / "target.txt").write_text(target + "\n")
            subprocess.run(
                ["node", "scripts/fetch-sidecars.mjs", "--target", target, "--check-llama", str(folder)],
                cwd=root, check=True, timeout=130,
            )
            print(f"{target}: pinned sha256, {len(pin['files'])} archive entries, native load PASS", flush=True)


if __name__ == "__main__":
    main()
