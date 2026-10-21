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

/** What to download (D2, #11, D-015); matches src-tauri's `download::Format`. */
export type Format = "default" | "compatibleVideo" | "audio" | "compatibleAudio";

export const FORMATS: { value: Format; label: string; detail: string }[] = [
  { value: "default", label: "Best quality", detail: "yt-dlp picks the best video and container (often .webm or .mkv)." },
  { value: "compatibleVideo", label: "Most compatible video (MP4)", detail: "Prefers H.264 + AAC .mp4, which plays almost everywhere, even if a sharper format exists." },
  { value: "audio", label: "Audio only", detail: "The best audio, as the site serves it (often .opus or .m4a). Nothing is re-encoded." },
  { value: "compatibleAudio", label: "Most compatible audio (M4A)", detail: "Prefers AAC .m4a when the site has it." },
];

export const formatSetting = () => invoke<Format>("settings_format");
/** Saves the format; downloads already running keep theirs. */
export const setFormat = (format: Format) => invoke<Format>("settings_set_format", { format });

/** How proposed names are built (N3, #17); matches src-tauri's `naming::Template`. */
export type Template = "title" | "channel_title" | "date_title";

export const TEMPLATES: { value: Template; label: string }[] = [
  { value: "title", label: "Title" },
  { value: "channel_title", label: "Channel - Title" },
  { value: "date_title", label: "Upload date Title" },
];

export const namingTemplate = () => invoke<Template>("settings_naming_template");
/** Saves the template; queued rows whose name wasn't edited by hand are renamed (as events). */
export const setNamingTemplate = (template: Template) =>
  invoke<Template>("settings_set_naming_template", { template });
/** Each template's name for a fixed sample item, for the live preview. */
export const namingPreviews = () => invoke<[Template, string][]>("naming_previews");
