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
/** Who suggests names (N4, #25); matches src-tauri's `ai::Provider`. */
export type Provider = "local" | "api" | "off";

export const PROVIDERS: { value: Provider; label: string; detail: string }[] = [
  { value: "local", label: "Local (default)", detail: "A small model on this computer. Nothing leaves your machine." },
  { value: "api", label: "API", detail: "A service you set up. The item's title, channel, description and the other details listed below are sent to it." },
  { value: "off", label: "Off", detail: "Names come from the rules only." },
];

/** The chosen provider and why it can't be asked yet (`null` when it can). */
export interface NamingStatus {
  provider: Provider;
  unavailable: "off" | "model_missing" | "runtime_missing" | "not_set_up" | "store_unreachable" | null;
}

export const namingStatus = () => invoke<NamingStatus>("naming_status");
export const namingProvider = () => invoke<Provider>("settings_naming_provider");
/** Saves the provider; queued rows not renamed by hand are asked again if it can be used. */
export const setNamingProvider = (provider: Provider) =>
  invoke<NamingStatus>("settings_set_naming_provider", { provider });

/** The API service's setup (N8, #32). The key never comes back: only whether one is saved. */
export interface ApiSetup {
  kind: "openai_compatible";
  baseUrl: string;
  model: string;
  keySet: boolean;
  /** Why the credential store couldn't be read, if it couldn't. */
  storeError: string | null;
}

/** What the API is sent for each item; matches src-tauri's `ai::SENT`, in order. */
export const SENT = [
  "Title",
  "Channel",
  "Upload date",
  "Artist",
  "Track",
  "Series",
  "Season number",
  "Episode number",
  "Description",
];

export const apiSetup = () => invoke<ApiSetup>("settings_api");
/** Saves the base URL and model; the backend checks the URL (https://, or http:// on this computer). */
export const setApi = (baseUrl: string, model: string) => invoke<ApiSetup>("settings_set_api", { baseUrl, model });
/** Stores the key in the OS credential store. */
export const setApiKey = (key: string) => invoke<ApiSetup>("api_key_set", { key });
export const removeApiKey = () => invoke<ApiSetup>("api_key_remove");
/** Names a sample item with the saved service; the message never has the key or the reply in it. */
export const testConnection = () => invoke<{ ok: boolean; message: string }>("api_test_connection");
