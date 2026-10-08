import type { DeviceTokenUsage, TokenCost, UsageSourceSummary } from "./types.js";

export const EXPORT_SOURCES = [
  { id: "codex", label: "Codex" },
  { id: "claude-code", label: "Claude Code" },
  { id: "opencode", label: "OpenCode" },
  { id: "workbuddy", label: "WorkBuddy" },
  { id: "workbuddy-ai", label: "WorkBuddy AI" },
  { id: "cursor", label: "Cursor" },
] as const;

export type ExportSourceId = UsageSourceSummary["id"];
export type UsageExportPreset = "today" | "last7Days" | "last30Days" | "all" | "custom";
export type UsageExportRange = { kind: "all" } | { kind: "dates"; startDate: string; endDate: string };
export interface UsageExportQuery { range: UsageExportRange; sourceIds: ExportSourceId[] }
export interface ExportMetrics { usage: DeviceTokenUsage; cost: TokenCost }
export interface ExportModelRow extends ExportMetrics { sourceId: ExportSourceId; sourceLabel: string; model: string }
export interface ExportDailyRow extends ExportMetrics { date: string | null; sourceId: ExportSourceId; sourceLabel: string }
export interface ExportDailyModelRow extends ExportModelRow { date: string | null }
export interface ExportSourceRow extends ExportMetrics {
  sourceId: ExportSourceId;
  sourceLabel: string;
  status: UsageSourceSummary["status"] | "unknown";
  indexReadStatus: "readable" | "notApplicable";
  lastIndexedAt: string | null;
  collectionStatusObservedAt: string | null;
}
export interface ExportProjectRow extends ExportMetrics { project: string; sourceIds: ExportSourceId[]; sourceLabels: string[]; merged: boolean }
export interface UsageExportReport {
  schemaVersion: 1;
  generatedAt: string;
  query: UsageExportQuery;
  timeZone: "system-local";
  utcOffsetMinutes: number;
  lastCollectionAt: string | null;
  pricingUpdatedAt: string | null;
  summary: ExportMetrics;
  models: ExportModelRow[];
  daily: ExportDailyRow[];
  dailyModels: ExportDailyModelRow[];
  sources: ExportSourceRow[];
  projects: ExportProjectRow[];
}
export type UsageExportDataset = "models" | "daily" | "dailyModels" | "sources" | "projects" | "full";
export type UsageExportFormat = "csv" | "json";
export interface SaveUsageExportRequest { format: UsageExportFormat; suggestedFileName: string; content: string }
export interface SaveUsageExportResult { status: "saved" | "cancelled"; path: string | null }

const usageKeys = ["inputTokens", "cachedInputTokens", "cacheWriteTokens", "cacheWrite5mTokens", "cacheWrite1hTokens", "cacheWriteUnknownTokens", "outputTokens", "reasoningOutputTokens", "totalTokens"] as const;
const costKeys = ["usd", "cny", "unpricedTokens"] as const;
const sourceIds = new Set<string>(EXPORT_SOURCES.map(source => source.id));

