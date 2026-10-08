// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { UsageExportDialog } from "../src/ui/usage-export-dialog";
import { MockDashboardAdapter, TauriDashboardAdapter, USAGE_EXPORT_COMMAND, SAVE_USAGE_EXPORT_COMMAND } from "../src/ui/dashboard-adapter";
import { createMockUsageExportReport } from "../src/ui/mock-usage-export";
import { EXPORT_SOURCES, type UsageExportReport } from "../src/core/usage-export";

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

function setup() {
  const adapter = new MockDashboardAdapter("ready", 0);
  const read = vi.spyOn(adapter, "readUsageExport");
  const save = vi.spyOn(adapter, "saveUsageExport").mockResolvedValue({ status: "saved", path: "D:\\报表\\usage.csv" });
  const close = vi.fn();
  const rendered = render(<UsageExportDialog adapter={adapter} onClose={close} />);
  return { adapter, read, save, close, ...rendered };
}

describe("export report dialog", () => {
  it("marks synthetic reports clearly and prefixes demo filenames", async () => {
    const adapter = new MockDashboardAdapter("ready", 0);
    const save = vi.spyOn(adapter, "saveUsageExport").mockResolvedValue({ status: "saved", path: null });
    render(<UsageExportDialog adapter={adapter} onClose={() => {}} isDemo />);
    expect(screen.getByRole("note").textContent).toBe("演示报表 · 数字为示例");
    fireEvent.click(screen.getByRole("button", { name: "生成报表" }));
    fireEvent.click(await screen.findByRole("button", { name: "导出完整报表 JSON" }));
    await waitFor(() => expect(save).toHaveBeenCalledOnce());
    expect(save.mock.calls[0]![0].suggestedFileName).toMatch(/^demo-ai-token-full-report-.*\.json$/);
  });

  it("defaults to 30 local dates and all sources, then reuses one report for multiple exports", async () => {
    const { read, save } = setup();
    expect((screen.getByLabelText("日期范围") as HTMLSelectElement).value).toBe("last30Days");
    for (const source of EXPORT_SOURCES) expect((screen.getByLabelText(source.label) as HTMLInputElement).checked).toBe(true);
    expect(read).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "生成报表" }));
    await screen.findByRole("button", { name: "导出模型用量 CSV" });
    fireEvent.click(screen.getByRole("button", { name: "导出模型用量 CSV" }));
    await screen.findByText(/已保存至/);
    fireEvent.click(screen.getByRole("button", { name: "导出完整报表 JSON" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
    expect(read).toHaveBeenCalledOnce();
    const complete = JSON.parse(save.mock.calls[1]![0].content) as UsageExportReport;
    expect(complete.query.sourceIds).toHaveLength(6);
    for (const key of ["models", "daily", "dailyModels", "sources", "projects"] as const) expect(complete[key].reduce((sum, row) => sum + row.usage.totalTokens, 0)).toBe(complete.summary.usage.totalTokens);
    expect(screen.getByText(/不能据此判断实际用量为零/)).toBeTruthy();
  });

  it("keeps the captured query when filters change and accepts a long custom range upon regeneration", async () => {
    const { read, save } = setup();
    fireEvent.click(screen.getByRole("button", { name: "生成报表" }));
    await screen.findByRole("button", { name: "导出每日趋势 CSV" });
    const originalQuery = structuredClone(read.mock.calls[0]![0]);
    fireEvent.change(screen.getByLabelText("日期范围"), { target: { value: "custom" } });
    fireEvent.change(screen.getByLabelText("开始日期"), { target: { value: "2025-01-01" } });
    fireEvent.change(screen.getByLabelText("结束日期"), { target: { value: "2026-10-08" } });
    expect(screen.getByText(/范围已更改/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "导出完整报表 JSON" }));
    await waitFor(() => expect(save).toHaveBeenCalledOnce());
    expect(JSON.parse(save.mock.calls[0]![0].content).query).toEqual(originalQuery);
    fireEvent.click(screen.getByRole("button", { name: "重新生成报表" }));
    await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
    expect(read.mock.calls[1]![0].range).toEqual({ kind: "dates", startDate: "2025-01-01", endDate: "2026-10-08" });
  });

  it("handles invalid dates, cancellation and save failures without losing the report", async () => {
    const { read, save } = setup();
    fireEvent.change(screen.getByLabelText("日期范围"), { target: { value: "custom" } });
    fireEvent.change(screen.getByLabelText("开始日期"), { target: { value: "2026-10-08" } });
    fireEvent.change(screen.getByLabelText("结束日期"), { target: { value: "2026-01-01" } });
    fireEvent.click(screen.getByRole("button", { name: "生成报表" }));
    expect(await screen.findByRole("alert")).toHaveProperty("textContent", "开始日期不能晚于结束日期。");
    expect(read).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("日期范围"), { target: { value: "all" } });
    fireEvent.click(screen.getByRole("button", { name: "生成报表" }));
    await screen.findByRole("button", { name: "导出模型用量 CSV" });
    save.mockResolvedValueOnce({ status: "cancelled", path: null });
    fireEvent.click(screen.getByRole("button", { name: "导出模型用量 CSV" }));
    expect(await screen.findByText(/已取消保存/)).toBeTruthy();
    save.mockRejectedValueOnce(new Error("文件写入失败"));
    fireEvent.click(screen.getByRole("button", { name: "导出模型用量 CSV" }));
    expect(await screen.findByText("文件写入失败")).toBeTruthy();
    expect(read).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: "导出模型用量 CSV" })).toHaveProperty("disabled", false);
  });

  it("traps keyboard focus, closes with Escape and restores the opening control", () => {
    const opener = document.createElement("button"); opener.textContent = "打开导出"; document.body.append(opener); opener.focus();
    const { close, unmount } = setup();
    const closing = screen.getByRole("button", { name: "关闭导出报表" });
    expect(document.activeElement).toBe(closing);
    fireEvent.keyDown(document, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "生成报表" }));
    fireEvent.keyDown(document, { key: "Tab" });
    expect(document.activeElement).toBe(closing);
    fireEvent.keyDown(document, { key: "Escape" }); expect(close).toHaveBeenCalledOnce();
    unmount(); expect(document.activeElement).toBe(opener); opener.remove();
  });

  it("retains an earlier report if regeneration fails and prevents duplicate generation", async () => {
    const { read } = setup();
    fireEvent.click(screen.getByRole("button", { name: "生成报表" }));
    await screen.findByRole("button", { name: "导出模型用量 CSV" });
    let reject!: (error: Error) => void;
    read.mockImplementationOnce(() => new Promise((_resolve, fail) => { reject = fail; }));
    fireEvent.click(screen.getByRole("button", { name: "重新生成报表" }));
    expect(screen.getByRole("button", { name: "导出模型用量 CSV" })).toHaveProperty("disabled", true);
    await act(async () => reject(new Error("索引暂不可读")));
    expect(await screen.findByText("索引暂不可读")).toBeTruthy();
    expect(within(screen.getByLabelText("已生成报表")).getByText(/已记录 Token/)).toBeTruthy();
  });
});

