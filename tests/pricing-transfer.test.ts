import { describe, expect, it } from "vitest";
import { mergePricingImport, normalizePricingTransfer, parsePricingTransfer, pricingImportSummary, type PricingTransferDocumentV1 } from "../src/core/pricing-transfer.js";

function document(): PricingTransferDocumentV1 {
  return { format: "ai-token-pricing", version: 1, unit: "perMillionTokens", exportedAt: "2026-11-07T10:00:00Z", savedAt: null,
    peak: { windows: [{ weekdayStart: 1, weekdayEnd: 5, startTime: "09:00", endTime: "12:00" }], multiplier: 3, timeZone: "Asia/Tokyo", excludeChinaHolidays: true, specialDates: ["2026-11-08"] },
    models: [{ modelId: "model-a", displayName: "导入名称", currency: "CNY", inputPerMillion: 0, cachedInputPerMillion: null, cacheWritePerMillion: 0.125, outputPerMillion: 10, peakEnabled: false },
      { modelId: "new-model", displayName: "新模型", currency: "USD", inputPerMillion: 1, cachedInputPerMillion: 2, cacheWritePerMillion: 3, outputPerMillion: 4, peakEnabled: true }] };
}
function draft(): Parameters<typeof mergePricingImport>[0] {
  return { updatedAt: "2026-11-06T10:00:00Z", pendingSpecialDate: "2026-11-09",
    peak: { windows: [], multiplier: "1.5", timeZone: "local", excludeChinaHolidays: false, specialDates: [] },
    models: [{ modelId: "MODEL-A", displayName: "本地名称", currency: "USD", inputPerMillion: "12.", cachedInputPerMillion: "", cacheWritePerMillion: "1", outputPerMillion: "2", peakEnabled: true },
      { modelId: "keep", displayName: "保留", currency: "USD", inputPerMillion: "unfinished", cachedInputPerMillion: "", cacheWritePerMillion: "", outputPerMillion: "", peakEnabled: true }] };
}

describe("pricing JSON transfer", () => {
  it("roundtrips currencies, all rates, zero/null and the complete peak schedule", () => {
    const original = document();
    expect(parsePricingTransfer(JSON.stringify(original))).toEqual(original);
    expect(parsePricingTransfer(`\uFEFF${JSON.stringify(original)}`)).toEqual(original);
  });
  it("merges by case-insensitive ID, preserves unrelated raw draft and local identity", () => {
    const original = draft(); const before = JSON.stringify(original);
    const next = mergePricingImport(original, document());
    expect(pricingImportSummary(original, document())).toEqual({ added: 1, overwritten: 1, total: 3 });
    expect(next.models[0]).toMatchObject({ modelId: "MODEL-A", displayName: "导入名称", currency: "CNY", inputPerMillion: "0", cachedInputPerMillion: "", cacheWritePerMillion: "0.125", peakEnabled: true });
    expect(next.models[1]).toBe(original.models[1]);
    expect(next.models[1]!.inputPerMillion).toBe("unfinished");
    expect(next.models[2]!.peakEnabled).toBe(false);
    expect(next.peak).toBe(original.peak);
    expect(next.pendingSpecialDate).toBe("2026-11-09");
    expect(next.updatedAt).toBe(original.updatedAt);
    expect(JSON.stringify(original)).toBe(before);
  });
  it("imports peak rules only when selected, retaining flags on unmatched models", () => {
    const next = mergePricingImport(draft(), document(), true);
    expect(next.peak).toMatchObject({ ...document().peak, multiplier: "3" });
    expect(next.pendingSpecialDate).toBe("");
    expect(next.models.map(model => model.peakEnabled)).toEqual([false, true, true]);
    expect(next.updatedAt).toBe(draft().updatedAt);
  });
  it("checks full merged count and refuses atomically without truncation", () => {
    const current = draft();
    current.models = Array.from({ length: 200 }, (_, index) => ({ ...current.models[0]!, modelId: `existing-${index}` }));
    const before = JSON.stringify(current);
    expect(() => mergePricingImport(current, document())).toThrow(/202.*当前 200.*新增 2/);
    expect(JSON.stringify(current)).toBe(before);
    const incoming = document(); incoming.models[0]!.modelId = "EXISTING-0"; incoming.models = [incoming.models[0]!];
    expect(mergePricingImport(current, incoming).models).toHaveLength(200);
  });
  it("recomputes against the latest draft rather than an older preview", () => {
    const current = draft();
    expect(pricingImportSummary(current, document()).added).toBe(1);
    current.models.push({ ...current.models[0]!, modelId: "new-model" });
    expect(pricingImportSummary(current, document()).added).toBe(0);
    expect(mergePricingImport(current, document()).models).toHaveLength(3);
  });
  it.each([
    ["format", "other"], ["version", 2], ["unit", "perToken"], ["exportedAt", "yesterday"], ["savedAt", "broken"], ["unexpected", true],
  ])("rejects invalid %s", (key, value) => {
    expect(() => normalizePricingTransfer({ ...document(), [key]: value })).toThrow();
  });
  it("rejects duplicate IDs, missing prices, invalid rates, currencies and oversized files", () => {
    const duplicate = document(); duplicate.models[1]!.modelId = "MODEL-A";
    expect(() => normalizePricingTransfer(duplicate)).toThrow(/重复/);
    for (const price of [-1, 1_000_001, Number.NaN, Number.POSITIVE_INFINITY, "1", undefined]) {
      const bad = document(); (bad.models[0] as unknown as Record<string, unknown>).outputPerMillion = price;
      expect(() => normalizePricingTransfer(bad)).toThrow(/单价/);
    }
    const bad = document(); (bad.models[0] as unknown as Record<string, unknown>).currency = "EUR";
    expect(() => normalizePricingTransfer(bad)).toThrow(/币种/);
    expect(() => parsePricingTransfer("{broken")).toThrow(/解析/);
    expect(() => parsePricingTransfer(" ".repeat(1024 * 1024 + 1))).toThrow(/1 MiB/);
  });
  it("validates imported peak fields even when not applying them", () => {
    const bad = document(); bad.peak.timeZone = "bad/city";
    expect(() => mergePricingImport(draft(), bad, false)).toThrow(/时区/);
    const badDate = document(); badDate.peak.specialDates = ["2026-02-30"];
    expect(() => normalizePricingTransfer(badDate)).toThrow(/特殊日期/);
    const badWindow = document(); badWindow.peak.windows[0]!.weekdayStart = 0;
    expect(() => normalizePricingTransfer(badWindow)).toThrow(/星期/);
  });
});
