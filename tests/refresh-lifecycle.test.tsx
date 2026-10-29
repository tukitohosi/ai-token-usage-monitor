// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../src/App";
import type { AccountSync, DashboardSnapshot, RefreshProgress, RefreshSchedule } from "../src/core/types";
import { MockDashboardAdapter, type DashboardSnapshotListener } from "../src/ui/dashboard-adapter";
import { createMockDashboardSnapshot } from "../src/ui/mock-data";

beforeEach(() => {
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: vi.fn(() => ({ matches: false, addEventListener() {}, removeEventListener() {} })),
  });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

const initialFetchedAt = Date.now();

function snapshot(revision: number, accountStatus: AccountSync["status"] = "ready", localRevision = revision): DashboardSnapshot {
  const value = createMockDashboardSnapshot("ready");
  value.revision = revision;
  value.fetchedAt = new Date(initialFetchedAt + localRevision * 1_000).toISOString();
  value.lastSuccessfulAt = value.fetchedAt;
  if (value.deviceUsage) value.deviceUsage.generatedAt = value.fetchedAt;
  value.message = null;
  value.accountSync = {
    status: accountStatus,
    quota: { lastSuccessfulAt: value.fetchedAt, stale: accountStatus !== "ready" },
    usage: { lastSuccessfulAt: value.fetchedAt, stale: accountStatus !== "ready" },
  };
  return value;
}