describe("export adapter contract", () => {
  it("fills every requested local date for readable mock sources while keeping model days sparse", () => {
    const now = new Date(2026, 9, 8, 12);
    const selected = EXPORT_SOURCES.map(source => source.id);
    const report = createMockUsageExportReport({ range: { kind: "dates", startDate: "2026-08-01", endDate: "2026-09-10" }, sourceIds: selected }, now);
    expect(report.daily).toHaveLength(41 * 5);
    expect(report.daily.filter(row => row.sourceId === "workbuddy-ai")).toHaveLength(41);
    expect(report.daily.filter(row => row.sourceId === "cursor")).toHaveLength(0);
    expect(report.daily.filter(row => row.date === "2026-08-01").every(row => row.usage.totalTokens === 0)).toBe(true);
    expect(report.dailyModels.some(row => row.date === "2026-08-01")).toBe(false);
    expect(report.daily.reduce((sum, row) => sum + row.usage.totalTokens, 0)).toBe(report.summary.usage.totalTokens);
    const all = createMockUsageExportReport({ range: { kind: "all" }, sourceIds: selected }, now);
    expect(all.daily).toHaveLength(60 * 5);
  });

  it("passes exact read and native-save command payloads and validates returned data", async () => {
    const query = { range: { kind: "all" as const }, sourceIds: ["codex" as const] };
    const report = createMockUsageExportReport(query);
    const invoke = vi.fn().mockResolvedValueOnce(report).mockResolvedValueOnce({ status: "cancelled", path: null });
    const adapter = new TauriDashboardAdapter({ invoke, listen: vi.fn() });
    await expect(adapter.readUsageExport(query)).resolves.toEqual(report);
    expect(invoke).toHaveBeenNthCalledWith(1, USAGE_EXPORT_COMMAND, { query });
    const request = { format: "json" as const, suggestedFileName: "usage.json", content: "{}\n" };
    await expect(adapter.saveUsageExport(request)).resolves.toEqual({ status: "cancelled", path: null });
    expect(invoke).toHaveBeenNthCalledWith(2, SAVE_USAGE_EXPORT_COMMAND, { request });
    invoke.mockResolvedValueOnce({ status: "done" });
    await expect(adapter.saveUsageExport(request)).rejects.toThrow(/无法确认/);
  });
});
