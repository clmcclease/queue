import { useEffect, useState } from "react";
import QueueList from "./components/QueueList";
import UrlBar from "./components/UrlBar";
import { addItem, downloadItem, listItems, mergeItems, onItemChanged, type QueueItem } from "./queue";

export default function App() {
  const [items, setItems] = useState<QueueItem[]>([]);
  const [error, setError] = useState<string | null>(null);
  // Download results arrive only as events, so downloads stay off until the listener is up. A
  // registration failure is kept separately: a later successful command must not hide it.
  const [listening, setListening] = useState(false);
  const [disconnected, setDisconnected] = useState<string | null>(null);

  useEffect(() => {
    // Listen before taking the snapshot, so a download finishing in between can't be missed.
    let cancelled = false;
    let stop: (() => void) | undefined;
    (async () => {
      try {
        const unlisten = await onItemChanged((item) => setItems((prev) => mergeItems(prev, [item])));
        if (cancelled) return unlisten();
        stop = unlisten;
        setListening(true);
        const listed = await listItems();
        // Merge rather than replace: a row added before this snapshot arrives must not disappear.
        if (!cancelled) setItems((prev) => mergeItems(prev, listed));
      } catch (e) {
        if (cancelled) return;
        if (stop) setError(String(e));
        else setDisconnected(String(e));
      }
    })();
    return () => {
      cancelled = true;
      stop?.();
    };
  }, []);

  async function add(url: string) {
    try {
      const item = await addItem(url);
      setItems((prev) => mergeItems(prev, [item]));
      setError(null);
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  }

  async function download(id: number) {
    try {
      const item = await downloadItem(id);
      setItems((prev) => mergeItems(prev, [item]));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <main className="flex h-screen flex-col bg-white p-4 dark:bg-neutral-900">
      <UrlBar onAdd={add} />
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
          busy={!listening || items.some((item) => item.status.state === "downloading")}
        />
      </div>
    </main>
  );
}
