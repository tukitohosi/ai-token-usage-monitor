// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
const bridge = vi.hoisted(() => ({ invoke: vi.fn(), isTauri: vi.fn(() => true) }));
vi.mock("@tauri-apps/api/core", () => bridge);
import { exportSavedPricing, openPricingImport } from "../src/ui/pricing-transfer-api";

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
});
