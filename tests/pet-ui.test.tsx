// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_PET_STATE, type PetState } from "../src/core/pet-types";
import type { DashboardSnapshot, RefreshProgress, RefreshSchedule } from "../src/core/types";
import type { DashboardAdapter } from "../src/ui/dashboard-adapter";
import type { PetApi } from "../src/ui/pet-api";
import { PetSettings } from "../src/ui/pet-settings";
import { spriteFrameStyle, PET_ANIMATIONS } from "../src/ui/pet-sprite";
import { buildPetUsageView, newestPetSnapshot } from "../src/ui/pet-view-model";
import { PetWindow } from "../src/ui/pet-window";
import { createMockDashboardSnapshot } from "../src/ui/mock-data";

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

const importedPet = { id: "fox", displayName: "狐仙", description: "本地宠物", spriteVersionNumber: 2 as const, spritesheetPath: "data:image/webp;base64,AAAA" };
function makeApi(initial: PetState = structuredClone(DEFAULT_PET_STATE)): PetApi {
  return {
    native: true,
    readState: vi.fn().mockResolvedValue(initial), subscribe: vi.fn().mockResolvedValue(() => {}),
    discover: vi.fn().mockResolvedValue([{ ...importedPet, sourcePath: "C:/pets/fox/pet.json" }]),
    selectDirectory: vi.fn().mockResolvedValue(null), preview: vi.fn().mockResolvedValue("data:image/webp;base64,AAAA"),
    importSource: vi.fn().mockResolvedValue({ preferences: { ...initial.preferences, selectedPetId: "fox" }, pets: [importedPet] }),
    selectFile: vi.fn().mockResolvedValue(null),
    setPreferences: vi.fn().mockImplementation(async preferences => ({ ...initial, preferences, pets: [importedPet] })),
    resetPosition: vi.fn().mockResolvedValue(initial), readAppTheme: vi.fn().mockResolvedValue("dark"), subscribeAppTheme: vi.fn().mockResolvedValue(() => {}),
    setLayout: vi.fn().mockResolvedValue(undefined), dragStart: vi.fn().mockResolvedValue(undefined), dragUpdate: vi.fn().mockResolvedValue(undefined), dragEnd: vi.fn().mockResolvedValue(undefined), showDashboard: vi.fn().mockResolvedValue(undefined),
  };
}

describe("pet usage presentation", () => {
  it("uses all device activity and preserves separate currency amounts", () => {
    const snapshot = createMockDashboardSnapshot("ready");
    snapshot.deviceUsage!.today.totalTokens = 125678;
    snapshot.deviceUsage!.todayCost = { usd: 2.5, cny: 8, unpricedTokens: 500 };
    const view = buildPetUsageView(snapshot, Date.now());
    expect(view.tokensLabel).toContain("12.6");
    expect(view.costLabel).toContain("2.50");
    expect(view.costLabel).toContain("8.00");
    expect(view.details).toContain("125,678 Token");
    expect(view.details).toContain("500 Token");
    expect(view.quotas.map(item => item.label)).toEqual(["5h", "7天", "储备"]);
    expect(view).not.toHaveProperty("tokensPerSecond");
  });
  it("retains server remaining quota after reset time and labels stale data", () => {
    const snapshot = createMockDashboardSnapshot("ready");
    snapshot.quotaWindows.forEach(quota => { quota.resetsAt = 1; quota.remainingPercent = 18; });
    snapshot.accountSync = { status: "offline", quota: { stale: true, lastSuccessfulAt: "2026-11-07T00:00:00Z" }, usage: { stale: false, lastSuccessfulAt: null } };
    const view = buildPetUsageView(snapshot, Date.parse("2026-11-07T06:00:00Z"));
    expect(view.quotas[0].remaining).toBe(18);
    expect(view.quotas[0].resetLabel).toBe("等待更新");
    expect(view.quotas[0].remainingLabel).toBe("待更新");
    expect(view.quotas[0].expired).toBe(true);
    expect(view.quotaStatus).toContain("旧额度");
    expect(view.details).toContain("旧数据");
  });
  it("distinguishes unavailable data from zero consumption and local-only mode", () => {
    expect(buildPetUsageView(null, Date.now()).tokensLabel).toBe("—");
    expect(buildPetUsageView(null, Date.now()).costLabel).toBe("费用 —");
    const snapshot = createMockDashboardSnapshot("ready");
    snapshot.quotaWindows = [];
    snapshot.accountSync = { status: "localOnly", quota: { stale: true, lastSuccessfulAt: null }, usage: { stale: true, lastSuccessfulAt: null } };
    expect(buildPetUsageView(snapshot, Date.now()).quotaStatus).toContain("纯本地模式");
    const newer = { ...snapshot, revision: 22 };
    expect(newestPetSnapshot(newer, { ...snapshot, revision: 21 })).toBe(newer);
  });
  it("uses the actual v1/v2 atlas grids and the required row frame counts", () => {
    expect(spriteFrameStyle(192, 7, 5, 1)).toMatchObject({ width: 192, height: 208, backgroundSize: "1536px 1872px", backgroundPosition: "-960px -1456px" });
    expect(spriteFrameStyle(192, 3, 3, 2)).toMatchObject({ backgroundSize: "1536px 2288px", backgroundPosition: "-576px -624px" });
    expect(PET_ANIMATIONS.idle.frames).toBe(6);
    expect(PET_ANIMATIONS.wave.frames).toBe(4);
  });
});

