import { useEffect, useState } from "react";
import { TEMPLATES, namingPreviews, namingTemplate, setNamingTemplate, type Template } from "../settings";

/**
 * The naming template (N3, #17): a radio group, each with a live preview of its name for a sample
 * item, saved by the backend as it changes. Queued rows follow it unless renamed by hand.
 */
export default function TemplatePicker() {
  const [template, setShown] = useState<Template | null>(null);
  const [previews, setPreviews] = useState<Partial<Record<Template, string>>>({});
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    namingTemplate().then(
      (t) => !cancelled && setShown((shown) => shown ?? t),
      (e) => !cancelled && setError(String(e)),
    );
    namingPreviews().then(
      (list) => !cancelled && Array.isArray(list) && setPreviews(Object.fromEntries(list)),
      () => {}, // the choices still work without their previews
    );
    return () => {
      cancelled = true;
    };
  }, []);

  async function choose(next: Template) {
    if (next === template || saving) return;
    const previous = template;
    setShown(next); // show it at once; put it back if saving fails
    setSaving(true);
    try {
      await setNamingTemplate(next);
      setError(null);
    } catch (e) {
      setShown(previous);
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div role="radiogroup" aria-labelledby="settings-naming" data-role="template" className="mt-2 flex flex-col gap-2">
      {TEMPLATES.map((t) => (
        <label key={t.value} className="flex cursor-pointer items-start gap-2 text-sm">
          <input
            type="radio"
            name="template"
            value={t.value}
            checked={template === t.value}
            disabled={template === null || saving}
            onChange={() => choose(t.value)}
            className="mt-1"
          />
          <span>
            <span className="text-neutral-900 dark:text-neutral-100">{t.label}</span>
            {previews[t.value] && (
              <span data-role="preview" className="block font-mono text-xs text-neutral-500 dark:text-neutral-400">
                {previews[t.value]}
              </span>
            )}
          </span>
        </label>
      ))}
      <p className="text-xs text-neutral-500 dark:text-neutral-400">Names you've edited by hand stay as they are.</p>
      {error && (
        <p role="alert" data-role="template-error" className="text-xs text-red-600 dark:text-red-400">
          {error}
        </p>
      )}
    </div>
  );
}
