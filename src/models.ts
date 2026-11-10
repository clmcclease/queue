// The local naming model (N7, #31): its download, size, location and state. Matches
// src-tauri's `ModelStatus`.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type ModelId = "qwen3_4b_instruct_2507" | "qwen3_5_2b";

export interface ModelInfo {
  id: ModelId;
  name: string;
  file: string;
  /** Where it comes from, as text (`huggingface.co/<repo>`). */
  source: string;
  size: number;
  state: "missing" | "partial" | "ready" | "unverified";
  /** Bytes saved so far, for a partial download. */
  received?: number;
  downloading: boolean;
  verifying: boolean;
  error: string | null;
}

export interface ModelStatus {
  /** Rises with every snapshot the backend takes; only a newer one replaces what's shown. */
  rev: number;
  chosen: ModelId;
  /** Whether the user picked it, rather than the default (the 4B, D-044). */
  picked: boolean;
  suggested: ModelId | null;
  dir: string;
  models: ModelInfo[];
}

export const modelStatus = () => invoke<ModelStatus>("model_status");
export const setLocalModel = (model: ModelId) => invoke<ModelStatus>("settings_set_local_model", { model });
/** Starts the download: only ever on the user's click, next to the size and the source. */
export const downloadModel = (model: ModelId) => invoke<ModelStatus>("model_download", { model });
export const cancelModel = () => invoke<ModelStatus>("model_cancel");
export const deleteModel = (model: ModelId) => invoke<ModelStatus>("model_delete", { model });

export const chosenModel = (status: ModelStatus | null) =>
  status?.models?.find((m) => m.id === status.chosen) ?? null;

/** Sizes as people read them: "2.5 GB". */
export const gb = (bytes: number) => `${(bytes / 1e9).toFixed(1)} GB`;

/** Whole-percent progress of a download. */
export const percent = (m: ModelInfo) => Math.floor(((m.received ?? 0) / m.size) * 100);

/** Runs a model command; its reply is shown only if it's newer than what's shown. */
export type Track = (action: () => Promise<ModelStatus>) => Promise<ModelStatus>;

/** How often to ask for the status when its events can't be received. */
const POLL_MS = 1000;

/**
 * The model status, kept current by `queue://model` events. Every snapshot, event or command
 * reply, carries a revision, and only a newer one is shown, so a late event or reply can't
 * undo a newer state (#31 review). It listens first and then asks, so nothing that happens in
 * between is missed; if listening fails, it asks every second instead. `onChange` runs after
 * each newer snapshot, so the naming status (and its banner) can follow a finished download.
 */
export function useModelStatus(onChange?: () => void) {
  const [status, setStatus] = useState<ModelStatus | null>(null);
  const shown = useRef(0);
  const accept = useRef((next: ModelStatus | null | undefined, changed: boolean) => {
    if (!next?.models || !(next.rev > shown.current)) return;
    shown.current = next.rev;
    setStatus(next);
    if (changed) onChange?.();
  });
  useEffect(() => {
    let cancelled = false;
    let stop: (() => void) | undefined;
    let poll: ReturnType<typeof setInterval> | undefined;
    const ask = () =>
      modelStatus().then(
        (s) => !cancelled && accept.current(s, false),
        () => {},
      );
    listen<ModelStatus>("queue://model", (event) => {
      if (!cancelled) accept.current(event.payload, true);
    }).then(
      (unlisten) => {
        if (cancelled) return unlisten();
        stop = unlisten;
        ask();
      },
      () => {
        if (cancelled) return;
        ask();
        poll = setInterval(ask, POLL_MS);
      },
    );
    return () => {
      cancelled = true;
      stop?.();
      if (poll) clearInterval(poll);
    };
    // Listens once; `onChange` from the first render only refreshes the naming status.
  }, []);
  const track: Track = async (action) => {
    const reply = await action();
    accept.current(reply, true);
    return reply;
  };
  return [status, track] as const;
}

/**
 * Whether Queue can't recommend a local model on this computer (D-044): no GPU build, under 8 GB
 * of memory, or memory unknown. Settings then points to the API or Off, and the banner's offer
 * says it may be slow. A status not yet loaded isn't "no recommendation".
 */
export const noRecommendation = (status: ModelStatus | null) => status !== null && status.suggested === null;

/**
 * One line per model from Queue's own tests (D-044), rounded and from one Mac, never "on this
 * computer". Sources: D-031 and D-033 (the owner's held-out titles); the shipped tag list's gain is
 * unmeasured (#143).
 */
export const MODEL_NOTES: Record<ModelId, string> = {
  qwen3_4b_instruct_2507:
    "In Queue's tests it gave the expected name for about 2 in 3 titles, in about 0.7 s each on one Apple-silicon Mac.",
  qwen3_5_2b:
    "Smaller and faster, but much less accurate: the expected name for about 1 in 3 titles in the same tests, in about 0.45 s each.",
};
