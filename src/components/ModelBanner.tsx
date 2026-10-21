import { useState } from "react";
import { cancelModel, chosenModel, downloadModel, gb, percent, type ModelStatus, type Track } from "../models";

const button =
  "ml-2 rounded-md border border-neutral-300 px-2 py-0.5 text-xs text-neutral-800 disabled:opacity-40 dark:border-neutral-700 dark:text-neutral-200";

/**
 * The local model's banner (N7, #31; PRD §6.3): why names come from the rules, and the one
 * place besides Settings to download the model. The Download click is the consent, so the size
 * and the source are in the same sentence. Non-blocking: the queue works the whole time.
 */
export default function ModelBanner({
  status,
  track,
  onSettings,
}: {
  status: ModelStatus | null;
  track: Track;
  onSettings: () => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
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
    }
  }

  const settings = (
    <button type="button" data-role="model-settings" className={button} onClick={onSettings}>
      Settings…
    </button>
  );
  const download = (label: string) =>
    m && (
      <button type="button" data-role="model-download" className={button} disabled={busy} onClick={() => run(() => downloadModel(m.id))}>
        {label}
      </button>
    );

  let text;
  let actions;
  if (!m) {
    text = "AI naming uses a local model that isn't downloaded yet, so names come from the rules for now.";
  } else if (m.downloading && m.verifying) {
    text = "Checking the download…";
  } else if (m.downloading) {
    text = `Downloading the local model… ${percent(m)}% (${gb(m.received ?? 0)} of ${gb(m.size)})`;
    actions = (
      <button type="button" data-role="model-cancel" className={button} disabled={busy} onClick={() => run(cancelModel)}>
        Cancel
      </button>
    );
  } else if (m.error) {
    text = m.error;
    actions = download("Try again");
  } else if (m.state === "partial") {
    text = `The model download is paused at ${percent(m)}%.`;
    actions = download("Resume");
  } else if (m.state === "unverified") {
    text = "A file with the model's name is in the models folder, but Queue didn't download it, so it isn't used.";
    actions = settings;
  } else if (m.state === "ready") {
    text = "The local model is downloaded. This version of Queue can't run it yet, so names come from the rules.";
  } else {
    text = `AI naming uses a local model that isn't downloaded yet (${m.name}, ${gb(m.size)} from huggingface.co). Names come from the rules for now.`;
    actions = (
      <>
        {download("Download")}
        {settings}
      </>
    );
  }

  return (
    <div role="status" data-role="naming-banner" className="mt-2 text-sm text-neutral-600 dark:text-neutral-300">
      <span data-role="model-banner-text">{text}</span>
      {actions}
      {error && (
        <span role="alert" data-role="model-error" className="ml-2 text-red-600 dark:text-red-400">
          {error}
        </span>
      )}
    </div>
  );
}
