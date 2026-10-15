# Security

Please report security problems privately, through GitHub's
[private vulnerability reporting](../../security/advisories/new) for this repository, not in a
public issue. Include what you found, how to reproduce it, and the OS and Queue version.

Queue runs entirely on your computer. It has no accounts and no telemetry. It runs yt-dlp,
ffmpeg and deno as local subprocesses, with arguments passed as an array (never through a
shell). Optional API keys for AI naming live in the OS credential store.
