import { useEffect, useRef, useState } from "react";
import {
  DEFAULT_BASE_URL,
  SENT,
  apiSetup,
  namingStatus,
  removeApiKey,
  setApi,
  setApiKey,
  testConnection,
  type ApiKind,
  type ApiSetup as Setup,
  type NamingStatus,
} from "../settings";

const field =
  "w-full rounded border border-neutral-300 bg-white px-1.5 py-1 text-xs text-neutral-900 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100";
const button =
  "shrink-0 rounded-md border border-neutral-300 px-3 py-1 text-xs text-neutral-800 disabled:opacity-40 dark:border-neutral-700 dark:text-neutral-200";

/**
 * The API service (N8, #32, #91): OpenAI-compatible or Anthropic, a base URL and model, a key kept in the OS
 * credential store (write-only: it's never shown again), Test connection, and exactly what is
 * sent. `onStatus` gets the naming status after each change, so the banner and notes follow.
 */
export default function ApiSetup({ onStatus }: { onStatus?: (status: NamingStatus) => void }) {
  const [setup, setSetup] = useState<Setup | null>(null);
  const [kind, setKind] = useState<ApiKind>("openai_compatible");
  const [baseUrl, setBaseUrl] = useState("");
  const [model, setModel] = useState("");
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tested, setTested] = useState<{ ok: boolean; message: string } | null>(null);
  // The latest status refresh; 0 once this setup is gone (the provider changed, or Settings
  // closed). An older or orphaned reply must not overwrite a newer status (#32 review).
  const refresh = useRef(0);
  useEffect(() => {
    refresh.current = 1;
    return () => {
      refresh.current = 0;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    apiSetup().then(
      (s) => {
        if (cancelled || !s) return;
        setSetup(s);
        setKind(s.kind);
        // With nothing saved, the service's default address is filled in as a real value (#109): as
        // a placeholder it looked saved, and Queue stayed "Not set up yet".
        setBaseUrl(s.baseUrl || DEFAULT_BASE_URL[s.kind]);
        setModel(s.model);
      },
      (e) => !cancelled && setError(String(e)),
    );
    return () => {
      cancelled = true;
    };
  }, []);

  // Runs one change; on success shows the new setup and refreshes the naming status.
  async function run(change: () => Promise<Setup>, after?: (saved: Setup) => void) {
    setBusy(true);
    try {
      const saved = await change();
      setSetup(saved);
      setError(null);
      setTested(null);
      after?.(saved);
      const ticket = refresh.current > 0 ? ++refresh.current : 0;
      namingStatus().then(
        (s) => ticket > 0 && ticket === refresh.current && onStatus?.(s),
        () => {},
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function test() {
    setBusy(true);
    setTested(null);
    try {
      setTested(await testConnection());
    } catch (e) {
      setTested({ ok: false, message: String(e) });
    } finally {
      setBusy(false);
    }
  }

  const saved = setup !== null && setup.baseUrl !== "" && setup.model !== "";
  // The default filled in on load isn't a change by itself; Save stores it with the model.
  const savedUrl = setup === null ? "" : setup.baseUrl || DEFAULT_BASE_URL[setup.kind];
  const unsaved =
    setup !== null && (kind !== setup.kind || baseUrl.trim() !== savedUrl || model.trim() !== setup.model);
  const anthropic = kind === "anthropic";

  return (
    <div data-role="api-setup" className="mt-2 flex flex-col gap-2 border-l-2 border-neutral-200 pl-3 dark:border-neutral-700">
      <label className="flex flex-col gap-0.5 text-xs text-neutral-600 dark:text-neutral-400">
        Service
        <select
          data-role="api-kind"
          className={field}
          value={kind}
          disabled={setup === null || busy}
          onChange={(e) => {
            // Each service has its own address and model names; nothing is saved until Save.
            const next = e.target.value as ApiKind;
            setKind(next);
            setBaseUrl(DEFAULT_BASE_URL[next]);
            setModel("");
          }}
        >
          <option value="openai_compatible">OpenAI-compatible</option>
          <option value="anthropic">Anthropic</option>
        </select>
      </label>
      <label className="flex flex-col gap-0.5 text-xs text-neutral-600 dark:text-neutral-400">
        Base URL
        <input
          data-role="api-base-url"
          className={field}
          value={baseUrl}
          spellCheck={false}
          disabled={setup === null || busy}
          onChange={(e) => setBaseUrl(e.target.value)}
        />
      </label>
      <label className="flex flex-col gap-0.5 text-xs text-neutral-600 dark:text-neutral-400">
        Model
        <input
          data-role="api-model"
          className={field}
          value={model}
          placeholder={anthropic ? "claude-haiku-5-5" : undefined}
          spellCheck={false}
          disabled={setup === null || busy}
          onChange={(e) => setModel(e.target.value)}
        />
      </label>
      <div>
        <button
          type="button"
          data-role="api-save"
          className={button}
          disabled={setup === null || busy || !unsaved}
          onClick={() =>
            run(
              () => setApi(kind, baseUrl, model),
              // Show what was saved: the backend trims the model and drops a pasted /chat/completions.
              (saved) => {
                setKind(saved.kind);
                setBaseUrl(saved.baseUrl);
                setModel(saved.model);
              },
            )
          }
        >
          Save
        </button>
      </div>
      <label className="flex flex-col gap-0.5 text-xs text-neutral-600 dark:text-neutral-400">
        API key
        <input
          data-role="api-key"
          type="password"
          autoComplete="off"
          className={field}
          value={key}
          placeholder={
            setup?.keySet
              ? "Key saved. Paste a new one to replace it."
              : anthropic
                ? "Required for Anthropic"
                : "Optional for Ollama and LM Studio"
          }
          disabled={setup === null || busy}
          onChange={(e) => setKey(e.target.value)}
        />
      </label>
      <div className="flex items-center gap-2">
        <button
          type="button"
          data-role="api-key-save"
          className={button}
          disabled={setup === null || busy || key.trim() === ""}
          onClick={() => run(() => setApiKey(key), () => setKey(""))}
        >
          Save key
        </button>
        <button
          type="button"
          data-role="api-key-remove"
          className={button}
          disabled={setup === null || busy || !setup.keySet}
          onClick={() => run(removeApiKey)}
        >
          Remove key
        </button>
        <span data-role="api-key-status" className="text-xs text-neutral-500 dark:text-neutral-400">
          {setup === null ? "" : setup.storeError ?? (setup.keySet ? "Key saved" : "No key")}
        </span>
      </div>
      <div className="flex items-center gap-2">
        <button
          type="button"
          data-role="api-test"
          className={button}
          disabled={!saved || unsaved || busy}
          onClick={test}
        >
          Test connection
        </button>
        {tested && (
          <span
            data-role="api-test-result"
            role="status"
            className={`text-xs ${tested.ok ? "text-green-700 dark:text-green-400" : "text-red-600 dark:text-red-400"}`}
          >
            {tested.message}
          </span>
        )}
      </div>
      {error && (
        <p role="alert" data-role="api-error" className="text-xs text-red-600 dark:text-red-400">
          {error}
        </p>
      )}
      <div data-role="api-sent" className="text-xs text-neutral-500 dark:text-neutral-400">
        What is sent for each item:
        <ul className="ml-4 list-disc">
          {SENT.map((f) => (
            <li key={f}>{f}</li>
          ))}
        </ul>
      </div>
    </div>
  );
}
