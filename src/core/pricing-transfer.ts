import type { ModelPricingRule, PricingSettings } from "./types.js";

type RateField = "inputPerMillion" | "cachedInputPerMillion" | "cacheWritePerMillion" | "outputPerMillion";
type PricingDraft = Omit<PricingSettings, "models" | "peak"> & {
  pendingSpecialDate: string;
  models: Array<Omit<ModelPricingRule, RateField> & Record<RateField, string>>;
  peak: Omit<PricingSettings["peak"], "multiplier"> & { multiplier: string };
};

export const MAX_PRICING_MODELS = 200;
export const MAX_PRICING_FILE_BYTES = 1024 * 1024;

export interface PricingTransferDocumentV1 {
  format: "ai-token-pricing";
  version: 1;
  unit: "perMillionTokens";
  exportedAt: string;
  savedAt?: string | null;
  peak: PricingSettings["peak"];
  models: ModelPricingRule[];
}
export interface PricingExportResult { status: "saved" | "cancelled"; path: string | null }

const identity = (value: string) => value.trim().toLowerCase();
const record = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const validText = (value: unknown): value is string => typeof value === "string" && value.trim() === value && value.length > 0 && [...value].length <= 256;
const validTime = (value: unknown): value is string => typeof value === "string" && /^(?:[01]\d|2[0-3]):[0-5]\d$/.test(value);
const validDate = (value: unknown): value is string => typeof value === "string" && /^[1-9]\d{3}-(?:0[1-9]|1[0-2])-(?:0[1-9]|[12]\d|3[01])$/.test(value)
  && Number.isFinite(Date.parse(`${value}T00:00:00Z`)) && new Date(`${value}T00:00:00Z`).toISOString().slice(0, 10) === value;
const validTimestamp = (value: unknown): value is string => typeof value === "string" && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(value) && Number.isFinite(Date.parse(value));
const rateFields = ["inputPerMillion", "cachedInputPerMillion", "cacheWritePerMillion", "outputPerMillion"] as const;

/** Validate desktop responses as well as files opened in browser previews. */
export function normalizePricingTransfer(value: unknown): PricingTransferDocumentV1 {
  if (!record(value) || value.format !== "ai-token-pricing") throw new Error("这不是 AI Token 用量监控的定价文件。");
  if (value.version !== 1) throw new Error(`暂不支持定价文件版本 ${String(value.version)}，请使用版本 1。`);
  if (value.unit !== "perMillionTokens") throw new Error("定价单位必须为每百万 Token（perMillionTokens）。");
  if (!validTimestamp(value.exportedAt) || !(value.savedAt == null || validTimestamp(value.savedAt))) throw new Error("定价文件中的导出或保存时间无效。");
  if (Object.keys(value).some(key => !["format", "version", "unit", "exportedAt", "savedAt", "peak", "models"].includes(key))) throw new Error("定价文件包含无法识别的字段。");
  const peak = value.peak;
  if (!record(peak) || typeof peak.multiplier !== "number" || !Number.isFinite(peak.multiplier) || peak.multiplier < 1 || peak.multiplier > 100
    || typeof peak.timeZone !== "string" || typeof peak.excludeChinaHolidays !== "boolean" || !Array.isArray(peak.windows) || peak.windows.length > 32
    || !Array.isArray(peak.specialDates) || peak.specialDates.length > 2000 || peak.specialDates.some(date => !validDate(date)) || new Set(peak.specialDates).size !== peak.specialDates.length) {
    throw new Error("高峰定价配置无效，请检查倍率、时段和特殊日期。");
  }
  if (peak.timeZone !== "local") {
    try { new Intl.DateTimeFormat("en-US", { timeZone: peak.timeZone }).format(0); }
    catch { throw new Error("高峰定价的城市时区无效。"); }
  }
  for (const window of peak.windows) {
    if (!record(window) || ![window.weekdayStart, window.weekdayEnd].every(day => typeof day === "number" && Number.isInteger(day) && day >= 1 && day <= 7)
      || !validTime(window.startTime) || !validTime(window.endTime)) throw new Error("高峰定价的星期或时段无效。");
  }
  if (!Array.isArray(value.models) || value.models.length > MAX_PRICING_MODELS) throw new Error("定价文件最多可包含 200 个模型。");
  const ids = new Set<string>();
  for (const model of value.models) {
    if (!record(model) || !validText(model.modelId) || !validText(model.displayName) || !["USD", "CNY"].includes(String(model.currency)) || typeof model.peakEnabled !== "boolean") {
      throw new Error("模型名称、ID、币种或高峰开关无效。");
    }
    const key = identity(model.modelId);
    if (ids.has(key)) throw new Error(`模型 ID 重复（忽略大小写）：${model.modelId}。`);
    ids.add(key);
    for (const field of rateFields) {
      const rate = model[field];
      if (!(rate === null || typeof rate === "number" && Number.isFinite(rate) && rate >= 0 && rate <= 1_000_000)) throw new Error(`模型 ${model.modelId} 的单价必须为 0 到 1,000,000 的数字，或 null。`);
    }
  }
  // Return only serialized data, detached from caller-owned objects.
  return JSON.parse(JSON.stringify(value)) as PricingTransferDocumentV1;
}

export function parsePricingTransfer(text: string): PricingTransferDocumentV1 {
  if (new TextEncoder().encode(text).byteLength > MAX_PRICING_FILE_BYTES) throw new Error("定价文件不能超过 1 MiB。");
  let value: unknown;
  try { value = JSON.parse(text.replace(/^\uFEFF/, "")); } catch { throw new Error("无法解析定价 JSON，请检查文件内容。"); }
  return normalizePricingTransfer(value);
}

export function pricingImportSummary(draft: PricingDraft, document: PricingTransferDocumentV1) {
  const ids = new Set(draft.models.map(model => identity(model.modelId)));
  const added = document.models.filter(model => !ids.has(identity(model.modelId))).length;
  return { added, overwritten: document.models.length - added, total: draft.models.length + added };
}

/** Merge into the latest raw draft; unfinished fields in unrelated rows survive. */
export function mergePricingImport(draft: PricingDraft, input: PricingTransferDocumentV1, includePeak = false): PricingDraft {
  const document = normalizePricingTransfer(input);
  const summary = pricingImportSummary(draft, document);
  if (summary.total > MAX_PRICING_MODELS) throw new Error(`合并后将有 ${summary.total} 个模型（当前 ${draft.models.length} 个，新增 ${summary.added} 个），超过 200 个上限；未导入任何内容。`);
  const imported = new Map(document.models.map(model => [identity(model.modelId), {
    ...model,
    inputPerMillion: model.inputPerMillion === null ? "" : String(model.inputPerMillion),
    cachedInputPerMillion: model.cachedInputPerMillion === null ? "" : String(model.cachedInputPerMillion),
    cacheWritePerMillion: model.cacheWritePerMillion === null ? "" : String(model.cacheWritePerMillion),
    outputPerMillion: model.outputPerMillion === null ? "" : String(model.outputPerMillion),
  }]));
  const models = draft.models.map(local => {
    const key = identity(local.modelId);
    const incoming = imported.get(key);
    if (!incoming) return local;
    imported.delete(key);
    return { ...incoming, modelId: local.modelId, peakEnabled: includePeak ? incoming.peakEnabled : local.peakEnabled };
  });
  for (const incoming of imported.values()) models.push({ ...incoming, peakEnabled: includePeak ? incoming.peakEnabled : false });
  return { ...draft, models, ...(includePeak ? { peak: { ...document.peak, multiplier: String(document.peak.multiplier) }, pendingSpecialDate: "" } : {}) };
}
