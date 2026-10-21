// Client for the Rust queue (src-tauri/src/queue.rs). Components never hold queue logic themselves.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Status =
  | { state: "fetching" }
  | { state: "queued" }
  | { state: "downloading" }
  | { state: "processing" }
  | { state: "done"; path: string }
  | { state: "failed"; reason: string };

/** From `yt-dlp -J` (src-tauri/src/meta.rs). Untrusted text: display only. */
export interface Meta {
  title: string | null;
  channel: string | null;
  /** Seconds. */
  duration: number | null;
  thumbnail: string | null;
  site: string | null;
}

/** Live download progress (src-tauri/src/download.rs `Progress`); every field may be missing. */
export interface Progress {
  percent: number | null;
  downloaded: number | null;
  total: number | null;
  /** Bytes per second. */
  speed: number | null;
  /** Seconds left. */
  eta: number | null;
  /** 1 = first stream (video), 2 = second (audio) for merged downloads. */
  part: number;
}

export interface QueueItem {
  id: number;
  url: string;
  status: Status;
  meta: Meta | null;
  progress?: Progress | null;
  /** Download attempt: 1, then 2–3 when retrying after a 403 (D-017); 0 when not downloading. */
  attempt: number;
  /** About a Done row's file, e.g. a "most compatible" format that wasn't available (#11). */
  notice?: string | null;
  /** Bumped by the backend on every change; higher wins when replies arrive out of order. */
  rev: number;
  /** N1 (#15): the file name it will be saved as, without extension; set once the info arrives. */
  name?: string | null;
  /** Where the name came from; an `edited` name is never replaced automatically. */
  name_source?: "rules" | "ai" | "edited" | null;
}

/** Why `addItem` rejected a link (src-tauri/src/queue.rs `AddError`). */
export type AddError =
  | { kind: "invalid"; message: string }
  | { kind: "duplicate"; message: string; id: number };

export function isAddError(e: unknown): e is AddError {
  return typeof e === "object" && e !== null && "kind" in e && "message" in e;
}

/** Rejects with an {@link AddError} for invalid or duplicate links. */
export const addItem = (url: string) => invoke<QueueItem>("queue_add", { url });
export const listItems = () => invoke<QueueItem[]>("queue_list");
export const downloadItem = (id: number) => invoke<QueueItem>("queue_download", { id });
/**
 * Q6 (#74): cancels a download in progress. Its row comes back Queued, marked Cancelled, as a
 * `queue://item` event once the download has stopped (or Done, if it finished first).
 */
export const cancelItem = (id: number) => invoke<void>("queue_cancel", { id });
/** Q6: removes a row that isn't downloading (a Done row's file stays on disk). */
export const removeItem = (id: number) => invoke<void>("queue_remove", { id });
/** D7: opens the download folder in the OS file manager. */
export const openDownloadFolder = () => invoke<void>("open_download_folder");
/**
 * D7: reveals a Done row's file in the OS file manager (by row id, never by path). Resolves a
 * notice when only its folder could be opened (Linux without a file manager that can select).
 */
export const revealDownload = (id: number) => invoke<string | null>("reveal_download", { id });
/** N1 (#15): renames a Queued row. The backend sanitizes the name and returns the updated row. */
export const renameItem = (id: number, name: string) => invoke<QueueItem>("queue_rename", { id, name });
/** Q6: a Failed row goes back to Fetching (its info never arrived) or Queued. */
export const retryItem = (id: number) => invoke<QueueItem>("queue_retry", { id });
/** "Download all" (Q4): resolves false if a run was already in progress. */
export const runAll = () => invoke<boolean>("queue_run_all");
export const runnerState = () => invoke<boolean>("queue_runner_state");
/** Why the queue won't be saved this session (#12, e.g. another copy still holds it), or null. */
export const storageProblem = () => invoke<string | null>("queue_storage_problem");
/** A save started failing (the reason) or works again (null). */
export const onStorageChanged = (handler: (problem: string | null) => void): Promise<UnlistenFn> =>
  listen<string | null>("queue://storage", (event) => handler(event.payload));
/** "Download all" starting or finishing. */
export const onRunnerChanged = (handler: (running: boolean) => void): Promise<UnlistenFn> =>
  listen<{ running: boolean }>("queue://runner", (event) => handler(event.payload.running));