describe("pet settings", () => {
  it("imports a local Codex pet with preview and requires explicit visibility", async () => {
    const api = makeApi();
    render(<PetSettings appTheme="dark" api={api} />);
    await waitFor(() => expect((screen.getByRole("button", { name: "从本机 Codex 导入" }) as HTMLButtonElement).disabled).toBe(false));
    expect((screen.getByRole("checkbox", { name: "显示桌面宠物" }) as HTMLInputElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "从本机 Codex 导入" }));
    await screen.findByRole("img", { name: "狐仙 导入预览" });
    expect(api.preview).toHaveBeenCalledWith("C:/pets/fox/pet.json");
    fireEvent.click(screen.getByRole("button", { name: "导入这只宠物" }));
    await screen.findByRole("img", { name: "狐仙" });
    expect((screen.getByRole("checkbox", { name: "显示桌面宠物" }) as HTMLInputElement).checked).toBe(false);
    fireEvent.click(screen.getByRole("checkbox", { name: "显示桌面宠物" }));
    await waitFor(() => expect(api.setPreferences).toHaveBeenCalledWith(expect.objectContaining({ enabled: true, selectedPetId: "fox" })));
  });
  it("keeps the current pet when file selection is cancelled", async () => {
    const initial = { preferences: { ...DEFAULT_PET_STATE.preferences, selectedPetId: "fox" }, pets: [importedPet] };
    const api = makeApi(initial);
    render(<PetSettings appTheme="light" api={api} />);
    await screen.findByRole("img", { name: "狐仙" });
    fireEvent.click(screen.getByRole("button", { name: "导入宠物文件" }));
    await waitFor(() => expect(api.selectFile).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("img", { name: "狐仙" })).toBeTruthy();
    expect(api.setPreferences).not.toHaveBeenCalled();
  });
});

