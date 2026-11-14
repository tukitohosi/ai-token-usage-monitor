import { invoke, isTauri } from "@tauri-apps/api/core";
import type { PricingSettings } from "../core/types";
import { MAX_PRICING_FILE_BYTES, normalizeCliRules, normalizePricingTransfer, parsePricingTransfer, type PricingTransferDocument, type PricingExportResult } from "../core/pricing-transfer";

export async function openPricingImport(): Promise<PricingTransferDocument | null> {
  if (isTauri()) {
    const value = await invoke<unknown>("import_pricing_file");
    return value == null ? null : normalizePricingTransfer(value);
  }
  return new Promise((resolve, reject) => {
    const input = document.createElement("input"); input.type = "file"; input.accept = ".json,application/json";
    input.addEventListener("cancel", () => resolve(null), { once: true });
    input.addEventListener("change", () => {
      const file = input.files?.[0];
      if (!file) { resolve(null); return; }
      if (file.size > MAX_PRICING_FILE_BYTES) { reject(new Error("定价文件不能超过 1 MiB。")); return; }
      file.text().then(text => resolve(parsePricingTransfer(text))).catch(reject);
    }, { once: true });
    input.click();
  });
}

export async function readCliLongContextRules() {
  if (!isTauri()) throw new Error("请在桌面应用中读取本机 Claude Code 规则，或在模型下方手动填写。");
  return normalizeCliRules(await invoke<unknown>("read_claude_long_context_rules"));
}

export async function exportSavedPricing(saved?: PricingSettings): Promise<PricingExportResult> {
  if (isTauri()) return invoke<PricingExportResult>("export_pricing_file");
  if (!saved) throw new Error("当前环境无法读取已保存的模型定价。");
  const document = normalizePricingTransfer({ format: "ai-token-pricing", version: 2, unit: "perMillionTokens", exportedAt: new Date().toISOString(), savedAt: saved.updatedAt, peak: saved.peak, models: saved.models.map(model => ({ ...model, longContext: model.longContext ?? null })) });
  const blob = new Blob([JSON.stringify(document, null, 2)], { type: "application/json;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const link = window.document.createElement("a");
  link.href = url; link.download = `AI-Token-模型定价-${new Date().toLocaleDateString("sv-SE")}.json`; link.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 1_000);
  return { status: "saved", path: null };
}
