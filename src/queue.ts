// Client for the Rust queue (src-tauri/src/queue.rs). Components never hold queue logic themselves.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Status =
  | { state: "queued" }
  | { state: "downloading" }
  | { state: "done"; path: string }
  | { state: "failed"; reason: string };

export interface QueueItem {
  id: number;
  url: string;
  status: Status;
  /** Bumped by the backend on every change; higher wins when replies arrive out of order. */
  rev: number;
}

export const addItem = (url: string) => invoke<QueueItem>("queue_add", { url });
export const listItems = () => invoke<QueueItem[]>("queue_list");
export const downloadItem = (id: number) => invoke<QueueItem>("queue_download", { id });
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
