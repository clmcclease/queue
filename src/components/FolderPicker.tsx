import { useEffect, useRef, useState } from "react";
import { chooseDownloadFolder, downloadFolder, type DownloadFolder } from "../settings";

/** "Saving to <folder> · Change…" (D1). The folder is remembered by the backend. */
export default function FolderPicker() {
  const [folder, setFolder] = useState<DownloadFolder | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [choosing, setChoosing] = useState(false);
  // Once a folder is chosen, the startup read is stale: its late reply or error must not win.
  const chosen = useRef(false);

  useEffect(() => {
    let cancelled = false;
    downloadFolder().then(
      (f) => !cancelled && !chosen.current && setFolder(f),
      (e) => !cancelled && !chosen.current && setError(String(e)),
    );
    return () => {
      cancelled = true;
    };
  }, []);

  async function change() {
    setChoosing(true);
    try {
      const picked = await chooseDownloadFolder();
      if (picked) {
        chosen.current = true;
        setFolder(picked);
        setError(null); // only a pick resolves an earlier error; a cancel leaves it showing
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setChoosing(false);
    }
  }

  const problem = error ?? folder?.error ?? null;
  return (
    <div className="flex min-w-0 flex-1 flex-col text-xs">
      <div className="flex min-w-0 items-center gap-2 text-neutral-500 dark:text-neutral-400">
        <span className="shrink-0">Saving to</span>
        <span data-role="folder" className="truncate font-mono text-neutral-800 dark:text-neutral-200" title={folder?.path}>
          {folder?.path ?? "…"}
        </span>
        <button
          type="button"
          onClick={change}
          disabled={choosing}
          className="shrink-0 rounded px-1.5 py-0.5 text-neutral-700 underline-offset-2 hover:underline disabled:opacity-40 dark:text-neutral-300"
        >
          Change…
        </button>
      </div>
      {problem && (
        <p role="alert" data-role="folder-error" className="text-red-600 dark:text-red-400">
          {problem}
        </p>
      )}
    </div>
  );
}
