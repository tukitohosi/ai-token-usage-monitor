// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
const bridge = vi.hoisted(() => ({ invoke: vi.fn(), isTauri: vi.fn(() => true) }));
vi.mock("@tauri-apps/api/core", () => bridge);
import { exportSavedPricing, openPricingImport } from "../src/ui/pricing-transfer-api";
import { mergePricingImport, parsePricingTransfer } from "../src/core/pricing-transfer";
import { createPricingDraft, parsePricingDraft } from "../src/ui/pricing-editor";
import type { PricingSettings } from "../src/core/types";

afterEach(() => vi.clearAllMocks());
describe("native pricing transfer bridge", () => {
  it("treats a cancelled picker as no import", async () => {
    bridge.invoke.mockResolvedValue(null);
    expect(await openPricingImport()).toBeNull();
    expect(bridge.invoke).toHaveBeenCalledWith("import_pricing_file");
  });
  it("never sends frontend draft content to the native exporter", async () => {
    bridge.invoke.mockResolvedValue({ status: "saved", path: "prices.json" });
    await exportSavedPricing({ updatedAt: null, peak: { multiplier: 2, windows: [], timeZone: "local", excludeChinaHolidays: false, specialDates: [] }, models: [] });
    expect(bridge.invoke).toHaveBeenCalledWith("export_pricing_file");
  });
  it("preserves native cancellation and errors", async () => {
    bridge.invoke.mockResolvedValue({ status: "cancelled", path: null });
    expect((await exportSavedPricing()).status).toBe("cancelled");
    bridge.invoke.mockRejectedValue(new Error("无法保存"));
    await expect(exportSavedPricing()).rejects.toThrow("无法保存");
  });
  it("reimports browser-exported v2 settings with both enabled and disabled rules", async () => {
    bridge.isTauri.mockReturnValueOnce(false);
    const createUrl = vi.fn((_blob: Blob) => "blob:test-pricing");
    const originalUrl = URL;
    vi.stubGlobal("URL", class extends originalUrl { static createObjectURL = createUrl; static revokeObjectURL = vi.fn(); });
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    vi.useFakeTimers();
    try {
      const model = { modelId: "astra", displayName: "Astra", inputPerMillion: 10, cachedInputPerMillion: 1, cacheWritePerMillion: null, outputPerMillion: 50, currency: "USD" as const, peakEnabled: true };
      const saved: PricingSettings = { updatedAt: null, peak: { multiplier: 2, windows: [], timeZone: "local", excludeChinaHolidays: false, specialDates: [] },
        models: [{ ...model, longContext: { threshold: 272000, inputMultiplier: 2, outputMultiplier: 1.5 } }, { ...model, modelId: "other", longContext: null }] };
      await exportSavedPricing(saved);
      vi.runOnlyPendingTimers();
      vi.useRealTimers();
      const blob = createUrl.mock.calls[0]![0];
      const reader = new FileReader();
      const text = await new Promise<string>((resolve, reject) => {
        reader.onload = () => resolve(String(reader.result)); reader.onerror = () => reject(reader.error);
        reader.readAsText(blob);
      });
      const file = parsePricingTransfer(text);
      expect(file.version).toBe(2);
      const draft = createPricingDraft({ ...saved, models: [] });
      expect(parsePricingDraft(mergePricingImport(draft, file, true))).toEqual(saved);
    } finally {
      vi.useRealTimers(); vi.unstubAllGlobals(); vi.restoreAllMocks();
    }
  });
});
