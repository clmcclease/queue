import { useEffect, useState } from "react";
import QueueList from "./components/QueueList";
import UrlBar from "./components/UrlBar";
import { addItem, listItems, mergeItems, type QueueItem } from "./queue";

export default function App() {
  const [items, setItems] = useState<QueueItem[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    // Merge rather than replace: a row added before this snapshot arrives must not disappear.
    listItems().then(
      (listed) => setItems((prev) => mergeItems(prev, listed)),
      (e) => setError(String(e)),
    );
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

  return (
    <main className="flex h-screen flex-col bg-white p-4 dark:bg-neutral-900">
      <UrlBar onAdd={add} />
      {error && (
        <p role="alert" className="mt-2 text-sm text-red-600 dark:text-red-400">
          {error}
        </p>
      )}
      <div className="min-h-0 flex-1 overflow-y-auto">
        <QueueList items={items} />
      </div>
    </main>
  );
}
