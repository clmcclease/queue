import { useEffect, useState } from "react";
import { setYtdlpUpdates, ytdlpStatus, type YtdlpStatus } from "../settings";

/** A reply that is a status, or `null`. */
function status(reply: unknown): YtdlpStatus | null {
  const r = reply as Partial<YtdlpStatus> | null;
  return r && typeof r.enabled === "boolean" && typeof r.version === "string" && typeof r.updated === "boolean"
    ? {
        enabled: r.enabled,
        version: r.version,
        updated: r.updated,
        lastCheck: typeof r.lastCheck === "number" ? r.lastCheck : null,
        lastResult: typeof r.lastResult === "string" ? r.lastResult : null,
      }
    : null;
}

/**
 * Keep yt-dlp up to date (#21, D-042): one checkbox and one status line, read when Settings
 * opens. On, Queue checks GitHub at most once a day and uses a newer yt-dlp only after
 * verifying its signature; off, downloads use the copy that came with Queue and nothing is
 * requested.
 */
export default function YtdlpUpdates() {
  const [shown, setShown] = useState<YtdlpStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    ytdlpStatus().then(
      (reply) => !cancelled && setShown(status(reply)),
      (e) => !cancelled && setError(String(e)),
    );
    return () => {
      cancelled = true;
    };
  }, []);

  async function toggle(enabled: boolean) {
    if (saving) return;
    setSaving(true);
    try {
      const reply = status(await setYtdlpUpdates(enabled));
      if (reply === null) throw new Error("Queue couldn't read the yt-dlp setting. Reopen Settings.");
      setShown(reply);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  const checked = shown?.lastCheck ? new Date(shown.lastCheck * 1000).toLocaleString() : null;
  return (
    <div data-role="ytdlp-updates" className="mt-2 text-sm">
      <label className="flex cursor-pointer items-start gap-2">
        <input
          type="checkbox"
          checked={shown?.enabled ?? false}
          disabled={shown === null || saving}
          onChange={(e) => void toggle(e.target.checked)}
          className="mt-1"
        />
        <span>
          <span className="text-neutral-900 dark:text-neutral-100">Keep yt-dlp up to date</span>
          <span className="block text-xs text-neutral-600 dark:text-neutral-400">
            Checks GitHub at most once a day and uses a new version only after verifying its signature.
          </span>
        </span>
      </label>
      {shown && (
        <p data-role="ytdlp-status" className="mt-1 text-xs text-neutral-600 dark:text-neutral-400">
          yt-dlp {shown.version} ({shown.updated ? "updated" : "came with Queue"})
          {checked && ` · Last check ${checked}${shown.lastResult ? `: ${shown.lastResult}` : ""}`}
        </p>
      )}
      {error && (
        <p role="alert" data-role="ytdlp-error" className="mt-1 text-xs text-red-700 dark:text-red-400">
          {error}
        </p>
      )}
    </div>
  );
}
