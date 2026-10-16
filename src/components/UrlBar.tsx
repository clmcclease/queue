import { useRef, useState, type ClipboardEvent, type FormEvent } from "react";
import { splitLinks } from "../queue";

interface Props {
  /** Resolves true when the item was added, so the field can clear. */
  onAdd: (url: string) => Promise<boolean>;
  /** Q8: several links pasted at once, one per line (the field keeps what it had). */
  onAddMany: (urls: string[]) => Promise<void>;
}

export default function UrlBar({ onAdd, onAddMany }: Props) {
  const [value, setValue] = useState("");
  const input = useRef<HTMLInputElement>(null);
  const empty = value.trim() === "";

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (empty) return;
    const submitted = value;
    // Clicking + moves focus to the button; return it now so a paste during the add lands here.
    input.current?.focus();
    // Only clear if the field still holds what was submitted: the user may have typed the next link.
    if (await onAdd(submitted)) setValue((current) => (current === submitted ? "" : current));
  }

  // A one-line field would join pasted lines into one bad link, so a multi-line paste becomes one
  // row per line instead (Q8). A single line pastes as usual.
  function paste(event: ClipboardEvent<HTMLInputElement>) {
    const lines = splitLinks(event.clipboardData.getData("text/plain"));
    if (!lines) return;
    event.preventDefault();
    void onAddMany(lines);
  }

  return (
    <form onSubmit={submit} className="flex gap-2">
      <input
        ref={input}
        autoFocus
        type="text"
        inputMode="url"
        autoComplete="off"
        spellCheck={false}
        aria-label="Link to download"
        placeholder="Paste a link (or several, one per line)"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onPaste={paste}
        className="min-w-0 flex-1 rounded-md border border-neutral-300 bg-white px-3 py-2 text-sm text-neutral-900 outline-none focus:border-neutral-500 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100"
      />
      <button
        type="submit"
        disabled={empty}
        aria-label="Add to queue"
        title="Add to queue"
        className="rounded-md bg-neutral-900 px-4 text-lg font-medium text-white disabled:opacity-40 dark:bg-neutral-100 dark:text-neutral-900"
      >
        +
      </button>
    </form>
  );
}
