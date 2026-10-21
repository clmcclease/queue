import { useEffect, useState } from "react";
import ApiSetup from "./ApiSetup";
import ModelManager from "./ModelManager";
import type { ModelStatus, Track } from "../models";
import { PROVIDERS, namingProvider, setNamingProvider, type NamingStatus, type Provider } from "../settings";

/**
 * Who suggests names (N4, #25): Local (default), API or Off, saved by the backend as it
 * changes; choosing API opens its setup (#32) below it, and Local its model (#31). `status` is the main window's (so a saved API choice still says it isn't set up when
 * Settings reopens); `onStatus` gets the new one after a save, so the banner and this note follow.
 */
export default function ProviderPicker({
  status,
  onStatus,
  models,
  onModels,
}: {
  status?: NamingStatus | null;
  onStatus?: (status: NamingStatus) => void;
  models?: ModelStatus | null;
  onModels?: Track;
}) {
  const [provider, setShown] = useState<Provider | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    namingProvider().then(
      (p) => !cancelled && setShown((shown) => shown ?? p),
      (e) => !cancelled && setError(String(e)),
    );
    return () => {
      cancelled = true;
    };
  }, []);

  async function choose(next: Provider) {
    if (next === provider || saving) return;
    const previous = provider;
    setShown(next); // show it at once; put it back if saving fails
    setSaving(true);
    try {
      const saved = await setNamingProvider(next);
      onStatus?.(saved);
      setError(null);
    } catch (e) {
      setShown(previous);
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div role="radiogroup" aria-labelledby="settings-ai" data-role="provider" className="mt-2 flex flex-col gap-2">
      {PROVIDERS.map((p) => (
        <label key={p.value} className="flex cursor-pointer items-start gap-2 text-sm">
          <input
            type="radio"
            name="provider"
            value={p.value}
            checked={provider === p.value}
            disabled={provider === null || saving}
            onChange={() => choose(p.value)}
            className="mt-1"
          />
          <span>
            <span className="text-neutral-900 dark:text-neutral-100">{p.label}</span>
            <span className="block text-xs text-neutral-500 dark:text-neutral-400">{p.detail}</span>
            {provider === "api" && p.value === "api" && status?.unavailable === "not_set_up" && (
              <span data-role="provider-note" className="block text-xs text-amber-700 dark:text-amber-400">
                Not set up yet. Names come from the rules until it is.
              </span>
            )}
            {provider === "api" && p.value === "api" && status?.unavailable === "store_unreachable" && (
              <span data-role="provider-note" className="block text-xs text-amber-700 dark:text-amber-400">
                Couldn't reach your system's credential store. Names come from the rules for now.
              </span>
            )}
          </span>
        </label>
      ))}
      {provider === "api" && <ApiSetup onStatus={onStatus} />}
      {provider === "local" && onModels && <ModelManager status={models ?? null} track={onModels} />}
      {error && (
        <p role="alert" data-role="provider-error" className="text-xs text-red-600 dark:text-red-400">
          {error}
        </p>
      )}
    </div>
  );
}
