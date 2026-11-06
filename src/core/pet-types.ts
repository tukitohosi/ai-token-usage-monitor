export type PetTheme = "app" | "light" | "dark";

export interface PetDescriptor {
  id: string;
  displayName: string;
  description: string;
  spriteVersionNumber: 1 | 2;
  /** A copy owned by this application, never the original Codex file. */
  spritesheetPath: string;
}

export interface PetSource extends PetDescriptor {
  sourcePath: string;
}

export interface PetPreferences {
  enabled: boolean;
  selectedPetId: string | null;
  /** Sprite width in logical pixels. */
  size: number;
  theme: PetTheme;
  position: { x: number; y: number } | null;
}

export interface PetState {
  preferences: PetPreferences;
  pets: PetDescriptor[];
}

export interface PetRect { x: number; y: number; width: number; height: number }
export interface PetLayout {
  width: number;
  height: number;
  sprite: PetRect;
  bar: PetRect;
}

export const DEFAULT_PET_STATE: PetState = {
  preferences: { enabled: false, selectedPetId: null, size: 128, theme: "app", position: null },
  pets: [],
};
