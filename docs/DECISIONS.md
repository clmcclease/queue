# Decisions

Status values: **Confirmed** (owner agreed) · **Provisional** (sensible default; can change with rationale).

## D-001 Wrap yt-dlp; don't fork it — Confirmed (2026-10-10)
yt-dlp is designed to be driven by other programs and already pulls page metadata. Forking it would mean taking on its constant fixes for site changes. We bundle the upstream binary and run it as a subprocess.

## D-002 Tauri v2 + React/TS for the desktop shell — Provisional (2026-10-10)
Smaller installer and lower memory than Electron. Keeps Windows and Linux possible later. Alternative: SwiftUI if the app will only ever run on macOS (PRD §12 Q1).

## D-003 Compute the filename before download and pass it via `-o` — Provisional (2026-10-10)
yt-dlp writes the final name directly, so there's no separate rename step and no window where a file has the wrong name. Users can edit the proposed name inline before the download starts.

## D-004 Backlog lives in `docs/BACKLOG.md` and GitHub Issues — Confirmed (2026-10-10)
BACKLOG.md is the source of truth for scope and order. Each item links to a GitHub issue that tracks the work.

## D-005 Private GitHub repo `clmcclease/queue` — Confirmed (2026-10-10)
