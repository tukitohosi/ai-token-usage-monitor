import {
  EXPORT_SOURCES, exportDateKey,
  type ExportDailyModelRow, type ExportDailyRow, type ExportMetrics,
  type ExportModelRow, type ExportProjectRow, type ExportSourceRow,
  type UsageExportQuery, type UsageExportReport,
} from "../core/usage-export";

function empty(): ExportMetrics {
  return { usage: { inputTokens: 0, cachedInputTokens: 0, cacheWriteTokens: 0, cacheWrite5mTokens: 0, cacheWrite1hTokens: 0, cacheWriteUnknownTokens: 0, outputTokens: 0, reasoningOutputTokens: 0, totalTokens: 0 }, cost: { usd: 0, cny: 0, unpricedTokens: 0 } };
}

function add(target: ExportMetrics, value: ExportMetrics) {
  for (const key of Object.keys(target.usage) as Array<keyof ExportMetrics["usage"]>) target.usage[key] += value.usage[key];
  for (const key of Object.keys(target.cost) as Array<keyof ExportMetrics["cost"]>) target.cost[key] += value.cost[key];
}

/** Synthetic, internally reconcilable data used only by the browser demo adapter. */
export function createMockUsageExportReport(query: UsageExportQuery, now = new Date()): UsageExportReport {
  const generatedAt = now.toISOString();
  const report: UsageExportReport = {
    schemaVersion: 1, generatedAt, query: structuredClone(query), timeZone: "system-local", utcOffsetMinutes: -now.getTimezoneOffset(),
    lastCollectionAt: generatedAt, pricingUpdatedAt: generatedAt, summary: empty(), models: [], daily: [], dailyModels: [], sources: [], projects: [],
  };
  const models = new Map<string, ExportModelRow>();
  const daily = new Map<string, ExportDailyRow>();
  const dailyModels = new Map<string, ExportDailyModelRow>();
  const projects = new Map<string, ExportProjectRow>();
  for (const [sourceIndex, source] of EXPORT_SOURCES.entries()) {
    if (!query.sourceIds.includes(source.id)) continue;
    const sourceRow: ExportSourceRow = {
      sourceId: source.id, sourceLabel: source.label, status: source.id === "cursor" ? "unavailable" : source.id === "workbuddy-ai" ? "notDetected" : "ready",
      indexReadStatus: source.id === "cursor" ? "notApplicable" : "readable", lastIndexedAt: sourceIndex < 4 ? generatedAt : null, collectionStatusObservedAt: generatedAt, ...empty(),
    };
    report.sources.push(sourceRow);
    if (sourceIndex >= 4) continue;
    for (let age = 0; age < 60; age++) {
      const date = new Date(now); date.setDate(date.getDate() - age);
      const day = exportDateKey(date);
      if (query.range.kind === "dates" && (day < query.range.startDate || day > query.range.endDate)) continue;
      const model = sourceIndex === 0 ? age % 3 ? "GPT-5.6 Sol" : "GPT-6 Astra" : sourceIndex === 1 ? "Claude Sonnet" : sourceIndex === 2 ? "DeepSeek Flash" : "默认模型";
      const metrics = empty();
      metrics.usage.inputTokens = (61 - age) * (sourceIndex + 1) * 230;
      metrics.usage.cachedInputTokens = metrics.usage.inputTokens * 2;
      metrics.usage.cacheWriteTokens = metrics.usage.cacheWrite5mTokens = sourceIndex === 1 ? 1000 : 0;
      metrics.usage.outputTokens = (age + 3) * 110;
      metrics.usage.reasoningOutputTokens = Math.floor(metrics.usage.outputTokens / 3);
      metrics.usage.totalTokens = metrics.usage.inputTokens + metrics.usage.cachedInputTokens + metrics.usage.cacheWriteTokens + metrics.usage.outputTokens;
      if (sourceIndex === 0) metrics.cost.usd = metrics.usage.totalTokens * 0.000004;
      else if (sourceIndex === 3) metrics.cost.cny = metrics.usage.totalTokens * 0.000002;
      else metrics.cost.unpricedTokens = metrics.usage.totalTokens;
      const modelKey = `${source.id}:${model}`;
      const dailyKey = `${day}:${source.id}`;
      const dayModelKey = `${dailyKey}:${model}`;
      const project = sourceIndex < 2 ? "示例项目 · 用量监控" : "示例项目 · 日常工作";
      if (!models.has(modelKey)) models.set(modelKey, { sourceId: source.id, sourceLabel: source.label, model, ...empty() });
      if (!daily.has(dailyKey)) daily.set(dailyKey, { date: day, sourceId: source.id, sourceLabel: source.label, ...empty() });
      if (!dailyModels.has(dayModelKey)) dailyModels.set(dayModelKey, { date: day, sourceId: source.id, sourceLabel: source.label, model, ...empty() });
      if (!projects.has(project)) projects.set(project, { project, sourceIds: [], sourceLabels: [], merged: true, ...empty() });
      const projectRow = projects.get(project)!;
      if (!projectRow.sourceIds.includes(source.id)) { projectRow.sourceIds.push(source.id); projectRow.sourceLabels.push(source.label); }
      [report.summary, sourceRow, models.get(modelKey)!, daily.get(dailyKey)!, dailyModels.get(dayModelKey)!, projectRow].forEach(target => add(target, metrics));
    }
  }
  const recordedDates = [...daily.values()].flatMap(row => row.date ? [row.date] : []).sort();
  const startDate = query.range.kind === "dates" ? query.range.startDate : recordedDates[0];
  const endDate = query.range.kind === "dates" ? query.range.endDate : recordedDates.at(-1);
  if (startDate && endDate) {
    for (const day = new Date(`${startDate}T12:00:00`); exportDateKey(day) <= endDate; day.setDate(day.getDate() + 1)) {
      const date = exportDateKey(day);
      for (const source of report.sources) {
        if (source.indexReadStatus === "notApplicable") continue;
        const key = `${date}:${source.sourceId}`;
        if (!daily.has(key)) daily.set(key, { date, sourceId: source.sourceId, sourceLabel: source.sourceLabel, ...empty() });
      }
    }
  }
  report.models = [...models.values()].sort((a, b) => b.usage.totalTokens - a.usage.totalTokens);
  report.daily = [...daily.values()].sort((a, b) => a.date!.localeCompare(b.date!) || a.sourceId.localeCompare(b.sourceId));
  report.dailyModels = [...dailyModels.values()].sort((a, b) => a.date!.localeCompare(b.date!) || b.usage.totalTokens - a.usage.totalTokens);
  report.projects = [...projects.values()].sort((a, b) => b.usage.totalTokens - a.usage.totalTokens);
  return report;
}