describe("pet window subscription", () => {
  function liveSnapshot(revision: number, accountStatus: "ready" | "syncing" | "offline" = "ready", localRevision = revision) {
    const snapshot = createMockDashboardSnapshot("ready");
    snapshot.revision = revision;
    snapshot.fetchedAt = new Date(1_790_000_000_000 + localRevision * 1000).toISOString();
    snapshot.refreshProgress = { phase: "complete", label: "本机统计已更新", processedSources: 6, totalSources: 6, currentSource: null, processedFiles: 0, totalFiles: 0 };
    snapshot.accountSync = { status: accountStatus, quota: { stale: accountStatus !== "ready", lastSuccessfulAt: snapshot.fetchedAt }, usage: { stale: accountStatus !== "ready", lastSuccessfulAt: snapshot.fetchedAt } };
    snapshot.refreshSchedule = { nextRefreshAtMs: Date.now() + 60_000, intervalSeconds: 60 };
    return snapshot;
  }

  function liveAdapter(initial: DashboardSnapshot) {
    const listeners: { snapshot?: (snapshot: DashboardSnapshot) => void; progress?: (progress: RefreshProgress) => void; schedule?: (schedule: RefreshSchedule) => void } = {};
    const stops = [vi.fn(), vi.fn(), vi.fn()];
    const adapter: DashboardAdapter = {
      readSnapshot: vi.fn(), refresh: vi.fn(), refreshLocal: vi.fn(),
      readCachedSnapshot: vi.fn().mockResolvedValue(initial),
      subscribe: vi.fn().mockImplementation(async listener => { listeners.snapshot = listener; return stops[0]; }),
      subscribeProgress: vi.fn().mockImplementation(async listener => { listeners.progress = listener; return stops[1]; }),
      subscribeRefreshSchedule: vi.fn().mockImplementation(async listener => { listeners.schedule = listener; return stops[2]; }),
    };
    return { adapter, listeners, stops };
  }

  it("uses native edge placement and unsubscribes when the pet closes", async () => {
    const api = makeApi({ preferences: { ...DEFAULT_PET_STATE.preferences, enabled: true, selectedPetId: "fox" }, pets: [importedPet] });
    let placementListener!: Parameters<NonNullable<PetApi["subscribeLayout"]>>[0];
    const stop = vi.fn();
    api.subscribeLayout = vi.fn(async listener => { placementListener = listener; return stop; });
    const { adapter } = liveAdapter(liveSnapshot(1));
    const result = render(<PetWindow api={api} adapter={adapter} />);
    const sprite = await screen.findByRole("img", { name: "狐仙" });
    act(() => placementListener({ width: 274, height: 225, sprite: { x: 0, y: 86, width: 128, height: 139 }, bar: { x: 0, y: 0, width: 274, height: 82 } }));
    expect(sprite.parentElement!.style.left).toBe("0px");
    expect(sprite.parentElement!.style.top).toBe("86px");
    expect(screen.getByRole("button", { name: "打开应用额度详情" }).style.top).toBe("0px");
    result.unmount();
    expect(stop).toHaveBeenCalledOnce();
  });

  it("recovers fresh quota when a push event was missed", async () => {
    vi.useFakeTimers();
    try {
      const initial = liveSnapshot(4);
      const fresh = liveSnapshot(5);
      fresh.deviceUsage!.today.totalTokens = 888;
      const { adapter } = liveAdapter(initial);
      vi.mocked(adapter.readCachedSnapshot!).mockResolvedValueOnce(initial).mockResolvedValue(fresh);
      const result = render(<PetWindow api={makeApi()} adapter={adapter} />);
      await act(async () => { await vi.advanceTimersByTimeAsync(0); });
      await act(async () => { await vi.advanceTimersByTimeAsync(45_000); });
      expect(screen.getByText("888")).toBeTruthy();
      expect(adapter.refresh).not.toHaveBeenCalled();
      result.unmount();
    } finally { vi.useRealTimers(); }
  });

  it("uses the real schedule and waits for the completed local and account values", async () => {
    const { adapter, listeners, stops } = liveAdapter(liveSnapshot(1));
    const result = render(<PetWindow api={makeApi()} adapter={adapter} />);
    const label = await screen.findByLabelText("自动更新状态");
    await waitFor(() => expect(label.textContent).toMatch(/约 \d+ 秒后自动更新/));
    act(() => listeners.schedule?.({ nextRefreshAtMs: Date.now() + 25_000, intervalSeconds: 60 }));
    expect(label.textContent).toMatch(/约 2[56] 秒后自动更新/);
    act(() => {
      listeners.progress?.({ phase: "codex", label: "正在读取", processedSources: 0, totalSources: 6, currentSource: "Codex", processedFiles: 0, totalFiles: 0 });
      listeners.schedule?.({ nextRefreshAtMs: Date.now() + 60_000, intervalSeconds: 60 });
      listeners.progress?.(liveSnapshot(2).refreshProgress!);
    });
    expect(label.textContent).toBe("正在更新");
    act(() => listeners.snapshot?.(liveSnapshot(2, "syncing")));
    expect(label.textContent).toBe("正在更新账号用量");
    act(() => listeners.snapshot?.(liveSnapshot(3, "ready", 2)));
    expect(label.textContent).toMatch(/约 \d+ 秒后自动更新/);
    act(() => listeners.snapshot?.(liveSnapshot(2, "syncing")));
    expect(label.textContent).toMatch(/约 \d+ 秒后自动更新/);
    expect(adapter.readSnapshot).not.toHaveBeenCalled();
    expect(adapter.refresh).not.toHaveBeenCalled();
    expect(adapter.refreshLocal).not.toHaveBeenCalled();
    result.unmount();
    stops.forEach(stop => expect(stop).toHaveBeenCalledOnce());
  });

  it("counts down from the backend deadline without creating a refresh request", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-11-08T10:00:00Z"));
    try {
      const { adapter } = liveAdapter(liveSnapshot(1));
      const result = render(<PetWindow api={makeApi()} adapter={adapter} />);
      await act(async () => {});
      const label = screen.getByLabelText("自动更新状态");
      expect(label.textContent).toBe("约 60 秒后自动更新");
      act(() => vi.advanceTimersByTime(1000));
      expect(label.textContent).toBe("约 59 秒后自动更新");
      act(() => vi.advanceTimersByTime(59_000));
      expect(label.textContent).toBe("即将更新");
      expect(adapter.refresh).not.toHaveBeenCalled();
      result.unmount();
    } finally { vi.useRealTimers(); }
  });

  it("does not let account-only pushes or completion events end the next local scan", async () => {
    const { adapter, listeners } = liveAdapter(liveSnapshot(1));
    render(<PetWindow api={makeApi()} adapter={adapter} />);
    const label = await screen.findByLabelText("自动更新状态");
    await waitFor(() => expect(label.textContent).toMatch(/约 \d+ 秒后自动更新/));
    act(() => listeners.progress?.({ phase: "sources", label: "正在扫描", processedSources: 2, totalSources: 6, currentSource: null, processedFiles: 0, totalFiles: 0 }));
    act(() => listeners.snapshot?.(liveSnapshot(2, "ready", 1)));
    act(() => listeners.progress?.(liveSnapshot(2).refreshProgress!));
    expect(label.textContent).toBe("正在更新");
    act(() => listeners.snapshot?.(liveSnapshot(3, "offline")));
    expect(label.textContent).toMatch(/约 \d+ 秒后自动更新/);
    expect(screen.getByText(/旧额度/)).toBeTruthy();
  });

  it("does not let a delayed cached snapshot finish a newly started scan or replace its schedule", async () => {
    const { adapter, listeners } = liveAdapter(liveSnapshot(1));
    let cached!: (snapshot: DashboardSnapshot) => void;
    vi.mocked(adapter.readCachedSnapshot!).mockImplementation(() => new Promise(resolve => { cached = resolve; }));
    render(<PetWindow api={makeApi()} adapter={adapter} />);
    await waitFor(() => expect(adapter.readCachedSnapshot).toHaveBeenCalledOnce());
    act(() => {
      listeners.progress?.({ phase: "codex", label: "正在扫描", processedSources: 0, totalSources: 6, currentSource: null, processedFiles: 0, totalFiles: 0 });
      listeners.schedule?.({ nextRefreshAtMs: Date.now() + 10_000, intervalSeconds: 60 });
    });
    await act(async () => cached(liveSnapshot(1)));
    expect(screen.getByLabelText("自动更新状态").textContent).toBe("正在更新");
    act(() => listeners.snapshot?.(liveSnapshot(2)));
    expect(screen.getByLabelText("自动更新状态").textContent).toMatch(/约 \d+ 秒后自动更新/);
  });

  it("shows an interrupted channel instead of a misleading countdown when subscription fails", async () => {
    const { adapter, listeners } = liveAdapter(liveSnapshot(1));
    vi.mocked(adapter.subscribe!).mockRejectedValue(new Error("订阅失败"));
    render(<PetWindow api={makeApi()} adapter={adapter} />);
    await screen.findByText("后台更新通道中断");
    act(() => listeners.progress?.({ phase: "codex", label: "正在扫描", processedSources: 0, totalSources: 6, currentSource: null, processedFiles: 0, totalFiles: 0 }));
    expect(screen.getByLabelText("自动更新状态").textContent).toBe("后台更新通道中断");
    expect(adapter.readCachedSnapshot).toHaveBeenCalledOnce();
  });

  it("uses waiting on hover and screen-coordinate direction during a captured drag", async () => {
    const api = makeApi({ preferences: { ...DEFAULT_PET_STATE.preferences, enabled: true, selectedPetId: "fox" }, pets: [importedPet] });
    const { adapter } = liveAdapter(liveSnapshot(1));
    render(<PetWindow api={api} adapter={adapter} />);
    const sprite = await screen.findByRole("img", { name: "狐仙" });
    const handle = sprite.parentElement!;
    handle.setPointerCapture = vi.fn(); handle.hasPointerCapture = () => true; handle.releasePointerCapture = vi.fn();
    const pointer = (kind: string, screenX: number, screenY = 100) => {
      const event = new MouseEvent(kind, { bubbles: true, button: 0, clientX: 140, clientY: 60, screenX, screenY });
      Object.defineProperty(event, "pointerId", { value: 7 });
      return event;
    };
    fireEvent.pointerEnter(handle);
    expect(sprite.dataset.animation).toBe("waiting");
    fireEvent(handle, pointer("pointerdown", 100));
    fireEvent(handle, pointer("pointermove", 103));
    expect(sprite.dataset.animation).toBe("waiting");
    fireEvent(handle, pointer("pointermove", 104));
    expect(sprite.dataset.animation).toBe("running-right");
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 125)); });
    expect(sprite.dataset.frame).toBe("1");
    fireEvent(handle, pointer("pointermove", 114));
    expect(sprite.dataset.frame).toBe("1");
    fireEvent(handle, pointer("pointermove", 114, 150));
    expect(sprite.dataset.animation).toBe("running-right");
    fireEvent(handle, pointer("pointermove", 110, 150));
    expect(sprite.dataset.animation).toBe("running-left");
    expect(sprite.dataset.frame).toBe("0");
    fireEvent(handle, pointer("pointerup", 110, 150));
    expect(sprite.dataset.animation).toBe("waiting");
    await waitFor(() => expect(api.dragEnd).toHaveBeenCalledOnce());
    for (const kind of ["pointercancel", "lostpointercapture"]) {
      fireEvent(handle, pointer("pointerdown", 100));
      fireEvent(handle, pointer("pointermove", 90));
      expect(sprite.dataset.animation).toBe("running-left");
      fireEvent(handle, pointer(kind, 90));
      expect(sprite.dataset.animation).toBe("waiting");
      await waitFor(() => expect(api.dragEnd).toHaveBeenCalledTimes(kind === "pointercancel" ? 2 : 3));
    }
  });

  it("retains the pointer-down origin when a quick drag ends before native startup responds", async () => {
    const api = makeApi({ preferences: { ...DEFAULT_PET_STATE.preferences, enabled: true, selectedPetId: "fox" }, pets: [importedPet] });
    let ready!: () => void;
    vi.mocked(api.dragStart).mockImplementation(() => new Promise<void>(resolve => { ready = resolve; }));
    const adapter: DashboardAdapter = { readSnapshot: vi.fn(), readCachedSnapshot: vi.fn().mockResolvedValue(null) };
    render(<PetWindow api={api} adapter={adapter} />);
    const sprite = await screen.findByRole("img", { name: "狐仙" });
    const handle = sprite.parentElement!;
    handle.setPointerCapture = vi.fn();
    handle.hasPointerCapture = () => true;
    handle.releasePointerCapture = vi.fn();
    const pointer = (kind: string, x: number, y: number) => {
      const event = new MouseEvent(kind, { bubbles: true, button: 0, clientX: x, clientY: y });
      Object.defineProperty(event, "pointerId", { value: 7 });
      return event;
    };
    fireEvent(handle, pointer("pointerdown", 140, 60));
    fireEvent(handle, pointer("pointerup", 85, 32));
    expect(api.dragStart).toHaveBeenCalledWith(140, 60);
    expect(api.dragEnd).not.toHaveBeenCalled();
    await act(async () => ready());
    await waitFor(() => expect(api.dragEnd).toHaveBeenCalledOnce());
    expect(api.dragUpdate).toHaveBeenCalledOnce();
  });

  it("subscribes before reading cache, rejects older cache, and never refreshes", async () => {
    const snapshot = createMockDashboardSnapshot("ready");
    const fresh = { ...snapshot, revision: 20, deviceUsage: { ...snapshot.deviceUsage!, today: { ...snapshot.deviceUsage!.today, totalTokens: 888 } } };
    const old = { ...snapshot, revision: 19, deviceUsage: { ...snapshot.deviceUsage!, today: { ...snapshot.deviceUsage!.today, totalTokens: 111 } } };
    const stop = vi.fn();
    const adapter: DashboardAdapter = {
      readSnapshot: vi.fn(), refresh: vi.fn(), refreshLocal: vi.fn(),
      subscribe: vi.fn().mockImplementation(async (listener: (value: DashboardSnapshot) => void) => { listener(fresh); return stop; }),
      readCachedSnapshot: vi.fn().mockResolvedValue(old),
    };
    const api = makeApi({ preferences: { ...DEFAULT_PET_STATE.preferences, enabled: true, selectedPetId: "fox" }, pets: [importedPet] });
    const result = render(<PetWindow api={api} adapter={adapter} />);
    await screen.findByText("888");
    expect(adapter.readSnapshot).not.toHaveBeenCalled();
    expect(adapter.refresh).not.toHaveBeenCalled();
    expect(adapter.refreshLocal).not.toHaveBeenCalled();
    expect(screen.queryByText("111")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "打开应用额度详情" }));
    expect(api.showDashboard).toHaveBeenCalledTimes(1);
    result.unmount();
    expect(stop).toHaveBeenCalledTimes(1);
  });
});
