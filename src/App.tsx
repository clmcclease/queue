import { useEffect, useRef, useState } from "react";
import FolderPicker from "./components/FolderPicker";
import QueueList from "./components/QueueList";
import SettingsPanel from "./components/SettingsPanel";
import UrlBar from "./components/UrlBar";
import {
  addItem,
  addManySummary,
  type AddManyResult,
  downloadItem,
  isAddError,
  listItems,
  mergeItems,
  openDownloadFolder,
  revealDownload,
  removeItem,
  retryItem,
  onItemChanged,
  onRunnerChanged,
  runAll,
  runnerState,
  onStorageChanged,
  storageProblem,
  type QueueItem,
} from "./queue";
import { useDownloadFolder } from "./useDownloadFolder";

import { FILE_MANAGER, os, SETTINGS_SHORTCUT } from "./platform";

export default function App() {
  const [items, setItems] = useState<QueueItem[]>([]);
  const [error, setError] = useState<string | null>(null);
  // Download results arrive only as events, so downloads stay off until the listener is up. A
  // registration failure is kept separately: a later successful command must not hide it.
  const [listening, setListening] = useState(false);
  const [disconnected, setDisconnected] = useState<string | null>(null);
  // The queue can't be saved this session (#12); shown until restart.
  const [unsaved, setUnsaved] = useState<string | null>(null);
  // "Download all" (Q4) runs in the backend; this mirrors it for the button.
  const [running, setRunning] = useState(false);
  // Duplicate links (Q2): a short toast and a brief highlight on the row that already has it.
  const [toast, setToast] = useState<string | null>(null);
  const [highlight, setHighlight] = useState<number | null>(null);
  const toastTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  // The download folder (D1), shared by the "Saving to" line and Settings so both agree.
  const folder = useDownloadFolder();
  const [settingsOpen, setSettingsOpen] = useState(false);
  // What had focus when Settings opened (the gear, or the field for ⌘,). Recorded here, before
  // the queue goes inert, and restored after it isn't, so StrictMode's effect replay can't lose it.
  const settingsOpener = useRef<HTMLElement | null>(null);
  function openSettings() {
    if (!settingsOpener.current) settingsOpener.current = document.activeElement as HTMLElement | null;
    setSettingsOpen(true);
  }
  useEffect(() => {
    if (settingsOpen) return;
    settingsOpener.current?.focus?.();
    settingsOpener.current = null;
  }, [settingsOpen]);
  // Rows removed here (Q6): a late event or snapshot must not bring them back.
  const removed = useRef(new Set<number>());

  function flashDuplicate(message: string, id: number) {
    showToast(message);
    setHighlight(id);
  }

  function showToast(message: string) {
    setToast(message);
    clearTimeout(toastTimer.current);
    toastTimer.current = setTimeout(() => {
      setToast(null);
      setHighlight(null);
    }, 3000);
  }

  useEffect(() => () => clearTimeout(toastTimer.current), []);

  useEffect(() => {
    // Listen first, then read the current state; an event is at least as new as the read.
    let cancelled = false;
    let eventSeen = false;
    let stop: (() => void) | undefined;
    onStorageChanged((problem) => {
      eventSeen = true;
      setUnsaved(problem);
    })
      .then((unlisten) => (cancelled ? unlisten() : (stop = unlisten)))
      .catch(() => {})
      .finally(() =>
        storageProblem().then(
          (problem) => !cancelled && !eventSeen && setUnsaved(problem ?? null),
          () => {},
        ),
      );
    return () => {
      cancelled = true;
      stop?.();
    };
  }, []);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((os === "macos" ? e.metaKey : e.ctrlKey) && e.key === ",") {
        e.preventDefault();
        openSettings();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    // Listen before taking the snapshot, so a download finishing in between can't be missed.
    let cancelled = false;
    const stops: (() => void)[] = [];
    let runnerEventSeen = false;
    (async () => {
      // Both event streams are required: without them results never arrive, so a failure here
      // is a lasting "disconnected" state that later successful commands must not clear.
      try {
        for (const subscribe of [
          () => onItemChanged((item) => setItems((prev) => mergeItems(prev, [item], removed.current))),
          () =>
            onRunnerChanged((running) => {
              runnerEventSeen = true;
              setRunning(running);
            }),
        ]) {
          const unlisten = await subscribe();
          if (cancelled) return unlisten();
          stops.push(unlisten);
        }
      } catch (e) {
        if (!cancelled) setDisconnected(String(e));
        return;
      }
      setListening(true);
      try {
        const listed = await listItems();
        // Merge rather than replace: a row added before this snapshot arrives must not disappear.
        if (!cancelled) setItems((prev) => mergeItems(prev, listed, removed.current));
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
      // Initial runner state only if no runner event has arrived yet: an event is at least as
      // new as this snapshot, and every later change emits another event. Doesn't gate the
      // list; false (the default) is safe if it can't be read.
      runnerState().then(
        (r) => !cancelled && !runnerEventSeen && setRunning(r),
        () => {},
      );
    })();
    return () => {
      cancelled = true;
      stops.forEach((stop) => stop());
    };
  }, []);

  async function add(url: string) {
    try {
      const item = await addItem(url);
      setItems((prev) => mergeItems(prev, [item], removed.current));
      setError(null);
      return true;
    } catch (e) {
      if (isAddError(e) && e.kind === "duplicate") {
        setError(null);
        flashDuplicate(e.message, e.id);
        return true; // already queued, so clear the field
      }
      setError(isAddError(e) ? e.message : String(e));
      return false; // keep the text so it can be fixed
    }
  }

  // Q8: several pasted links, added one at a time in order (each fetches its info as usual).
  async function addMany(urls: string[]) {
    const result: AddManyResult = { total: urls.length, added: 0, duplicates: 0, invalid: [], failed: [] };
    for (const url of urls) {
      try {
        const item = await addItem(url);
        setItems((prev) => mergeItems(prev, [item], removed.current));
        result.added++;
      } catch (e) {
        if (isAddError(e) && e.kind === "duplicate") result.duplicates++;
        else if (isAddError(e)) result.invalid.push(url);
        else result.failed.push(String(e));
      }
    }
    const { toast, error } = addManySummary(result);
    setHighlight(null);
    showToast(toast);
    setError(error);
  }

  async function download(id: number) {
    try {
      const item = await downloadItem(id);
      setItems((prev) => mergeItems(prev, [item], removed.current));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  async function remove(id: number) {
    try {
      await removeItem(id);
      removed.current.add(id);
      setItems((prev) => prev.filter((item) => item.id !== id));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  async function retry(id: number) {
    try {
      const item = await retryItem(id);
      setItems((prev) => mergeItems(prev, [item], removed.current));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  async function openFolder() {
    try {
      await openDownloadFolder();
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  async function reveal(id: number) {
    try {
      const notice = await revealDownload(id);
      setError(null);
      if (notice) showToast(notice);
    } catch (e) {
      setError(String(e));
    }
  }

  async function downloadAll() {
    try {
      // "Running" comes only from queue://runner events (emitted before this reply), so a late
      // reply can't revive a run that has already finished.
      await runAll();
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  const anyQueued = items.some((item) => item.status.state === "queued");

  return (
    <main className="relative flex h-screen flex-col bg-white p-4 dark:bg-neutral-900">
      {/* While Settings is open, the queue behind it can't be focused or clicked. */}
      <div inert={settingsOpen} className="flex min-h-0 flex-1 flex-col">
        <div className="flex gap-2">
          <div className="min-w-0 flex-1">
            <UrlBar onAdd={add} onAddMany={addMany} />
          </div>
          <button
            type="button"
            onClick={openSettings}
            aria-label="Settings"
            title={`Settings (${SETTINGS_SHORTCUT})`}
            className="shrink-0 rounded-md px-2 text-lg text-neutral-500 hover:bg-neutral-100 hover:text-neutral-900 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-neutral-100"
          >
            ⚙
          </button>
        </div>
        <div className="mt-2 flex items-start gap-3">
          <FolderPicker state={folder} />
          <button
            type="button"
            onClick={openFolder}
            title={`Open the download folder in ${FILE_MANAGER}`}
            className="shrink-0 rounded-md border border-neutral-300 px-3 py-1.5 text-sm text-neutral-800 dark:border-neutral-700 dark:text-neutral-200"
          >
            Open folder
          </button>
          <button
            type="button"
            onClick={downloadAll}
            disabled={!listening || running || !anyQueued}
            className="shrink-0 rounded-md bg-neutral-900 px-3 py-1.5 text-sm font-medium text-white disabled:opacity-40 dark:bg-neutral-100 dark:text-neutral-900"
          >
            {running ? "Downloading all…" : "Download all"}
          </button>
        </div>
        {unsaved && (
          <p role="alert" data-role="unsaved" className="mt-2 text-sm text-red-600 dark:text-red-400">
            {unsaved}
          </p>
        )}
        {disconnected && (
          <p role="alert" data-role="disconnected" className="mt-2 text-sm text-red-600 dark:text-red-400">
            Can't receive download updates ({disconnected}). Restart Queue to download.
          </p>
        )}
        {error && (
          <p role="alert" className="mt-2 text-sm text-red-600 dark:text-red-400">
            {error}
          </p>
        )}
        <div className="min-h-0 flex-1 overflow-y-auto">
          <QueueList
            items={items}
            onDownload={download}
            onRetry={retry}
            onRemove={remove}
            onReveal={reveal}
            connected={listening}
            busy={!listening || running || items.some((item) => item.status.state === "downloading" || item.status.state === "processing")}
            highlight={highlight}
          />
        </div>
        {toast && (
          <div
            role="status"
            data-role="toast"
            className="pointer-events-none fixed inset-x-0 bottom-4 mx-auto w-fit rounded-md bg-neutral-900 px-3 py-2 text-sm text-white shadow-lg dark:bg-neutral-100 dark:text-neutral-900"
          >
            {toast}
          </div>
        )}
      </div>
      {settingsOpen && <SettingsPanel folder={folder} onClose={() => setSettingsOpen(false)} />}
    </main>
  );
}
