import { opendir } from "node:fs/promises";
import { homedir } from "node:os";
import path from "node:path";

export interface CodexHomeDiscoveryOptions {
  env?: Readonly<Record<string, string | undefined>>;
  homeDirectory?: string;
  currentDirectory?: string;
}

export interface NormalizedProjectPath {
  path: string | null;
  key: string;
  label: string;
}

const UNKNOWN_PROJECT_KEY = "__unknown_project__";
const UNKNOWN_PROJECT_LABEL = "Unknown project";

export function discoverCodexHome(options: CodexHomeDiscoveryOptions = {}): string {
  const env = options.env ?? process.env;
  const configured = env.CODEX_HOME?.trim();
  const candidate = configured || path.join(options.homeDirectory ?? homedir(), ".codex");
  return path.resolve(options.currentDirectory ?? process.cwd(), candidate);
}

export function getRolloutSessionsDirectory(codexHome: string): string {
  return path.join(codexHome, "sessions");
}

export async function listRolloutFiles(codexHome: string): Promise<string[]> {
  const files: string[] = [];

  async function visit(directory: string): Promise<void> {
    let entries;
    try {
      entries = await opendir(directory);
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code === "ENOENT" || code === "ENOTDIR") return;
      throw error;
    }

    for await (const entry of entries) {
      const entryPath = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        await visit(entryPath);
      } else if (
        entry.isFile() &&
        entry.name.startsWith("rollout-") &&
        (entry.name.endsWith(".jsonl") || entry.name.endsWith(".jsonl.zst"))
      ) {
        files.push(entryPath);
      }
    }
  }

  await visit(getRolloutSessionsDirectory(codexHome));
  await visit(path.join(codexHome, "archived_sessions"));
  return files.sort((left, right) => left.localeCompare(right));
}

export function rolloutKeyFromPath(filePath: string): string {
  const baseName = path.basename(filePath).replace(/\.jsonl(?:\.zst)?$/i, "");
  const uuid = baseName.match(
    /([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/i,
  );
  return uuid?.[1]?.toLowerCase() ?? baseName;
}

export function normalizeProjectPath(value: unknown): NormalizedProjectPath {
  if (typeof value !== "string" || value.trim() === "") {
    return {
      path: null,
      key: UNKNOWN_PROJECT_KEY,
      label: UNKNOWN_PROJECT_LABEL,
    };
  }

  let raw = value.trim();
  if (raw.startsWith("\\\\?\\UNC\\")) raw = `\\\\${raw.slice(8)}`;
  else if (raw.startsWith("\\\\?\\")) raw = raw.slice(4);

  const windowsLike = /^[a-z]:[\\/]/i.test(raw) || raw.startsWith("\\\\");
  const pathApi = windowsLike ? path.win32 : path.posix;
  let normalized = pathApi.normalize(raw);

  if (windowsLike && /^[a-z]:/i.test(normalized)) {
    normalized = `${normalized[0]?.toUpperCase()}${normalized.slice(1)}`;
  }

  const parsed = pathApi.parse(normalized);
  while (normalized.length > parsed.root.length && /[\\/]$/.test(normalized)) {
    normalized = normalized.slice(0, -1);
  }

  const label = pathApi.basename(normalized) || normalized;
  const key = windowsLike ? normalized.toLocaleLowerCase("en-US") : normalized;
  return { path: normalized, key, label };
}