export function exportDateKey(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

function validDate(value: string): boolean {
  if (!/^[1-9]\d{3}-\d{2}-\d{2}$/.test(value)) return false;
  const date = new Date(`${value}T12:00:00`);
  return Number.isFinite(date.getTime()) && exportDateKey(date) === value;
}

export function createUsageExportQuery(preset: UsageExportPreset, selectedSources: readonly ExportSourceId[], startDate: string, endDate: string, now = new Date()): UsageExportQuery {
  const selected = EXPORT_SOURCES.filter(source => selectedSources.includes(source.id)).map(source => source.id);
  if (!selected.length) throw new Error("请至少选择一个应用来源。");
  if (preset === "all") return { range: { kind: "all" }, sourceIds: selected };
  if (preset !== "custom") {
    endDate = exportDateKey(now);
    const start = new Date(now);
    start.setDate(start.getDate() - (preset === "last30Days" ? 29 : preset === "last7Days" ? 6 : 0));
    startDate = exportDateKey(start);
  }
  if (!validDate(startDate) || !validDate(endDate)) throw new Error("请选择有效的开始日期和结束日期。");
  if (startDate > endDate) throw new Error("开始日期不能晚于结束日期。");
  return { range: { kind: "dates", startDate, endDate }, sourceIds: selected };
}

export function exportRangeLabel(range: UsageExportRange): string {
  return range.kind === "all" ? "全部历史" : range.startDate === range.endDate ? range.startDate : `${range.startDate} — ${range.endDate}`;
}

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("无法识别导出报表，请重新生成。");
  return value as Record<string, unknown>;
}
function string(value: unknown): string {
  if (typeof value !== "string") throw new Error("导出报表包含无效文本。");
  return value;
}
function nullableString(value: unknown): string | null { return value === null ? null : string(value); }
function finiteNumber(value: unknown, integer = false): number {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0 || (integer && !Number.isSafeInteger(value))) throw new Error("导出报表包含无法准确表示的数值。");
  return value;
}
function array<T>(value: unknown, read: (row: unknown) => T): T[] {
  if (!Array.isArray(value)) throw new Error("导出报表缺少数据列表。");
  return value.map(read);
}
function sourceId(value: unknown): ExportSourceId {
  const id = string(value);
  if (!sourceIds.has(id)) throw new Error("导出报表包含无法识别的应用来源。");
  return id as ExportSourceId;
}
function readMetrics(value: Record<string, unknown>): ExportMetrics {
  const usage = record(value.usage);
  const cost = record(value.cost);
  return {
    usage: Object.fromEntries(usageKeys.map(key => [key, finiteNumber(usage[key], true)])) as unknown as DeviceTokenUsage,
    cost: { usd: finiteNumber(cost.usd), cny: finiteNumber(cost.cny), unpricedTokens: finiteNumber(cost.unpricedTokens, true) },
  };
}
function readModel(value: unknown): ExportModelRow {
  const row = record(value);
  return { sourceId: sourceId(row.sourceId), sourceLabel: string(row.sourceLabel), model: string(row.model), ...readMetrics(row) };
}

/** Copy only public report fields, keeping raw paths or task data out of exported files. */
export function normalizeUsageExportReport(value: unknown): UsageExportReport {
  const report = record(value);
  const query = record(report.query);
  const range = record(query.range);
  const ids = array(query.sourceIds, sourceId);
  let normalizedQuery: UsageExportQuery;
  if (range.kind === "all") normalizedQuery = createUsageExportQuery("all", ids, "", "");
  else if (range.kind === "dates") normalizedQuery = createUsageExportQuery("custom", ids, string(range.startDate), string(range.endDate));
  else throw new Error("导出报表的日期范围无效。");
  if (report.schemaVersion !== 1 || report.timeZone !== "system-local" || !Number.isInteger(report.utcOffsetMinutes) || Math.abs(Number(report.utcOffsetMinutes)) > 1440) throw new Error("无法识别导出报表的版本或时区。");
  return {
    schemaVersion: 1,
    generatedAt: string(report.generatedAt), query: normalizedQuery, timeZone: "system-local", utcOffsetMinutes: report.utcOffsetMinutes as number,
    lastCollectionAt: nullableString(report.lastCollectionAt), pricingUpdatedAt: nullableString(report.pricingUpdatedAt), summary: readMetrics(record(report.summary)),
    models: array(report.models, readModel),
    daily: array(report.daily, value => { const row = record(value); return { date: nullableString(row.date), sourceId: sourceId(row.sourceId), sourceLabel: string(row.sourceLabel), ...readMetrics(row) }; }),
    dailyModels: array(report.dailyModels, value => ({ ...readModel(value), date: nullableString(record(value).date) })),
    sources: array(report.sources, value => {
      const row = record(value);
      const status = string(row.status) as ExportSourceRow["status"];
      if (!["ready", "notDetected", "unavailable", "error", "unknown"].includes(status)) throw new Error("导出报表的来源状态无效。");
      if (row.indexReadStatus !== "readable" && row.indexReadStatus !== "notApplicable") throw new Error("导出报表的索引状态无效。");
      return { sourceId: sourceId(row.sourceId), sourceLabel: string(row.sourceLabel), status, indexReadStatus: row.indexReadStatus, lastIndexedAt: nullableString(row.lastIndexedAt), collectionStatusObservedAt: nullableString(row.collectionStatusObservedAt), ...readMetrics(row) };
    }),
    projects: array(report.projects, value => {
      const row = record(value);
      if (typeof row.merged !== "boolean") throw new Error("导出报表的项目合并状态无效。");
      const ids = array(row.sourceIds, sourceId);
      const labels = array(row.sourceLabels, string);
      if (ids.length !== labels.length) throw new Error("导出报表的项目来源不完整。");
      return { project: string(row.project), sourceIds: ids, sourceLabels: labels, merged: row.merged, ...readMetrics(row) };
    }),
  };
}

