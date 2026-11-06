// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../src/App";
import { MockDashboardAdapter } from "../src/ui/dashboard-adapter";
import type { PricingTransferDocumentV1 } from "../src/core/pricing-transfer";
import { exportSavedPricing, openPricingImport } from "../src/ui/pricing-transfer-api";

vi.mock("../src/ui/pricing-transfer-api", async (original) => ({
  ...await original<typeof import("../src/ui/pricing-transfer-api")>(),
  openPricingImport: vi.fn(),
  exportSavedPricing: vi.fn(),
}));

beforeEach(() => {
  vi.clearAllMocks();
  Object.defineProperty(window, "matchMedia", { configurable: true, value: vi.fn(() => ({ matches: false, addEventListener() {}, removeEventListener() {} })) });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

async function setup() {
  const adapter = new MockDashboardAdapter("ready", 0);
  const saved = await adapter.readPricingSettings();
  const model = saved.models[0]!;
  const transfer: PricingTransferDocumentV1 = {
    format: "ai-token-pricing", version: 1, unit: "perMillionTokens", exportedAt: "2026-11-07T00:00:00Z",
    peak: { ...saved.peak, multiplier: 4, windows: [], specialDates: ["2026-11-07"] },
    models: [{ ...model, modelId: model.modelId.toUpperCase(), inputPerMillion: 0, outputPerMillion: null, peakEnabled: true }],
  };
  render(<App adapter={adapter} />);
  fireEvent.click(await screen.findByRole("button", { name: "模型定价" }));
  await screen.findByLabelText("GPT-5.6 Sol 新输入");
  return { adapter, saved, transfer };
}

describe("pricing transfer inside application", () => {
  it("imports into the latest draft without saving, then applies through existing local recalculation", async () => {
    const { adapter, transfer } = await setup();
    const save = vi.spyOn(adapter, "setPricingSettings");
    const local = vi.spyOn(adapter, "refreshLocal");
    vi.mocked(openPricingImport).mockResolvedValue(transfer);
    fireEvent.change(screen.getByLabelText("收费倍率"), { target: { value: "2.75" } });
    fireEvent.click(screen.getByRole("button", { name: "导入 JSON" }));
    await screen.findByRole("dialog", { name: "导入模型定价" });
    expect((screen.getByRole("button", { name: "保存价格并重算" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "导入到编辑页" }));
    expect((screen.getByLabelText("GPT-5.6 Sol 新输入") as HTMLInputElement).value).toBe("0");
    expect((screen.getByLabelText("GPT-5.6 Sol 输出") as HTMLInputElement).value).toBe("");
    expect((screen.getByLabelText("收费倍率") as HTMLInputElement).value).toBe("2.75");
    expect(save).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "保存价格并重算" }));
    await waitFor(() => expect(local).toHaveBeenCalledOnce());
    expect(save.mock.calls[0]![0].models.find(item => item.modelId === "GPT-5.6 Sol")).toMatchObject({ inputPerMillion: 0, outputPerMillion: null });
  });

  it("only replaces global peak rules when explicitly checked", async () => {
    const { transfer } = await setup();
    vi.mocked(openPricingImport).mockResolvedValue(transfer);
    fireEvent.click(screen.getByRole("button", { name: "导入 JSON" }));
    await screen.findByRole("dialog");
    const peak = screen.getByRole("checkbox", { name: /同时导入高峰规则/ });
    expect((peak as HTMLInputElement).checked).toBe(false);
    fireEvent.click(peak);
    expect(screen.getByText(/也会影响本地其他已开启高峰计费/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "导入到编辑页" }));
    expect((screen.getByLabelText("收费倍率") as HTMLInputElement).value).toBe("4");
    expect(screen.getByText("2026-11-07")).toBeTruthy();
  });

  it("keeps edits on picker cancellation and exports the saved configuration", async () => {
    const { saved } = await setup();
    let resolvePicker!: (value: null) => void;
    vi.mocked(openPricingImport).mockImplementation(() => new Promise(resolve => { resolvePicker = resolve; }));
    fireEvent.change(screen.getByLabelText("GPT-5.6 Sol 新输入"), { target: { value: "23." } });
    fireEvent.click(screen.getByRole("button", { name: "导入 JSON" }));
    expect((screen.getByRole("button", { name: "导入 JSON" }) as HTMLButtonElement).disabled).toBe(true);
    await act(async () => resolvePicker(null));
    expect((screen.getByLabelText("GPT-5.6 Sol 新输入") as HTMLInputElement).value).toBe("23.");
    vi.mocked(exportSavedPricing).mockResolvedValue({ status: "saved", path: null });
    fireEvent.click(screen.getByRole("button", { name: "导出 JSON" }));
    await screen.findByText(/当前未保存修改未包含在文件中/);
    expect(exportSavedPricing).toHaveBeenCalledWith(saved);
    expect((screen.getByLabelText("GPT-5.6 Sol 新输入") as HTMLInputElement).value).toBe("23.");
  });

  it("reports invalid import without discarding local edits", async () => {
    await setup();
    vi.mocked(openPricingImport).mockRejectedValue(new Error("定价文件版本无效"));
    fireEvent.change(screen.getByLabelText("GPT-5.6 Sol 新输入"), { target: { value: "7." } });
    fireEvent.click(screen.getByRole("button", { name: "导入 JSON" }));
    await screen.findByText("定价文件版本无效");
    expect((screen.getByLabelText("GPT-5.6 Sol 新输入") as HTMLInputElement).value).toBe("7.");
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
