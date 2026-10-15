import { useEffect, useState } from "react";
import { FORMATS, formatSetting, setFormat, type Format } from "../settings";

/** The download format (D2, #11): a radio group, saved by the backend as it changes. */
export default function FormatPicker() {
  const [format, setShown] = useState<Format | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    formatSetting().then(
      (f) => !cancelled && setShown((shown) => shown ?? f),
      (e) => !cancelled && setError(String(e)),
    );
    return () => {
      cancelled = true;
    };
  }, []);

  async function choose(next: Format) {
    if (next === format || saving) return;
    const previous = format;
    setShown(next); // show it at once; put it back if saving fails
    setSaving(true);
    try {
      await setFormat(next);
      setError(null);
    } catch (e) {
      setShown(previous);
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div role="radiogroup" aria-labelledby="settings-format" data-role="format" className="mt-2 flex flex-col gap-2">
      {FORMATS.map((f) => (
        <label key={f.value} className="flex cursor-pointer items-start gap-2 text-sm">
          <input
            type="radio"
            name="format"
            value={f.value}
            checked={format === f.value}
            disabled={format === null || saving}
            onChange={() => choose(f.value)}
            className="mt-1"
          />
          <span>
            <span className="text-neutral-900 dark:text-neutral-100">{f.label}</span>
            <span className="block text-xs text-neutral-500 dark:text-neutral-400">{f.detail}</span>
          </span>
        </label>
      ))}
      {error && (
        <p role="alert" data-role="format-error" className="text-xs text-red-600 dark:text-red-400">
          {error}
        </p>
      )}
    </div>
  );
}
