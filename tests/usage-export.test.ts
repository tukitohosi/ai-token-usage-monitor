import { describe, expect, it } from "vitest";
import {
  createUsageExportQuery, escapeUsageCsvCell, normalizeUsageExportReport, serializeUsageExport, usageExportFileName,
  type UsageExportReport,
} from "../src/core/usage-export.js";

const metrics = { usage: { inputTokens: 120, cachedInputTokens: 60, cacheWriteTokens: 20, cacheWrite5mTokens: 12, cacheWrite1hTokens: 8, cacheWriteUnknownTokens: 0, outputTokens: 70, reasoningOutputTokens: 30, totalTokens: 270 }, cost: { usd: .00123, cny: 0, unpricedTokens: 20 } };
function fixture(): UsageExportReport {
  return {
    schemaVersion: 1, generatedAt: "2026-10-08T01:02:03.000Z", query: { range: { kind: "dates", startDate: "2026-09-09", endDate: "2026-10-08" }, sourceIds: ["codex"] },
    timeZone: "system-local", utcOffsetMinutes: 480, lastCollectionAt: null, pricingUpdatedAt: null,
    summary: structuredClone(metrics),
    models: [{ sourceId: "codex", sourceLabel: "Codex", model: "模型, \"A\"\n第二行", ...structuredClone(metrics) }],
    daily: [{ date: "2026-10-08", sourceId: "codex", sourceLabel: "Codex", ...structuredClone(metrics) }],
    dailyModels: [{ date: "2026-10-08", sourceId: "codex", sourceLabel: "Codex", model: "模型A", ...structuredClone(metrics) }],
    sources: [{ sourceId: "codex", sourceLabel: "Codex", status: "unknown", indexReadStatus: "readable", lastIndexedAt: null, collectionStatusObservedAt: null, ...structuredClone(metrics) }],
    projects: [{ project: "=SUM(A1:A2)", sourceIds: ["codex"], sourceLabels: ["Codex"], merged: false, ...structuredClone(metrics) }],
  };
}

describe("usage export files", () => {
  it("keeps date presets local, includes both ends and permits custom ranges longer than 31 days", () => {
    const now = new Date(2026, 9, 8, 12);
    expect(createUsageExportQuery("last30Days", ["codex"], "", "", now).range).toEqual({ kind: "dates", startDate: "2026-09-09", endDate: "2026-10-08" });
    expect(createUsageExportQuery("today", ["codex"], "", "", now).range).toEqual({ kind: "dates", startDate: "2026-10-08", endDate: "2026-10-08" });
    expect(createUsageExportQuery("custom", ["codex"], "2024-02-29", "2026-10-08").range).toEqual({ kind: "dates", startDate: "2024-02-29", endDate: "2026-10-08" });
    expect(() => createUsageExportQuery("custom", ["codex"], "2026-02-29", "2026-10-08")).toThrow(/有效/);
    expect(() => createUsageExportQuery("custom", ["codex"], "2026-10-09", "2026-10-08")).toThrow(/晚于/);
    expect(() => createUsageExportQuery("all", [], "", "")).toThrow(/至少/);
  });

  it("exports UTF-8 BOM, CRLF, quoted Chinese cells and the existing exact token totals", () => {
    const csv = serializeUsageExport(fixture(), "models", "csv");
    expect(csv.startsWith("\uFEFFsourceId,sourceLabel,model,")).toBe(true);
    expect(csv).toContain('"模型, ""A""\n第二行"');
    expect(csv).toContain(",120,60,20,12,8,0,70,30,270,0.00123,0,20\r\n");
    expect(csv.endsWith("\r\n")).toBe(true);
    expect(csv).not.toContain("万");
  });

  it("neutralizes formula-looking strings while preserving numeric cells", () => {
    for (const value of ["=1+1", "+SUM(A1)", "-1+1", "@SUM(A1)", "   =1", "\t=1", "\r=1", "\n@x"]) {
      expect(escapeUsageCsvCell(value).replace(/^"/, "").startsWith("'")).toBe(true);
    }
    expect(escapeUsageCsvCell(123456789)).toBe("123456789");
    expect(escapeUsageCsvCell(-2)).toBe("-2");
    expect(serializeUsageExport(fixture(), "projects", "csv")).toContain("'=SUM(A1:A2)");
  });

  it("preserves metadata, undated rows, unpriced usage and distinct currencies in JSON", () => {
    const report = fixture(); report.query.range = { kind: "all" }; report.daily[0]!.date = null;
    const complete = JSON.parse(serializeUsageExport(report, "full", "json"));
    const { accountingNotes, ...data } = complete;
    expect(data).toEqual(report);
    expect(accountingNotes).toHaveLength(8);
    expect(accountingNotes).toContain("unpricedTokens 表示尚未定价的用量，不代表免费。");
    expect(accountingNotes.some((note: string) => note.includes("reasoningOutputTokens 是 outputTokens 的子集"))).toBe(true);
    const models = JSON.parse(serializeUsageExport(report, "models", "json"));
    expect(models).toMatchObject({ schemaVersion: 1, query: report.query, dataset: "models", rows: report.models, summary: metrics });
    expect(models.accountingNotes).toEqual(accountingNotes);
    expect(models.daily).toBeUndefined();
    expect(() => serializeUsageExport(report, "full", "csv")).toThrow(/JSON/);
  });

  it("keeps headers for empty CSV and discards fields outside the public report schema", () => {
    const report = fixture(); report.models = [];
    expect(serializeUsageExport(report, "models", "csv").split("\r\n")).toHaveLength(2);
    const withSecrets = { ...fixture(), apiKey: "not-for-export", rawPath: "C:/private/user", summary: { ...metrics, transcript: "not-for-export" }, models: [{ ...fixture().models[0]!, taskId: "not-for-export" }] };
    expect(serializeUsageExport(withSecrets, "full", "json")).not.toMatch(/not-for-export|private\/user|taskId|apiKey/);
  });

  it("rejects unsafe token precision or a malformed backend report", () => {
    const report = fixture(); report.summary.usage.totalTokens = Number.MAX_SAFE_INTEGER + 1;
    expect(() => normalizeUsageExportReport(report)).toThrow(/准确/);
    expect(() => normalizeUsageExportReport({ ...fixture(), schemaVersion: 99 })).toThrow(/版本/);
    expect(() => normalizeUsageExportReport({ ...fixture(), sources: [{ ...fixture().sources[0], indexReadStatus: "failed" }] })).toThrow(/索引/);
  });

  it("uses filesystem-safe names that retain report date range and timestamp", () => {
    expect(usageExportFileName(fixture(), "dailyModels", "csv")).toBe("ai-token-daily-models-2026-09-09_2026-10-08-20261008010203.csv");
  });
});
