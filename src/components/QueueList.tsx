import { SHOW_FILE } from "../platform";
import { formatDuration, progressText, type QueueItem } from "../queue";

interface Props {
  items: QueueItem[];
  onDownload: (id: number) => void;
  /** Q6: a Failed row tries again. */
  onRetry: (id: number) => void;
  /** Q6: removes a row that isn't downloading. */
  onRemove: (id: number) => void;
  /** Q6 (#74): stops a downloading or processing row. */
  onCancel: (id: number) => void;
  /** Rows whose Cancel was pressed and that haven't stopped yet. */
  cancelling: ReadonlySet<number>;
  /** D7: reveals a Done row's file in the OS file manager. */
  onReveal: (id: number) => void;
  /** Updates are arriving (the event listener is up), so a retried row can be followed. */
  connected: boolean;
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
    case "fetching":
      return <span className="text-neutral-500 dark:text-neutral-400">Fetching info…</span>;
    case "queued":
      // E.g. a download cut off when Queue closed (#12); it waits for Download.
      return item.notice ? (
        <span data-role="notice" className="block whitespace-normal text-amber-700 dark:text-amber-400">
          {item.notice}
        </span>
      ) : null;
    case "downloading":
      return (
        <span className="text-neutral-500 dark:text-neutral-400">
          {progressText(item.progress, item.attempt)}
        </span>
      );
    case "processing":
      return <span className="text-neutral-500 dark:text-neutral-400">Processing…</span>;
    case "done":
      return (
        <>
          <span className="text-green-700 dark:text-green-400" title={status.path}>
            ✓ {fileName(status.path)}
          </span>
          {item.notice && (
            <span data-role="notice" className="block whitespace-normal text-amber-700 dark:text-amber-400">
              {item.notice}
            </span>
          )}
        </>
      );
    case "failed":
      return (
        <span role="status" className="text-red-600 dark:text-red-400" title={status.reason}>
          Failed: {status.reason}
        </span>
      );
  }
}

/** 16:9 thumbnail from the site's CDN; a neutral box while fetching or when there is none. */
function Thumbnail({ src }: { src: string | null }) {
  return (
    <div className="h-9 w-16 shrink-0 overflow-hidden rounded bg-neutral-200 dark:bg-neutral-800">
      {src && <img src={src} alt="" loading="lazy" referrerPolicy="no-referrer" className="h-full w-full object-cover" />}
    </div>
  );
}

export default function QueueList({
  items,
  onDownload,
  onRetry,
  onRemove,
  onCancel,
  cancelling,
  onReveal,
  connected,
  busy,
  highlight,
}: Props) {
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
          <Thumbnail src={item.meta?.thumbnail ?? null} />
          <div className="min-w-0 flex-1">
            {item.meta?.title && (
              <div data-role="title" className="truncate font-medium text-neutral-900 dark:text-neutral-100" title={item.meta.title}>
                {item.meta.title}
              </div>
            )}
            {(item.meta?.channel || item.meta?.duration != null) && (
              <div data-role="byline" className="truncate text-xs text-neutral-500 dark:text-neutral-400">
                {[item.meta?.channel, formatDuration(item.meta?.duration)].filter(Boolean).join(" · ")}
              </div>
            )}
            <div
              data-role="url"
              className={`truncate font-mono text-neutral-800 dark:text-neutral-200 ${item.meta?.title ? "text-xs opacity-60" : ""}`}
              title={item.url}
            >
              {item.url}
            </div>
            <div className="truncate text-xs">
              <StatusLabel item={item} />
            </div>
            {item.status.state === "downloading" && item.progress?.percent != null && (
              <div
                role="progressbar"
                aria-label="Download progress"
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={Math.floor(item.progress.percent)}
                className="mt-1 h-1 overflow-hidden rounded bg-neutral-200 dark:bg-neutral-800"
              >
                <div className="h-full bg-neutral-900 transition-[width] dark:bg-neutral-100" style={{ width: `${item.progress.percent}%` }} />
              </div>
            )}
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
          {item.status.state === "done" && (
            <button
              type="button"
              onClick={() => onReveal(item.id)}
              className="shrink-0 rounded-md border border-neutral-300 px-3 py-1 text-xs text-neutral-800 dark:border-neutral-700 dark:text-neutral-200"
            >
              {SHOW_FILE}
            </button>
          )}
          {item.status.state === "failed" && (
            <button
              type="button"
              onClick={() => onRetry(item.id)}
              disabled={!connected}
              className="shrink-0 rounded-md border border-neutral-300 px-3 py-1 text-xs text-neutral-800 dark:border-neutral-700 dark:text-neutral-200 disabled:opacity-40"
            >
              Retry
            </button>
          )}
          {(item.status.state === "downloading" || item.status.state === "processing") && (
            <button
              type="button"
              onClick={() => onCancel(item.id)}
              disabled={cancelling.has(item.id)}
              title="Stop this download. It goes back to the queue."
              className="shrink-0 rounded-md border border-neutral-300 px-3 py-1 text-xs text-neutral-800 dark:border-neutral-700 dark:text-neutral-200 disabled:opacity-40"
            >
              {cancelling.has(item.id) ? "Cancelling…" : "Cancel"}
            </button>
          )}
          {item.status.state !== "downloading" && item.status.state !== "processing" && (
            <button
              type="button"
              onClick={() => onRemove(item.id)}
              aria-label="Remove"
              title="Remove from queue"
              className="shrink-0 rounded-md px-2 py-1 text-xs text-neutral-500 hover:bg-neutral-100 hover:text-neutral-900 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-neutral-100"
            >
              ✕
            </button>
          )}
        </li>
      ))}
    </ul>
  );
}
