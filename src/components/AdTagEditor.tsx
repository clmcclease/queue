import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";
import { AD_TAG_MAX_CHARS, addAdTag, adTags, editAdTag, removeAdTag, resetAdTags, type AdTagList } from "../settings";

/** A reply that is a tag list, or `null` (anything else is ignored). */
function tagList(reply: unknown): AdTagList | null {
  const r = reply as Partial<AdTagList> | null;
  return r &&
    typeof r.version === "number" &&
    Array.isArray(r.tags) &&
    r.tags.every((t) => typeof t === "string")
    ? { version: r.version, tags: r.tags }
    : null;
}

/**
 * The newest tag list the window has seen (#104, D-041). `App` holds it, so a reply that arrives
 * after Settings closed still reaches a reopened editor; `show` ignores a reply older than the
 * list shown. Stable across renders.
 */
export function useAdTagList(): [AdTagList | null, (reply: AdTagList) => void] {
  const [list, setList] = useState<AdTagList | null>(null);
  const shown = useRef(-1);
  const show = useCallback((reply: AdTagList) => {
    if (reply.version < shown.current) return;
    shown.current = reply.version;
    setList(reply);
  }, []);
  return [list, show];
}

interface Props {
  /** The window's newest list and how to hand it a reply (`useAdTagList`); without them the editor keeps its own. */
  list?: AdTagList | null;
  onList?: (reply: AdTagList) => void;
}

/**
 * The advertising tags taken out of AI names (N9, #104, D-036, D-041): a phrase such as
 * "Official Video" is removed only where it fills a whole bracket. Users add, edit and remove
 * phrases, or reset to the starter list. Each change names its phrase and the backend applies it
 * to the list as saved.
 *
 * A change starts only from Enter in a phrase field, a row's ✓ or ✕, the Add form, or Reset:
 * never from focus moving, so nothing depends on the order a webview delivers blur and click
 * (architect, D-028). An edit stays in its field, unsaved, until Enter or ✓; it's dropped when its
 * phrase leaves the list, on Reset, and when Settings closes. One change runs at a time and the
 * editor is disabled meanwhile. Collapsed by default: the list is long.
 */
