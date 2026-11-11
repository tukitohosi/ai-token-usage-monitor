import { convertFileSrc, invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { DEFAULT_PET_STATE, type PetLayout, type PetPreferences, type PetSource, type PetState } from "../core/pet-types";
import type { ThemePreference } from "../core/types";

type Unsubscribe = () => void;

export interface PetApi {
  native: boolean;
  readState(): Promise<PetState>;
  subscribe(listener: (state: PetState) => void): Promise<Unsubscribe>;
  discover(): Promise<PetSource[]>;
  selectDirectory(): Promise<PetSource[] | null>;
  preview(sourcePath: string): Promise<string>;
  importSource(sourcePath: string): Promise<PetState>;
  selectFile(): Promise<PetState | null>;
  setPreferences(preferences: PetPreferences): Promise<PetState>;
  resetPosition(): Promise<PetState>;
  readAppTheme(): Promise<ThemePreference>;
  subscribeAppTheme(listener: (theme: ThemePreference) => void): Promise<Unsubscribe>;
  subscribeLayout?(listener: (layout: PetLayout) => void): Promise<Unsubscribe>;
  setLayout(layout: PetLayout): Promise<PetLayout | void>;
  dragStart(clientX: number, clientY: number): Promise<void>;
  dragUpdate(): Promise<void>;
  dragEnd(): Promise<void>;
  showDashboard(): Promise<void>;
}

export function petAssetUrl(path: string): string {
  if (path.startsWith("data:image/")) return path;
  return isTauri() ? convertFileSrc(path) : "";
}

export const petApi: PetApi = {
  get native() { return isTauri(); },
  async readState() { return isTauri() ? invoke<PetState>("read_pet_state") : structuredClone(DEFAULT_PET_STATE); },
  async subscribe(listener) { return isTauri() ? listen<PetState>("pet://state", event => listener(event.payload)) : () => {}; },
  async discover() { return isTauri() ? invoke<PetSource[]>("discover_codex_pets") : []; },
  async selectDirectory() { return isTauri() ? invoke<PetSource[] | null>("select_codex_pet_directory") : null; },
  async preview(sourcePath) { return isTauri() ? invoke<string>("read_pet_preview", { sourcePath }) : ""; },
  async importSource(sourcePath) { return invoke<PetState>("import_codex_pet", { sourcePath }); },
  async selectFile() { return isTauri() ? invoke<PetState | null>("select_pet_file") : null; },
  async setPreferences(preferences) { return invoke<PetState>("set_pet_preferences", { preferences }); },
  async resetPosition() { return invoke<PetState>("reset_pet_position"); },
  async readAppTheme() {
    if (!isTauri()) return "system";
    const value = await invoke<{ themePreference: ThemePreference }>("read_app_preferences");
    return value.themePreference;
  },
  async subscribeAppTheme(listener) { return isTauri() ? listen<ThemePreference>("pet://app-theme", event => listener(event.payload)) : () => {}; },
  async subscribeLayout(listener) { return isTauri() ? listen<PetLayout>("pet://layout", event => listener(event.payload)) : () => {}; },
  async setLayout(layout) { if (isTauri()) return invoke<PetLayout>("set_pet_layout", { layout }); },
  async dragStart(clientX, clientY) { if (isTauri()) await invoke("pet_drag_start", { clientX, clientY }); },
  async dragUpdate() { if (isTauri()) await invoke("pet_drag_update"); },
  async dragEnd() { if (isTauri()) await invoke("pet_drag_end"); },
  async showDashboard() { if (isTauri()) await invoke("show_pet_dashboard"); },
};