function progress(phase: "codex" | "complete"): RefreshProgress {
  return {
    phase,
    label: phase === "complete" ? "刷新完成" : "正在读取 Codex 本机历史",
    processedSources: phase === "complete" ? 6 : 0,
    totalSources: 6,
    currentSource: phase === "complete" ? null : "Codex",
    processedFiles: 0,
    totalFiles: 0,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

class RefreshAdapter extends MockDashboardAdapter {
  snapshotListener?: DashboardSnapshotListener;
  progressListener?: (value: RefreshProgress) => void;
  scheduleListener?: (value: RefreshSchedule) => void;
  pending?: ReturnType<typeof deferred<DashboardSnapshot>>;
  constructor() { super("ready", 0); }
  override async refresh() { return this.pending ? this.pending.promise : snapshot(1); }
  async subscribe(listener: DashboardSnapshotListener) {
    this.snapshotListener = listener;
    return () => { this.snapshotListener = undefined; };
  }
  async subscribeProgress(listener: (value: RefreshProgress) => void) {
    this.progressListener = listener;
    return () => { this.progressListener = undefined; };
  }
  async subscribeRefreshSchedule(listener: (value: RefreshSchedule) => void) {
    this.scheduleListener = listener;
    return () => { this.scheduleListener = undefined; };
  }
}

async function openDashboard() {
  const adapter = new RefreshAdapter();
  render(<App adapter={adapter} />);
  await screen.findByRole("button", { name: "刷新数据" });
  return adapter;
}

function expectRefreshing() {
  expect((screen.getByRole("button", { name: "正在刷新数据" }) as HTMLButtonElement).disabled).toBe(true);
  expect(document.querySelector(".refresh-countdown")!.textContent).not.toMatch(/约 \d+ 秒后自动更新/);
  expect(document.querySelector(".status-chip")!.textContent).not.toBe("数据已就绪");
}

describe("refresh completion follows the displayed data", () => {
  it("keeps manual refresh busy until its request returns, even when completion is emitted first", async () => {
    const adapter = await openDashboard();
    adapter.pending = deferred<DashboardSnapshot>();
    fireEvent.click(screen.getByRole("button", { name: "刷新数据" }));
    act(() => adapter.progressListener?.(progress("complete")));
    expectRefreshing();

    await act(async () => { adapter.pending!.resolve(snapshot(2)); });
    expect((screen.getByRole("button", { name: "刷新数据" }) as HTMLButtonElement).disabled).toBe(false);
    expect(document.querySelector(".refresh-countdown")!.textContent).toMatch(/约 \d+ 秒后自动更新/);
  });

  it("does not finish a scheduled refresh until the completed snapshot has arrived", async () => {
    const adapter = await openDashboard();
    act(() => {
      adapter.progressListener?.(progress("codex"));
      adapter.scheduleListener?.({ nextRefreshAtMs: Date.now() + 60_000, intervalSeconds: 60 });
      adapter.progressListener?.(progress("complete"));
    });
    expectRefreshing();

    act(() => adapter.snapshotListener?.(snapshot(2)));
    await waitFor(() => expect(screen.getByRole("button", { name: "刷新数据" })).toBeTruthy());
    expect(document.querySelector(".refresh-countdown")!.textContent).toMatch(/约 \d+ 秒后自动更新/);
  });

  it("waits for account values after local scanning finishes and applies them before announcing readiness", async () => {
    const adapter = await openDashboard();
    adapter.pending = deferred<DashboardSnapshot>();
    fireEvent.click(screen.getByRole("button", { name: "刷新数据" }));
    await act(async () => {
      adapter.progressListener?.(progress("complete"));
      adapter.pending!.resolve(snapshot(2, "syncing"));
    });
    expectRefreshing();
    expect(document.querySelector(".refresh-countdown")!.textContent).toMatch(/账号/);
    expect(document.querySelector(".refresh-countdown")!.textContent).toMatch(/更新|同步/);

    const updated = snapshot(3, "ready", 2);
    const fiveHourQuota = updated.quotaWindows.find((quota) => quota.limitId === "codex" && quota.windowDurationMins === 300)!;
    fiveHourQuota.usedPercent = 73;
    fiveHourQuota.remainingPercent = 27;
    act(() => adapter.snapshotListener?.(updated));
    expect((screen.getByRole("button", { name: "刷新数据" }) as HTMLButtonElement).disabled).toBe(false);
    expect(document.querySelectorAll(".overview-grid__quota")[0]!.textContent).toContain("已使用 73%");
    expect(document.querySelector(".status-chip")!.textContent).toBe("数据已就绪");
    expect(document.querySelector(".refresh-countdown")!.textContent).toMatch(/约 \d+ 秒后自动更新/);
  });

  it("keeps scheduled refresh busy while only the account request is still pending", async () => {
    const adapter = await openDashboard();
    act(() => {
      adapter.progressListener?.(progress("codex"));
      adapter.progressListener?.(progress("complete"));
      adapter.snapshotListener?.(snapshot(2, "syncing"));
    });
    expectRefreshing();
    act(() => adapter.snapshotListener?.(snapshot(3, "ready", 2)));
    expect(screen.getByRole("button", { name: "刷新数据" })).toBeTruthy();
  });

  it.each([
    ["offline", "连接失败"],
    ["error", "读取失败"],
    ["partial", "部分数据暂不可用"],
    ["unauthenticated", "未登录"],
    ["unsupported", "接口不可用"],
  ] as const)("releases refresh after an account %s result and keeps that result visible", async (accountStatus, label) => {
    const adapter = await openDashboard();
    act(() => adapter.snapshotListener?.(snapshot(2, "syncing", 1)));
    expectRefreshing();
    act(() => adapter.snapshotListener?.(snapshot(3, accountStatus, 1)));
    expect((screen.getByRole("button", { name: "刷新数据" }) as HTMLButtonElement).disabled).toBe(false);
    expect(document.querySelector(".status-chip")!.textContent).not.toBe("数据已就绪");
    fireEvent.click(document.querySelector<HTMLButtonElement>(".status-chip")!);
    expect(within(screen.getByRole("dialog", { name: "数据状态详情" })).getAllByText(label).length).toBeGreaterThan(0);
  });

  it("finishes a local-only refresh without waiting for an account request", async () => {
    const adapter = await openDashboard();
    adapter.pending = deferred<DashboardSnapshot>();
    fireEvent.click(screen.getByRole("button", { name: "刷新数据" }));
    await act(async () => { adapter.pending!.resolve(snapshot(2, "localOnly")); });
    expect((screen.getByRole("button", { name: "刷新数据" }) as HTMLButtonElement).disabled).toBe(false);
    expect(document.querySelector(".refresh-countdown")!.textContent).toMatch(/约 \d+ 秒后自动更新/);
  });

  it("keeps scanning when the account request completes before the local result", async () => {
    const adapter = await openDashboard();
    act(() => adapter.progressListener?.(progress("codex")));
    act(() => adapter.snapshotListener?.(snapshot(2, "syncing", 1)));
    expectRefreshing();
    act(() => adapter.snapshotListener?.(snapshot(3, "ready", 1)));
    expectRefreshing();
    act(() => adapter.progressListener?.(progress("complete")));
    expectRefreshing();

    act(() => adapter.snapshotListener?.(snapshot(4)));
    expect(screen.getByRole("button", { name: "刷新数据" })).toBeTruthy();
  });

  it("ignores a late older snapshot while the newest result is still syncing its account", async () => {
    const adapter = await openDashboard();
    act(() => {
      adapter.progressListener?.(progress("codex"));
      adapter.progressListener?.(progress("complete"));
      adapter.snapshotListener?.(snapshot(3, "syncing"));
    });
    expectRefreshing();
    act(() => adapter.snapshotListener?.(snapshot(2)));
    expectRefreshing();
    act(() => adapter.snapshotListener?.(snapshot(4, "ready", 3)));
    expect(screen.getByRole("button", { name: "刷新数据" })).toBeTruthy();
  });

  it("releases a rejected manual request and allows a later retry", async () => {
    const adapter = await openDashboard();
    adapter.pending = deferred<DashboardSnapshot>();
    fireEvent.click(screen.getByRole("button", { name: "刷新数据" }));
    expectRefreshing();
    await act(async () => { adapter.pending!.reject(new Error("本机扫描失败")); });
    expect((screen.getByRole("button", { name: "刷新数据" }) as HTMLButtonElement).disabled).toBe(false);
    expect(screen.getByText("无法更新真实用量数据，请稍后重试。")).toBeTruthy();

    adapter.pending = deferred<DashboardSnapshot>();
    fireEvent.click(screen.getByRole("button", { name: "刷新数据" }));
    expectRefreshing();
    await act(async () => { adapter.pending!.resolve(snapshot(2)); });
    expect(document.querySelector(".status-chip")!.textContent).toBe("数据已就绪");
    expect(screen.getByRole("button", { name: "刷新数据" })).toBeTruthy();
  });

  it("keeps manual retry available when snapshot pushes fail but progress events still arrive", async () => {
    const adapter = new RefreshAdapter();
    const subscribe = vi.spyOn(adapter, "subscribe").mockRejectedValue(new Error("快照通道不可用"));
    adapter.pending = deferred<DashboardSnapshot>();
    render(<App adapter={adapter} />);
    await waitFor(() => expect(subscribe).toHaveBeenCalledOnce());
    await act(async () => { adapter.pending!.resolve(snapshot(1, "syncing")); });

    const expectRetryAvailable = () => {
      expect((screen.getByRole("button", { name: "刷新数据" }) as HTMLButtonElement).disabled).toBe(false);
      expect(document.querySelector(".status-chip")!.textContent).toBe("后台更新通道中断");
      expect(document.querySelector(".refresh-countdown")!.textContent).not.toMatch(/约 \d+ 秒后自动更新/);
    };
    expectRetryAvailable();
    expect(adapter.progressListener).toBeTypeOf("function");
    act(() => adapter.progressListener?.(progress("codex")));
    expectRetryAvailable();
    act(() => adapter.progressListener?.(progress("complete")));
    expectRetryAvailable();

    adapter.pending = deferred<DashboardSnapshot>();
    fireEvent.click(screen.getByRole("button", { name: "刷新数据" }));
    expectRefreshing();
    act(() => adapter.progressListener?.(progress("complete")));
    expectRefreshing();
    await act(async () => { adapter.pending!.resolve(snapshot(2, "syncing")); });
    expectRetryAvailable();
  });

  it("does not let a late manual response finish the next background scan", async () => {
    const adapter = await openDashboard();
    adapter.pending = deferred<DashboardSnapshot>();
    fireEvent.click(screen.getByRole("button", { name: "刷新数据" }));
    const manualResult = snapshot(2);
    act(() => adapter.snapshotListener?.(manualResult));
    expectRefreshing();

    act(() => adapter.progressListener?.(progress("codex")));
    await act(async () => { adapter.pending!.resolve(manualResult); });
    expectRefreshing();

    act(() => adapter.snapshotListener?.(snapshot(3)));
    expect((screen.getByRole("button", { name: "刷新数据" }) as HTMLButtonElement).disabled).toBe(false);
    expect(document.querySelector(".status-chip")!.textContent).toBe("数据已就绪");
  });
});
