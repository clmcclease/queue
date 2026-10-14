import { useDownloadFolder, type DownloadFolderState } from "../useDownloadFolder";

interface Props {
  /** Shared state (App passes one, so every copy agrees); without it, the picker keeps its own. */
  state?: DownloadFolderState;
  /** Text before the path. */
  label?: string;
}

/** "Saving to <folder> · Change…" (D1). The folder is remembered by the backend. */
export default function FolderPicker({ state, label = "Saving to" }: Props) {
  const own = useDownloadFolder(!state);
  const { folder, error, choosing, change } = state ?? own;
  const problem = error ?? folder?.error ?? null;
  return (
    <div className="flex min-w-0 flex-1 flex-col text-xs">
      <div className="flex min-w-0 items-center gap-2 text-neutral-500 dark:text-neutral-400">
        <span className="shrink-0">{label}</span>
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
