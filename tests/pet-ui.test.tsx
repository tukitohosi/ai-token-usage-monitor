// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_PET_STATE, type PetState } from "../src/core/pet-types";
import type { DashboardSnapshot } from "../src/core/types";
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