export default function AdTagEditor({ list: shared, onList }: Props) {
  const [own, showOwn] = useAdTagList();
  const list = onList ? (shared ?? null) : own;
  const show = onList ?? showOwn;
  /** Unsaved edits, by the phrase edited. A Map: a phrase can be any text, "constructor" too. */
  const [drafts, setDrafts] = useState<Map<string, string>>(() => new Map());
  const [adding, setAdding] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const running = useRef(false);

  useEffect(() => {
    let cancelled = false;
    adTags().then(
      (reply) => {
        const loaded = tagList(reply);
        if (loaded) show(loaded);
      },
      (e) => !cancelled && setError(String(e)),
    );
    return () => {
      cancelled = true;
    };
  }, [show]);

  // An edit of a phrase that's gone (removed, renamed, reset) goes with it.
  useEffect(() => {
    if (!list) return;
    setDrafts((d) => {
      const kept = new Map([...d].filter(([phrase]) => list.tags.includes(phrase)));
      return kept.size === d.size ? d : kept;
    });
  }, [list]);

  /** Runs one change; resolves to whether it saved. A failed change leaves the list as it was. */
  async function run(change: () => Promise<unknown>): Promise<boolean> {
    if (running.current) return false;
    running.current = true;
    setBusy(true);
    try {
      const reply = tagList(await change());
      if (reply === null) throw new Error("Queue couldn't read the saved list. Reopen Settings.");
      show(reply);
      setError(null);
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    } finally {
      running.current = false;
      setBusy(false);
    }
  }

  function draft(phrase: string, text: string) {
    setDrafts((d) => {
      const next = new Map(d);
      if (text === phrase) next.delete(phrase);
      else next.set(phrase, text);
      return next;
    });
  }

  async function save(phrase: string) {
    const text = drafts.get(phrase);
    if (text === undefined) return;
    if (text.trim() === phrase) return draft(phrase, phrase);
    // An edit cleared to nothing removes the phrase; a refused edit stays, to retry.
    if (await run(() => editAdTag(phrase, text.trim()))) draft(phrase, phrase);
  }

  function onKey(phrase: string, e: KeyboardEvent<HTMLInputElement>) {
    // Enter while an input method is composing (Japanese, Chinese, Korean…) picks the
    // candidate; it doesn't save.
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    if (e.key === "Enter") {
      e.preventDefault();
      void save(phrase);
    }
  }

  async function add() {
    const text = adding.trim();
    if (text && (await run(() => addAdTag(text)))) setAdding("");
  }

  async function reset() {
    if (await run(resetAdTags)) setDrafts(new Map());
  }

  const rows = list?.tags ?? null;
  const disabled = list === null || busy;

  return (
    <details data-role="ad-tags" className="mt-4 text-sm">
      <summary className="cursor-pointer text-neutral-900 dark:text-neutral-100">
        Advertising removed from AI names{rows ? ` (${rows.length})` : ""}
      </summary>
      <p className="mt-1 text-xs text-neutral-500 dark:text-neutral-400">
        A phrase is removed only when it fills a whole bracket, like “(Official Video)” or “[4K]”, in any letter case. Press
        Enter or ✓ to save an edit.
      </p>
      <ul className="mt-2 flex flex-col gap-1" aria-label="Advertising phrases">
        {rows?.map((phrase, i) => {
          const edit = drafts.get(phrase);
          return (
            <li key={phrase} className="flex items-center gap-2">
              <input
                type="text"
                aria-label={`Phrase ${i + 1}`}
                value={edit ?? phrase}
                maxLength={AD_TAG_MAX_CHARS}
                disabled={disabled}
                onChange={(e) => draft(phrase, e.target.value)}
                onKeyDown={(e) => onKey(phrase, e)}
                className="min-w-0 flex-1 rounded-md border border-neutral-300 bg-transparent px-2 py-1 text-sm dark:border-neutral-700"
              />
              {edit !== undefined && (
                <button
                  type="button"
                  aria-label={`Save ${phrase}`}
                  data-role="ad-tag-save"
                  disabled={disabled}
                  onClick={() => void save(phrase)}
                  className="rounded-md px-2 py-1 text-neutral-500 hover:bg-neutral-100 hover:text-neutral-900 disabled:opacity-50 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-neutral-100"
                >
                  ✓
                </button>
              )}
              <button
                type="button"
                aria-label={`Remove ${phrase}`}
                disabled={disabled}
                onClick={() => void run(() => removeAdTag(phrase))}
                className="rounded-md px-2 py-1 text-neutral-500 hover:bg-neutral-100 hover:text-neutral-900 disabled:opacity-50 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-neutral-100"
              >
                ✕
              </button>
            </li>
          );
        })}
      </ul>
      {rows?.length === 0 && (
        <p data-role="ad-tags-empty" className="mt-1 text-xs text-neutral-500 dark:text-neutral-400">
          No phrases: AI names are kept as they are.
        </p>
      )}
      <form
        className="mt-2 flex items-center gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          void add();
        }}
      >
        <input
          type="text"
          aria-label="New phrase"
          placeholder="Add a phrase, e.g. Official Video"
          value={adding}
          maxLength={AD_TAG_MAX_CHARS}
          disabled={disabled}
          onChange={(e) => setAdding(e.target.value)}
          className="min-w-0 flex-1 rounded-md border border-neutral-300 bg-transparent px-2 py-1 text-sm dark:border-neutral-700"
        />
        <button
          type="submit"
          disabled={disabled || !adding.trim()}
          className="rounded-md border border-neutral-300 px-2 py-1 disabled:opacity-50 dark:border-neutral-700"
        >
          Add
        </button>
      </form>
      <button
        type="button"
        data-role="ad-tags-reset"
        disabled={disabled}
        onClick={() => void reset()}
        className="mt-2 text-xs text-neutral-500 underline hover:text-neutral-900 disabled:opacity-50 dark:text-neutral-400 dark:hover:text-neutral-100"
      >
        Reset to the default list
      </button>
      {error && (
        <p role="alert" data-role="ad-tags-error" className="mt-1 text-xs text-red-600 dark:text-red-400">
          {error}
        </p>
      )}
    </details>
  );
}
