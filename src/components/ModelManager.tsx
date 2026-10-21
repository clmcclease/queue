import { useState } from "react";
import {
  cancelModel,
  chosenModel,
  deleteModel,
  downloadModel,
  gb,
  percent,
  setLocalModel,
  type ModelId,
  type ModelInfo,
  type ModelStatus,
  type Track,
} from "../models";

const button =
  "shrink-0 rounded-md border border-neutral-300 px-3 py-1 text-xs text-neutral-800 disabled:opacity-40 dark:border-neutral-700 dark:text-neutral-200";

function describe(m: ModelInfo) {
  if (m.downloading && m.verifying) return "Checking the download…";
  if (m.downloading) return `Downloading… ${percent(m)}% (${gb(m.received ?? 0)} of ${gb(m.size)})`;
  if (m.state === "ready") return "Downloaded.";
  if (m.state === "partial") return `Paused at ${percent(m)}%.`;
  if (m.state === "unverified") return "A file with this name is there but wasn't verified by Queue, so it isn't used.";
  return "Not downloaded.";
}

/**
 * The local model (N7, #31), under the Local provider: which model, its size, where it's saved
 * and where it comes from, and Download / Cancel / Resume / Delete. Delete asks once more.
 */
export default function ModelManager({
  status,
  track,
}: {
  status: ModelStatus | null;
  track: Track;
}) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState<ModelId | null>(null);
  const m = chosenModel(status);

  async function run(action: () => Promise<ModelStatus>) {
    setBusy(true);
    try {
      await track(action);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
      setConfirming(null);
    }
  }

  if (!status || !m) {
    return error ? <p role="alert" className="text-xs text-red-600 dark:text-red-400">{error}</p> : null;
  }
  const sep = status.dir.includes("\\") ? "\\" : "/";
  const other = status.models.find((x) => x.id !== m.id && x.downloading);

  return (
    <div data-role="model-manager" className="mt-2 flex flex-col gap-2 border-l-2 border-neutral-200 pl-3 text-xs dark:border-neutral-700">
      <div role="radiogroup" aria-label="Local model" className="flex flex-col gap-1">
        {status.models.map((x) => (
          <label key={x.id} className="flex cursor-pointer items-start gap-2">
            <input
              type="radio"
              name="local-model"
              value={x.id}
              checked={x.id === status.chosen}
              disabled={busy}
              onChange={() => run(() => setLocalModel(x.id))}
              className="mt-0.5"
            />
            <span>
              <span className="text-neutral-900 dark:text-neutral-100">
                {x.name}, {gb(x.size)}
              </span>
              {status.suggested === x.id && (
                <span data-role="model-suggested" className="block text-neutral-500 dark:text-neutral-400">
                  {x.id === "qwen3_5_2b"
                    ? "Suggested for this computer: no GPU, or under 8 GB of memory."
                    : "Suggested for this computer."}
                </span>
              )}
            </span>
          </label>
        ))}
      </div>
      <p data-role="model-state" className="text-neutral-700 dark:text-neutral-300">
        {m.error ?? describe(m)}
      </p>
      <p className="text-neutral-500 dark:text-neutral-400">
        Saved in <span data-role="model-path" className="break-all font-mono">{`${status.dir}${sep}${m.file}`}</span>
        <br />
        From <span data-role="model-source">{m.source}</span>
      </p>
      {other && <p data-role="model-other">Downloading {other.name}…</p>}
      <div className="flex flex-wrap items-center gap-2">
        {m.downloading || other ? (
          <button type="button" data-role="model-cancel" className={button} disabled={busy} onClick={() => run(cancelModel)}>
            Cancel download
          </button>
        ) : (
          m.state !== "ready" &&
          m.state !== "unverified" && (
            <button type="button" data-role="model-download" className={button} disabled={busy} onClick={() => run(() => downloadModel(m.id))}>
              {m.state === "partial" || m.error ? "Resume" : `Download (${gb(m.size)})`}
            </button>
          )
        )}
        {m.state !== "missing" &&
          !m.downloading &&
          (confirming === m.id ? (
            <>
              <span>Delete {gb(m.size)}?</span>
              <button type="button" data-role="model-delete-confirm" className={button} disabled={busy} onClick={() => run(() => deleteModel(m.id))}>
                Delete
              </button>
              <button type="button" data-role="model-delete-keep" className={button} onClick={() => setConfirming(null)}>
                Keep
              </button>
            </>
          ) : (
            <button type="button" data-role="model-delete" className={button} disabled={busy} onClick={() => setConfirming(m.id)}>
              Delete
            </button>
          ))}
      </div>
      {error && (
        <p role="alert" data-role="model-error" className="text-red-600 dark:text-red-400">
          {error}
        </p>
      )}
    </div>
  );
}