/** Background status changes (download finished or failed). */
export const onItemChanged = (handler: (item: QueueItem) => void): Promise<UnlistenFn> =>
  listen<QueueItem>("queue://item", (event) => handler(event.payload));

/**
 * Merge rows from any reply or event into what's shown: one row per id, the highest `rev` wins,
 * in backend order (ascending id). Replies can arrive late or out of order, so never append blindly.
 * Ids in `removed` (Q6) stay gone even if an older event or snapshot still has them; the backend
 * never reuses an id.
 */
export function mergeItems(
  shown: QueueItem[],
  incoming: QueueItem[],
  removed: ReadonlySet<number> = new Set(),
): QueueItem[] {
  const byId = new Map(shown.map((item) => [item.id, item]));
  for (const item of incoming) {
    if (removed.has(item.id)) continue;
    const current = byId.get(item.id);
    if (!current || item.rev >= current.rev) byId.set(item.id, item);
  }
  return [...byId.values()].sort((a, b) => a.id - b.id);
}

/** `1:02:03` / `4:05`; empty when unknown. */
export function formatDuration(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return "";
  const total = Math.round(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

/** `3.2 MB/s`; empty when unknown. Decimal units, as file managers show them. */
export function formatSpeed(bytesPerSecond: number | null | undefined): string {
  if (bytesPerSecond == null || !Number.isFinite(bytesPerSecond) || bytesPerSecond < 0) return "";
  const units = ["B/s", "KB/s", "MB/s", "GB/s"];
  let value = bytesPerSecond;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

/**
 * "Downloading… 42% · 3.2 MB/s · 0:35 left (part 2)" from whatever yt-dlp reported; a retry
 * after a 403 (D-017) leads with "Retrying (2/3)…" and keeps the live numbers.
 */
export function progressText(progress: Progress | null | undefined, attempt = 1): string {
  const parts = [attempt > 1 ? `Retrying (${attempt}/3)…` : "Downloading…"];
  if (progress?.percent != null) parts[0] += ` ${Math.floor(progress.percent)}%`;
  const speed = formatSpeed(progress?.speed);
  if (speed) parts.push(speed);
  const eta = formatDuration(progress?.eta);
  if (eta && progress?.eta) parts.push(`${eta} left`);
  const text = parts.join(" · ");
  return progress && progress.part > 1 ? `${text} (part ${progress.part})` : text;
}

/**
 * Q8 (#13): the links in pasted text, one per non-blank line, trimmed. `null` for a single line,
 * which is pasted into the field as usual.
 */
export function splitLinks(text: string): string[] | null {
  const lines = text
    .split(/\r\n|\r|\n/)
    .map((line) => line.trim())
    .filter((line) => line !== "");
  return lines.length > 1 ? lines : null;
}

/** How adding several pasted links went (Q8). */
export interface AddManyResult {
  total: number;
  added: number;
  duplicates: number;
  /** Lines that weren't links, as pasted. */
  invalid: string[];
  /** Other errors, e.g. the backend didn't answer. */
  failed: string[];
}

/** Q8: the summary toast after adding several links, and an error naming the lines that need fixing. */
export function addManySummary(r: AddManyResult): { toast: string; error: string | null } {
  const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
  const notes = [
    r.duplicates && `${r.duplicates} already in the queue`,
    r.invalid.length && `${r.invalid.length} not a link`,
    r.failed.length && `${r.failed.length} couldn't be added`,
  ].filter(Boolean);
  const toast =
    r.added === r.total
      ? `Added ${plural(r.added, "link")}.`
      : `Added ${r.added} of ${plural(r.total, "link")}${notes.length ? `: ${notes.join(", ")}` : ""}.`;
  const shown = r.invalid.slice(0, 3).map((line) => `"${line.length > 60 ? `${line.slice(0, 57)}...` : line}"`);
  const more = r.invalid.length > shown.length ? ` and ${r.invalid.length - shown.length} more` : "";
  const errors = [
    r.invalid.length ? `Not a link: ${shown.join(", ")}${more}.` : null,
    r.failed.length ? r.failed[0] : null,
  ].filter((e): e is string => e !== null);
  return { toast, error: errors.length ? errors.join(" ") : null };
}
