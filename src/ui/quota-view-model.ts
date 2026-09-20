import type { NormalizedQuotaWindow } from "../core/types.js";

export interface OverviewQuotaCard {
  quota: NormalizedQuotaWindow;
  title: string;
}

/** Selects known quota identities without depending on server map ordering. */
export function selectOverviewQuotas(windows: NormalizedQuotaWindow[]): OverviewQuotaCard[] {
  const lower = (value: string | null | undefined) => value?.trim().toLocaleLowerCase() ?? "";
  const isCodex = (window: NormalizedQuotaWindow) =>
    lower(window.limitId) === "codex" || lower(window.limitName) === "codex";
  const isLunaReserve = (window: NormalizedQuotaWindow) =>
    lower(window.limitId) === "base_model_inference"
    || lower(window.limitName) === "gpt-reserve"
    || lower(window.normalModelSlug) === "gpt-5.6-luna";
  const codexFiveHour = windows.find((window) => isCodex(window) && window.windowDurationMins === 300);
  const codexSevenDay = windows.find((window) => isCodex(window) && window.windowDurationMins === 10_080);
  const lunaReserve = windows.find((window) => isLunaReserve(window) && window.windowDurationMins === 10_080)
    ?? windows.find(isLunaReserve);
  return [
    codexFiveHour && { quota: codexFiveHour, title: "Codex 5 小时" },
    codexSevenDay && { quota: codexSevenDay, title: "Codex 7 天" },
    lunaReserve && { quota: lunaReserve, title: "GPT-5.6 Luna 储备 7 天" },
  ].filter((item): item is OverviewQuotaCard => Boolean(item));
}