const dimensionColumns: Record<Exclude<UsageExportDataset, "full">, readonly string[]> = {
  models: ["sourceId", "sourceLabel", "model"],
  daily: ["date", "dateStatus", "sourceId", "sourceLabel"],
  dailyModels: ["date", "dateStatus", "sourceId", "sourceLabel", "model"],
  sources: ["sourceId", "sourceLabel", "status", "indexReadStatus", "lastIndexedAt", "collectionStatusObservedAt"],
  projects: ["project", "sourceIds", "sourceLabels", "merged"],
};
const datasetFileNames: Record<UsageExportDataset, string> = { models: "models", daily: "daily", dailyModels: "daily-models", sources: "sources", projects: "projects", full: "full-report" };
const accountingNotes = [
  "报表统计本机已索引的用量，不代表账号额度或账号在其他设备上的全部消耗。",
  "日期按生成报表时的系统本地时区统计，utcOffsetMinutes 记录生成时的 UTC 时差。",
  "totalTokens 已包含记录中的缓存和输出用量；reasoningOutputTokens 是 outputTokens 的子集，缓存写入时长分类是 cacheWriteTokens 的子项，不要再次累加这些子项。",
  "费用按已保存的模型价格及高峰规则估算，不是实际账单。",
  "usd 与 cny 分别记录美元和人民币金额，未作汇率换算，不可直接相加。",
  "unpricedTokens 表示尚未定价的用量，不代表免费。",
  "指定日期范围会排除日期未知的记录；全部历史包含日期未知的记录，并以 date=null 表示。",
  "项目用量按已保存的项目合并规则汇总，merged 表示是否应用合并。",
] as const;

export function escapeUsageCsvCell(value: unknown): string {
  if (value === null || value === undefined) return "";
  const raw = Array.isArray(value) ? value.join("; ") : String(value);
  const safe = typeof value === "number" || typeof value === "boolean" ? raw : /^[\s\uFEFF]*[=+\-@]|^[\t\r\n]/.test(raw) ? `'${raw}` : raw;
  return /[",\r\n]/.test(safe) ? `"${safe.replaceAll('"', '""')}"` : safe;
}

export function serializeUsageExport(report: UsageExportReport, dataset: UsageExportDataset, format: UsageExportFormat): string {
  // Reconstruct the approved public schema even if a caller supplied extra properties.
  const safe = normalizeUsageExportReport(report);
  if (format === "json") {
    if (dataset === "full") return `${JSON.stringify({ ...safe, accountingNotes }, null, 2)}\n`;
    const { models: _models, daily: _daily, dailyModels: _dailyModels, sources: _sources, projects: _projects, ...metadata } = safe;
    return `${JSON.stringify({ ...metadata, accountingNotes, dataset, rows: safe[dataset] }, null, 2)}\n`;
  }
  if (dataset === "full") throw new Error("完整报表请使用 JSON 格式。");
  const dimensions = dimensionColumns[dataset];
  const columns = [...dimensions, ...usageKeys, ...costKeys];
  const rows = safe[dataset].map(row => {
    const values = row as unknown as Record<string, unknown>;
    return [...dimensions.map(key => key === "dateStatus" ? values.date === null ? "unknown" : "recorded" : values[key]), ...usageKeys.map(key => row.usage[key]), ...costKeys.map(key => row.cost[key])].map(escapeUsageCsvCell).join(",");
  });
  return `\uFEFF${[columns.join(","), ...rows].join("\r\n")}\r\n`;
}

export function usageExportFileName(report: UsageExportReport, dataset: UsageExportDataset, format: UsageExportFormat): string {
  const range = report.query.range;
  const period = range.kind === "all" ? "all" : `${range.startDate}_${range.endDate}`;
  const generated = report.generatedAt.replace(/[^0-9]/g, "").slice(0, 14);
  return `ai-token-${datasetFileNames[dataset]}-${period}-${generated}.${format}`;
}
