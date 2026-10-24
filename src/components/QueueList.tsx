import { useEffect, useRef, useState } from "react";
import { REMOVE_SHORTCUT, SHOW_FILE } from "../platform";
import { formatDuration, progressText, type QueueItem } from "../queue";

interface Props {
  items: QueueItem[];
  onDownload: (id: number) => void;
  /** Q6: a Failed row tries again. */
  onRetry: (id: number) => void;
  /** Q6: removes a row that isn't downloading; settles once the backend has answered. */
  onRemove: (id: number) => void | Promise<void>;
  /** N1 (#15): renames a Queued row's file (the backend sanitizes it). */
  onRename: (id: number, name: string) => void;
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
  /** #19: Escape on a row, or removing the last one, returns focus to the URL bar. */
  onLeave?: () => void;
}

/** Whether a row can be removed: not while it's downloading or processing (Q6). */
const removable = (item: QueueItem) => item.status.state !== "downloading" && item.status.state !== "processing";

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

/**
 * N1 (#15): the name the file will be saved as. On a Queued row a click edits it inline: Enter or
 * leaving the field saves, Escape cancels. Other rows just show it.
 */
function NameField({ item, onRename }: { item: QueueItem; onRename: (id: number, name: string) => void }) {
  const [draft, setDraft] = useState<string | null>(null);
  const cancelled = useRef(false);
  const queued = item.status.state === "queued";
  // A download that starts (e.g. from Download all) closes an open editor without saving: the
  // name was captured when it started.
  useEffect(() => {
    if (!queued) {
      cancelled.current = true;
      setDraft(null);
    }
  }, [queued]);
  if (!item.name) return null;
  const name = item.name;
  if (draft !== null && queued) {
    const finish = () => {
      const next = draft.trim();
      setDraft(null);
      if (!cancelled.current && next && next !== name) onRename(item.id, next);
      cancelled.current = false;
    };
    return (
      <input
        aria-label="File name"
        autoFocus
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={finish}
        onKeyDown={(e) => {
          // Enter while an input method is composing (Japanese, Chinese, Korean…) picks the
          // candidate; it doesn't save.
          if (e.nativeEvent.isComposing || e.keyCode === 229) return;
          if (e.key === "Enter") {
            e.preventDefault();
            e.currentTarget.blur();
          } else if (e.key === "Escape") {
            e.preventDefault();
            cancelled.current = true;
            e.currentTarget.blur();
          }
        }}
        className="mt-0.5 w-full rounded border border-neutral-300 bg-white px-1 py-0.5 text-xs text-neutral-900 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
      />
    );
  }
  const text = "truncate text-xs text-neutral-600 dark:text-neutral-300";
  // N4 (#25): "AI" when the AI named it; "rules" only when it was asked and didn't answer.
  const tag =
    item.name_source === "ai" ? (
      <span data-role="name-tag" className="ml-1 rounded bg-neutral-100 px-1 text-[10px] text-neutral-500 dark:bg-neutral-800 dark:text-neutral-400">
        AI
      </span>
    ) : item.name_source === "fallback" ? (
      <span
        data-role="name-tag"
        title="AI naming didn't give a usable name, so the rules named this."
        className="ml-1 rounded bg-neutral-100 px-1 text-[10px] text-neutral-500 dark:bg-neutral-800 dark:text-neutral-400"
      >
        rules
      </span>
    ) : null;
  return queued ? (
    <button
      type="button"
      data-role="name"
      onClick={() => {
        cancelled.current = false;
        setDraft(name);
      }}
      title="Saved as this name. Click to change it."
      className={`block max-w-full text-left hover:underline ${text}`}
    >
      {name}
      {tag}
    </button>
  ) : (
    <div data-role="name" title={name} className={text}>
      {name}
      {tag}
    </div>
  );
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
  onRename,
  onRemove,
  onCancel,
  cancelling,
  onReveal,
  connected,
  busy,
  highlight,
  onLeave,
}: Props) {
  const rows = useRef(new Map<number, HTMLLIElement>());
  // Rows removed from the keyboard whose removal hasn't come back yet: focus never lands on one,
  // or it would fall to the page when that row goes (#19 QA).
  const leaving = useRef(new Set<number>());
  if (items.length === 0) {
    return <p className="mt-16 text-center text-sm text-neutral-500 dark:text-neutral-400">Queue is empty.</p>;
  }
  return (
    <ul aria-label="Queue" className="mt-4 divide-y divide-neutral-200 dark:divide-neutral-800">
      {items.map((item) => (
        <li
          key={item.id}
          ref={(el) => {
            if (el) rows.current.set(item.id, el);
            else rows.current.delete(item.id);
          }}
          // #19: rows are focusable; keys act only on the row itself, never inside its fields
          // or buttons.
          tabIndex={0}
          onKeyDown={(e) => {
            if (e.target !== e.currentTarget || e.metaKey || e.ctrlKey || e.altKey) return;
            const at = items.findIndex((i) => i.id === item.id);
            const focusRow = (i: number) => {
              const row = items[i] && rows.current.get(items[i].id);
              row?.focus();
              return Boolean(row);
            };
            // The nearest row in `step`'s direction that isn't being removed.
            const focusStaying = (step: number) => {
              for (let i = at + step; i >= 0 && i < items.length; i += step) {
                if (!leaving.current.has(items[i].id)) return focusRow(i);
              }
              return false;
            };
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              focusStaying(e.key === "ArrowDown" ? 1 : -1);
            } else if (e.key === "Delete" || e.key === "Backspace") {
              e.preventDefault();
              if (!removable(item)) return;
              leaving.current.add(item.id);
              // Once the backend answers, the row is gone or, if it couldn't be removed, can be
              // focused again (#19 review).
              void Promise.resolve(onRemove(item.id)).finally(() => leaving.current.delete(item.id));
              if (!focusStaying(1) && !focusStaying(-1)) onLeave?.();
            } else if (e.key === "Escape") {
              e.preventDefault();
              onLeave?.();
            }
          }}
          data-state={item.status.state}
          data-highlight={item.id === highlight ? "" : undefined}
          className={`flex items-center gap-3 rounded py-2 text-sm outline-none transition-colors focus-visible:ring-2 focus-visible:ring-neutral-400 ${item.id === highlight ? "bg-amber-100 dark:bg-amber-900/40" : ""}`}
        >
          <Thumbnail src={item.meta?.thumbnail ?? null} />
          <div className="min-w-0 flex-1">
            {item.meta?.title && (
              <div data-role="title" className="truncate font-medium text-neutral-900 dark:text-neutral-100" title={item.meta.title}>
                {item.meta.title}
              </div>
            )}
            <NameField item={item} onRename={onRename} />
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
          {removable(item) && (
            <button
              type="button"
              onClick={() => onRemove(item.id)}
              aria-label="Remove"
              title={`Remove from queue (${REMOVE_SHORTCUT})`}
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
