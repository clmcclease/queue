// Per-OS wording and shortcuts. Never say "Finder" outside macOS (AGENTS.md).
export type Os = "macos" | "windows" | "linux";

/** The OS from `navigator.platform` (or the user agent): "MacIntel", "Win32", "Linux x86_64". */
export function detectOs(platform: string): Os {
  if (/Mac/.test(platform)) return "macos";
  if (/Win/.test(platform)) return "windows";
  return "linux";
}

/** What each OS calls things. */
export function wordingFor(os: Os) {
  return {
    /** Settings opens with ⌘, on macOS and Ctrl+, elsewhere. */
    settingsShortcut: os === "macos" ? "⌘," : "Ctrl+,",
    /** Download all (#19). */
    downloadAllShortcut: os === "macos" ? "⌘↩" : "Ctrl+Enter",
    /** Removes the focused row (#19); a Mac keyboard's Delete key is Backspace. */
    removeShortcut: os === "macos" ? "⌫" : "Delete",
    /** Follows "in": "in Finder", "in File Explorer", "in your file manager". */
    fileManager: { macos: "Finder", windows: "File Explorer", linux: "your file manager" }[os],
    /** The label for revealing a finished file (D7). */
    showFile: { macos: "Show in Finder", windows: "Show in File Explorer", linux: "Show in folder" }[os],
  };
}

export const os: Os = detectOs(typeof navigator === "undefined" ? "" : navigator.platform || navigator.userAgent);
const wording = wordingFor(os);
export const SETTINGS_SHORTCUT = wording.settingsShortcut;
export const DOWNLOAD_ALL_SHORTCUT = wording.downloadAllShortcut;
export const REMOVE_SHORTCUT = wording.removeShortcut;
export const FILE_MANAGER = wording.fileManager;
export const SHOW_FILE = wording.showFile;
