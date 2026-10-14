import { useEffect, useRef, useState } from "react";
import { chooseDownloadFolder, downloadFolder, type DownloadFolder } from "./settings";

/** The download folder (D1) and how to change it, shared by everything that shows it. */
export interface DownloadFolderState {
  folder: DownloadFolder | null;
  /** Why the folder couldn't be read or changed; the folder's own `error` is separate. */
  error: string | null;
  /** The OS picker is open. */
  choosing: boolean;
  change: () => Promise<void>;
}

/** Reads the folder once (unless `load` is false) and changes it through the OS picker. */
export function useDownloadFolder(load = true): DownloadFolderState {
  const [folder, setFolder] = useState<DownloadFolder | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [choosing, setChoosing] = useState(false);
  // Once a folder is chosen, the startup read is stale: its late reply or error must not win.
  const chosen = useRef(false);

  useEffect(() => {
    if (!load) return;
    let cancelled = false;
    downloadFolder().then(
      (f) => !cancelled && !chosen.current && setFolder(f),
      (e) => !cancelled && !chosen.current && setError(String(e)),
    );
    return () => {
      cancelled = true;
    };
  }, [load]);

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

  return { folder, error, choosing, change };
}
