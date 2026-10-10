// Client for the Rust queue (src-tauri/src/queue.rs). Components never hold queue logic themselves.
import { invoke } from "@tauri-apps/api/core";

export interface QueueItem {
  id: number;
  url: string;
}

export const addItem = (url: string) => invoke<QueueItem>("queue_add", { url });
export const listItems = () => invoke<QueueItem[]>("queue_list");

/**
 * Merge rows from any reply (initial snapshot or an add) into what's shown: one row per id, in
 * backend order (ascending id). Replies can arrive late or out of order, so never append blindly.
 */
export function mergeItems(shown: QueueItem[], incoming: QueueItem[]): QueueItem[] {
  const byId = new Map(shown.map((item) => [item.id, item]));
  for (const item of incoming) byId.set(item.id, item);
  return [...byId.values()].sort((a, b) => a.id - b.id);
}
