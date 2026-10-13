// Client for the user's settings (src-tauri/src/settings.rs and the settings_* commands in lib.rs).
import { invoke } from "@tauri-apps/api/core";

/** Where finished downloads go (D1). `error` says why it can't be used now; `path` still names it. */
export interface DownloadFolder {
  path: string;
  error: string | null;
}

export const downloadFolder = () => invoke<DownloadFolder>("settings_download_folder");
/** Opens the OS folder picker; resolves null if it was cancelled. */
export const chooseDownloadFolder = () => invoke<DownloadFolder | null>("settings_choose_download_folder");
