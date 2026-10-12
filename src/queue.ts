// Client for the Rust queue (src-tauri/src/queue.rs). Components never hold queue logic themselves.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Status =
  | { state: "fetching" }
  | { state: "queued" }
  | { state: "downloading" }
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

export interface QueueItem {
  id: number;
  url: string;
  status: Status;
  meta: Meta | null;
  /** Download attempt: 1, then 2–3 when retrying after a 403 (D-017); 0 when not downloading. */
  attempt: number;
  /** Bumped by the backend on every change; higher wins when replies arrive out of order. */
  rev: number;
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
/** "Download all" (Q4): resolves false if a run was already in progress. */
export const runAll = () => invoke<boolean>("queue_run_all");
export const runnerState = () => invoke<boolean>("queue_runner_state");
/** "Download all" starting or finishing. */
export const onRunnerChanged = (handler: (running: boolean) => void): Promise<UnlistenFn> =>
  listen<{ running: boolean }>("queue://runner", (event) => handler(event.payload.running));

/** Background status changes (download finished or failed). */
export const onItemChanged = (handler: (item: QueueItem) => void): Promise<UnlistenFn> =>
  listen<QueueItem>("queue://item", (event) => handler(event.payload));

/**
 * Merge rows from any reply or event into what's shown: one row per id, the highest `rev` wins,
 * in backend order (ascending id). Replies can arrive late or out of order, so never append blindly.
 */
export function mergeItems(shown: QueueItem[], incoming: QueueItem[]): QueueItem[] {
  const byId = new Map(shown.map((item) => [item.id, item]));
  for (const item of incoming) {
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
