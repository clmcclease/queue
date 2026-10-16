import { useEffect, useRef } from "react";
import type { DownloadFolderState } from "../useDownloadFolder";
import FolderPicker from "./FolderPicker";
import FormatPicker from "./FormatPicker";

interface Props {
  folder: DownloadFolderState;
  onClose: () => void;
}

/**
 * Settings (PRD §6.4, #36): a panel over the queue, in the app's single window. Escape or ✕
 * closes it. Later items add their sections here (naming #17, AI #25/#31/#32,
 * yt-dlp updates #21, notifications #20). Every change is saved by the backend as it's made.
 */
export default function SettingsPanel({ folder, onClose }: Props) {
  const close = useRef<HTMLButtonElement>(null);

  // Focus moves into the panel; App puts it back on the opener once the queue isn't inert.
  useEffect(() => close.current?.focus(), []);

  // Escape closes Settings wherever focus is (a click on the panel's background leaves it on
  // the page body), with or without ⌘/Ctrl held.
  const latestClose = useRef(onClose);
  latestClose.current = onClose;
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        latestClose.current();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="settings-title"
      data-role="settings"
      className="absolute inset-0 z-10 flex flex-col bg-white p-4 dark:bg-neutral-900"
    >
      <div className="flex items-center justify-between">
        <h2 id="settings-title" className="text-base font-semibold text-neutral-900 dark:text-neutral-100">
          Settings
        </h2>
        <button
          ref={close}
          type="button"
          onClick={onClose}
          aria-label="Close settings"
          className="rounded-md px-2 py-1 text-sm text-neutral-500 hover:bg-neutral-100 hover:text-neutral-900 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-neutral-100"
        >
          ✕
        </button>
      </div>
      <section aria-labelledby="settings-downloads" className="mt-4">
        <h3 id="settings-downloads" className="text-xs font-semibold uppercase tracking-wide text-neutral-500 dark:text-neutral-400">
          Downloads
        </h3>
        <div className="mt-2 flex">
          <FolderPicker state={folder} label="Download folder" />
        </div>
      </section>
      <section aria-labelledby="settings-format" className="mt-6">
        <h3 id="settings-format" className="text-xs font-semibold uppercase tracking-wide text-neutral-500 dark:text-neutral-400">
          Format
        </h3>
        <FormatPicker />
      </section>
    </div>
  );
}
