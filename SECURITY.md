# Security

Please report security problems privately, through GitHub's
[private vulnerability reporting](../../security/advisories/new) for this repository, not in a
public issue. Include what you found, how to reproduce it, and the OS and Queue version.

Queue runs entirely on your computer. It has no accounts and no telemetry. It runs yt-dlp,
ffmpeg and deno as local subprocesses, with arguments passed as an array (never through a
shell). Optional API keys for AI naming live in the OS credential store.

## yt-dlp updates (#21, D-042)

Queue keeps yt-dlp up to date unless you turn that off in Settings. At most once a day it asks
GitHub for the newest release, so GitHub sees your IP address. It sends nothing about your
downloads. A new copy goes into Queue's own data folder, never over the copy inside the app,
and is used only after all of these pass:
- the release's `SHA2-256SUMS` carries a valid signature from yt-dlp's release key, which is
  committed in this repository (`scripts/keys/yt-dlp.asc`) and pinned by fingerprint;
- the file's sha256 matches its line in those sums;
- run with `--version`, it reports the release it was downloaded as, newer than the copy in
  use, so an older signed release can't be passed off as a new one.

At every launch the stored copy's signature chain is checked again. A copy whose file changes
afterwards isn't used. This isn't a boundary against software already running as you: such a
process could swap in another genuine release.

Signatures are verified with [rPGP](https://github.com/rpgp/rpgp) (`pgp` 0.21.0). Its
dependency tree includes `rsa` 0.9.10 (RUSTSEC-2023-0071) and `crypto-bigint` 0.5.5
(RUSTSEC-2026-0387). Both advisories are timing leaks of secret data, a private key or secret
integers. Queue only verifies a public signature over public data, so neither applies. Audit
tools still flag them.
