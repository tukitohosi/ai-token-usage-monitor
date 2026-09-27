// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../src/App";
import { MockDashboardAdapter, type DashboardSnapshotListener } from "../src/ui/dashboard-adapter";
import { createMockDashboardSnapshot } from "../src/ui/mock-data";
import type { PricingSettings } from "../src/core/types";

beforeEach(() => {
  Object.defineProperty(window, "matchMedia", { configurable: true, value: vi.fn(() => ({ matches: false, addEventListener() {}, removeEventListener() {} })) });
});
afterEach(() => { cleanup(); vi.useRealTimers(); vi.restoreAllMocks(); });

class PushAdapter extends MockDashboardAdapter {
  listener?: DashboardSnapshotListener;
  constructor() { super("ready", 0); }
  async subscribe(listener: DashboardSnapshotListener) { this.listener = listener; return () => { this.listener = undefined; }; }
  emit(revision: number, newModel = false) {
    const snapshot = createMockDashboardSnapshot("ready");
    snapshot.revision = revision;
    if (newModel) {
      const source = snapshot.deviceUsage!.sources[0]!;
      source.byModel.push({ ...source.byModel[0]!, label: "new-model" });
    }
    this.listener?.(snapshot);
  }
}

async function openPricing(adapter = new PushAdapter()) {
  render(<App adapter={adapter} />);
  fireEvent.click(await screen.findByRole("button", { name: "模型定价" }));
  await screen.findByLabelText("GPT-5.6 Sol 新输入");
  return adapter;
}

