import type { QueueItem } from "../queue";

interface Props {
  items: QueueItem[];
  onDownload: (id: number) => void;
  /** A download is running; only one at a time (see queue.rs `start`). */
  busy: boolean;
  /** Row to flash briefly, e.g. the existing row for a duplicate link. */
  highlight?: number | null;
}

function fileName(path: string) {
  return path.split(/[\\/]/).pop() ?? path;
}

function StatusLabel({ item }: { item: QueueItem }) {
  const { status } = item;
  switch (status.state) {
    case "queued":
      return null;
    case "downloading":
      return <span className="text-neutral-500 dark:text-neutral-400">Downloading…</span>;
    case "done":
      return (
        <span className="text-green-700 dark:text-green-400" title={status.path}>
          ✓ {fileName(status.path)}
        </span>
      );
    case "failed":
      return (
        <span role="status" className="text-red-600 dark:text-red-400" title={status.reason}>
          Failed: {status.reason}
        </span>
      );
  }
}

export default function QueueList({ items, onDownload, busy, highlight }: Props) {
  if (items.length === 0) {
    return <p className="mt-16 text-center text-sm text-neutral-500 dark:text-neutral-400">Queue is empty.</p>;
  }
  return (
    <ul aria-label="Queue" className="mt-4 divide-y divide-neutral-200 dark:divide-neutral-800">
      {items.map((item) => (
        <li
          key={item.id}
          data-state={item.status.state}
          data-highlight={item.id === highlight ? "" : undefined}
          className={`flex items-center gap-3 rounded py-2 text-sm transition-colors ${item.id === highlight ? "bg-amber-100 dark:bg-amber-900/40" : ""}`}
        >
          <div className="min-w-0 flex-1">
            <div data-role="url" className="truncate font-mono text-neutral-800 dark:text-neutral-200" title={item.url}>
              {item.url}
            </div>
            <div className="truncate text-xs">
              <StatusLabel item={item} />
            </div>
          </div>
          {item.status.state === "queued" && (
            <button
              type="button"
              onClick={() => onDownload(item.id)}
              disabled={busy}
              title={busy ? "One download at a time" : undefined}
              className="shrink-0 rounded-md border border-neutral-300 px-3 py-1 text-xs text-neutral-800 dark:border-neutral-700 dark:text-neutral-200 disabled:opacity-40"
            >
              Download
            </button>
          )}
        </li>
      ))}
    </ul>
  );
}