describe("pricing draft lifetime and local save", () => {
  it("ignores an older snapshot after a newer account or local push", async () => {
    const adapter = await openPricing();
    const latest = { ...createMockDashboardSnapshot("ready"), revision: 20, message: "最新本机结果" };
    const older = { ...createMockDashboardSnapshot("ready"), revision: 19, message: "迟到的旧结果" };
    act(() => adapter.listener?.(latest));
    act(() => adapter.listener?.(older));
    expect(screen.getByText("最新本机结果")).toBeTruthy();
    expect(screen.queryByText("迟到的旧结果")).toBeNull();
  });

  it("keeps every field, focus and decimal text across two scheduled pushes, manual refresh, navigation and new models", async () => {
    const adapter = await openPricing();
    const input = screen.getByLabelText("GPT-5.6 Sol 新输入") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "12." } });
    fireEvent.change(screen.getByLabelText("GPT-5.6 Sol 输出"), { target: { value: "" } });
    fireEvent.change(screen.getByLabelText("GPT-5.6 Sol 缓存读入"), { target: { value: "0.125" } });
    fireEvent.change(screen.getByLabelText("GPT-5.6 Sol 缓存写入"), { target: { value: "3.75" } });
    fireEvent.change(screen.getByLabelText("GPT-5.6 Sol 币种"), { target: { value: "CNY" } });
    fireEvent.change(screen.getByLabelText("开始时间"), { target: { value: "20:30" } });
    fireEvent.change(screen.getByLabelText("结束时间"), { target: { value: "04:15" } });
    fireEvent.change(screen.getByLabelText("收费倍率"), { target: { value: "2.5" } });
    fireEvent.click(screen.getByLabelText("默认模型 高峰定价"));
    input.focus();
    vi.useFakeTimers();
    const timer = setInterval(() => adapter.emit(1), 60_000);
    await act(async () => { vi.advanceTimersByTime(120_000); });
    clearInterval(timer);
    vi.useRealTimers();
    expect(input.value).toBe("12.");
    expect(document.activeElement).toBe(input);
    act(() => adapter.emit(2, true));
    expect(screen.getByLabelText("GPT-5.6 Sol 新输入")).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(await screen.findByLabelText("new-model 新输入")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "刷新数据" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "刷新数据" })).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "费用" }));
    fireEvent.click(screen.getByRole("button", { name: "模型定价" }));
    for (const [label, value] of [["新输入", "12."], ["输出", ""], ["缓存读入", "0.125"], ["缓存写入", "3.75"], ["币种", "CNY"]]) {
      expect((screen.getByLabelText(`GPT-5.6 Sol ${label}`) as HTMLInputElement).value).toBe(value);
    }
    expect((screen.getByLabelText("开始时间") as HTMLInputElement).value).toBe("20:30");
    expect((screen.getByLabelText("结束时间") as HTMLInputElement).value).toBe("04:15");
    expect((screen.getByLabelText("收费倍率") as HTMLInputElement).value).toBe("2.5");
    expect((screen.getByLabelText("默认模型 高峰定价") as HTMLInputElement).checked).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "放弃修改" }));
    expect((screen.getByLabelText("GPT-5.6 Sol 新输入") as HTMLInputElement).value).toBe("4");
  });

  it("retains edits made while saving and uses only local recalculation", async () => {
    const adapter = new PushAdapter();
    let resolveSave!: (value: PricingSettings) => void;
    const save = vi.spyOn(adapter, "setPricingSettings").mockImplementation(() => new Promise(resolve => { resolveSave = resolve; }));
    const local = vi.spyOn(adapter, "refreshLocal").mockResolvedValue(createMockDashboardSnapshot("ready"));
    const online = vi.spyOn(adapter, "refresh");
    await openPricing(adapter);
    online.mockClear();
    fireEvent.change(screen.getByLabelText("GPT-5.6 Sol 新输入"), { target: { value: "5" } });
    fireEvent.click(screen.getByRole("button", { name: "保存价格并重算" }));
    expect((screen.getByRole("button", { name: "保存并重算中…" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(screen.getByLabelText("GPT-5.6 Sol 新输入"), { target: { value: "6." } });
    await act(async () => resolveSave(save.mock.calls[0]![0]));
    expect((screen.getByLabelText("GPT-5.6 Sol 新输入") as HTMLInputElement).value).toBe("6.");
    expect(screen.getByText("有未保存修改")).toBeTruthy();
    expect(local).toHaveBeenCalledOnce();
    expect(online).not.toHaveBeenCalled();
    expect(save).toHaveBeenCalledOnce();
  });

  it("preserves failed saves, distinguishes recalculation failure, and retries without saving again", async () => {
    const adapter = new PushAdapter();
    const save = vi.spyOn(adapter, "setPricingSettings").mockRejectedValueOnce(new Error("保存失败"));
    const local = vi.spyOn(adapter, "refreshLocal").mockRejectedValueOnce(new Error("离线索引失败")).mockResolvedValue(createMockDashboardSnapshot("ready"));
    await openPricing(adapter);
    fireEvent.change(screen.getByLabelText("GPT-5.6 Sol 新输入"), { target: { value: "9.1" } });
    fireEvent.click(screen.getByRole("button", { name: "保存价格并重算" }));
    expect(await screen.findByText("保存失败")).toBeTruthy();
    expect((screen.getByLabelText("GPT-5.6 Sol 新输入") as HTMLInputElement).value).toBe("9.1");
    fireEvent.click(screen.getByRole("button", { name: "保存价格并重算" }));
    expect(await screen.findByText("价格已保存，费用重算失败。请重试费用重算。")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "重试费用重算" }));
    expect(await screen.findByText("价格已保存，并已按新规则重新汇总本机费用。")).toBeTruthy();
    expect(save).toHaveBeenCalledTimes(2);
    expect(local).toHaveBeenCalledTimes(2);
  });

  it("waits for initial settings and rejects invalid decimal input without discarding it", async () => {
    const adapter = new PushAdapter();
    const original = await adapter.readPricingSettings();
    let complete!: (settings: PricingSettings) => void;
    vi.spyOn(adapter, "readPricingSettings").mockImplementation(() => new Promise(resolve => { complete = resolve; }));
    const save = vi.spyOn(adapter, "setPricingSettings");
    render(<App adapter={adapter} />);
    fireEvent.click(screen.getByRole("button", { name: "模型定价" }));
    expect(screen.getByText("正在读取已保存价格…")).toBeTruthy();
    expect(screen.queryByLabelText("GPT-5.6 Sol 新输入")).toBeNull();
    await act(async () => complete(original));
    fireEvent.change(screen.getByLabelText("GPT-5.6 Sol 新输入"), { target: { value: "-1" } });
    fireEvent.click(screen.getByRole("button", { name: "保存价格并重算" }));
    expect(await screen.findByText(/单价请输入有效的非负数字/)).toBeTruthy();
    expect(save).not.toHaveBeenCalled();
    expect((screen.getByLabelText("GPT-5.6 Sol 新输入") as HTMLInputElement).value).toBe("-1");
  });
});
