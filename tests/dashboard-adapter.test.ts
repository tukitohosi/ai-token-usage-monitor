import { describe, expect, it, vi } from "vitest";

import type { DashboardSnapshot } from "../src/core/types.js";
import { createMockDashboardSnapshot } from "../src/ui/mock-data.js";

// Keep the composite Node test project independent of browser-only Tauri
// modules while still loading the real adapter through Vitest at runtime.
const adapterModulePath = "../src/ui/dashboard-adapter.js";
const {
  createDashboardAdapter,
  MockDashboardAdapter,
  normalizeDashboardSnapshot,
  normalizeRefreshSchedule,
  APP_PREFERENCES_COMMAND,
  SET_APP_PREFERENCES_COMMAND,
  PROGRESS_EVENT,
  REFRESH_SCHEDULE_EVENT,
  PLAN_RENEWAL_COMMAND,
  CLEAR_BACKGROUND_COMMAND,
  SELECT_BACKGROUND_COMMAND,
  THEME_PREFERENCE_COMMAND,
  TEST_NOTIFICATION_COMMAND,
  INDEX_MAINTENANCE_COMMAND,
  REBUILD_INDEXES_COMMAND,
  PROJECT_MERGE_RULES_COMMAND,
  SET_PROJECT_MERGE_RULES_COMMAND,
  USAGE_DAY_DETAIL_COMMAND,
  VISUAL_PREFERENCES_COMMAND,
  SNAPSHOT_COMMAND,
  CACHED_SNAPSHOT_COMMAND,
  SNAPSHOT_EVENT,
  TauriDashboardAdapter,
} = await import(adapterModulePath);

type SnapshotEventHandler = (event: { payload: unknown }) => void;

function bridgeFixture(snapshot: DashboardSnapshot) {
  let eventHandler: SnapshotEventHandler | null = null;
  const unsubscribe = vi.fn();
  const bridge = {
    invoke: vi.fn(async (_command: string, _args?: Record<string, unknown>): Promise<unknown> => snapshot),
    listen: vi.fn(async (_event: string, handler: SnapshotEventHandler) => {
      eventHandler = handler;
      return unsubscribe;
    }),
  };
  return {
    bridge,
    unsubscribe,
    emit(payload: unknown) {
      if (!eventHandler) throw new Error("listener was not registered");
      eventHandler({ payload });
    },
  };
}

describe("Tauri dashboard adapter", () => {
  it("invokes the exact snapshot command and preserves an honest null renewal", async () => {
    const snapshot = createMockDashboardSnapshot("ready");
    const fixture = bridgeFixture(snapshot);
    const adapter = new TauriDashboardAdapter(fixture.bridge);

    await expect(adapter.refresh()).resolves.toEqual(snapshot);
    expect(fixture.bridge.invoke).toHaveBeenCalledOnce();
    expect(fixture.bridge.invoke).toHaveBeenCalledWith(SNAPSHOT_COMMAND);

    const legacySnapshot = { ...snapshot } as Record<string, unknown>;
    delete legacySnapshot.planRenewalAt;
    expect(normalizeDashboardSnapshot(legacySnapshot).planRenewalAt).toBeNull();
    delete legacySnapshot.refreshSchedule;
    expect(normalizeDashboardSnapshot(legacySnapshot).refreshSchedule).toBeNull();
    expect(normalizeRefreshSchedule({ nextRefreshAtMs: 1_000, intervalSeconds: 60 })).toEqual({ nextRefreshAtMs: 1_000, intervalSeconds: 60 });
  });

  it("loads the persisted snapshot through the fast startup command", async () => {
    const snapshot = createMockDashboardSnapshot("ready");
    const fixture = bridgeFixture(snapshot);
    const adapter = new TauriDashboardAdapter(fixture.bridge);
    await expect(adapter.readCachedSnapshot()).resolves.toEqual(snapshot);
    expect(fixture.bridge.invoke).toHaveBeenCalledWith(CACHED_SNAPSHOT_COMMAND);
  });

  it("forwards verified snapshot events and ignores malformed pushes", async () => {
    const snapshot = createMockDashboardSnapshot("ready");
    const fixture = bridgeFixture(snapshot);
    const adapter = new TauriDashboardAdapter(fixture.bridge);
    const listener = vi.fn();

    const unsubscribe = await adapter.subscribe(listener);
    expect(fixture.bridge.listen).toHaveBeenCalledWith(SNAPSHOT_EVENT, expect.any(Function));

    fixture.emit(snapshot);
    fixture.emit({ status: "ready", quotaWindows: "not-an-array" });
    expect(listener).toHaveBeenCalledOnce();
    expect(listener).toHaveBeenCalledWith(snapshot);

    unsubscribe();
    expect(fixture.unsubscribe).toHaveBeenCalledOnce();
  });

  it("rejects account diagnostics containing non-read RPC methods", () => {
    const snapshot = createMockDashboardSnapshot("ready");
    expect(() => normalizeDashboardSnapshot({
      ...snapshot,
      accountDiagnostics: {
        readAt: snapshot.fetchedAt,
        durationMs: 3,
        methods: ["account/read", "turn/start"],
      },
    })).toThrow(/账号读取诊断/);
  });

  it("stores a manual renewal through the typed local command", async () => {
    const snapshot = createMockDashboardSnapshot("ready");
    const setting = {
      planRenewalAt: "2027-02-03T04:34:56Z",
      planRenewalSource: "manual" as const,
    };
    const fixture = bridgeFixture(snapshot);
    fixture.bridge.invoke.mockResolvedValueOnce(setting);
    const adapter = new TauriDashboardAdapter(fixture.bridge);

    await expect(adapter.setPlanRenewalAt("2027-02-03T04:34:56Z")).resolves.toEqual(setting);
    expect(fixture.bridge.invoke).toHaveBeenCalledWith(PLAN_RENEWAL_COMMAND, {
      planRenewalAt: "2027-02-03T04:34:56Z",
    });
  });

  it("reads a date-scoped local task detail only through the explicit command", async () => {
    const snapshot = createMockDashboardSnapshot("ready");
    const detail = { date: "2026-08-28", tasks: [], lowCacheTasks: 0 };
    const fixture = bridgeFixture(snapshot);
    fixture.bridge.invoke.mockResolvedValueOnce(detail);
    const adapter = new TauriDashboardAdapter(fixture.bridge);

    await expect(adapter.readUsageDayDetail("2026-08-28")).resolves.toEqual(detail);
    expect(fixture.bridge.invoke).toHaveBeenCalledWith(USAGE_DAY_DETAIL_COMMAND, { date: "2026-08-28" });
  });

  it("persists visual preferences only through the dedicated local commands", async () => {
    const snapshot = createMockDashboardSnapshot("ready");
    const preferences = { themePreference: "dark" as const, backgroundAssetPath: "C:\\app-data\\backgrounds\\background-1.png" };
    const fixture = bridgeFixture(snapshot);
    fixture.bridge.invoke
      .mockResolvedValueOnce(preferences)
      .mockResolvedValueOnce(preferences)
      .mockResolvedValueOnce(preferences)
      .mockResolvedValueOnce({ ...preferences, backgroundAssetPath: null });
    const adapter = new TauriDashboardAdapter(fixture.bridge);

    await expect(adapter.readVisualPreferences()).resolves.toEqual(preferences);
    await expect(adapter.setThemePreference("dark")).resolves.toEqual(preferences);
    await expect(adapter.selectBackgroundImage()).resolves.toEqual(preferences);
    await expect(adapter.clearBackgroundImage()).resolves.toEqual({ ...preferences, backgroundAssetPath: null });
    expect(fixture.bridge.invoke).toHaveBeenNthCalledWith(1, VISUAL_PREFERENCES_COMMAND);
    expect(fixture.bridge.invoke).toHaveBeenNthCalledWith(2, THEME_PREFERENCE_COMMAND, { themePreference: "dark" });
    expect(fixture.bridge.invoke).toHaveBeenNthCalledWith(3, SELECT_BACKGROUND_COMMAND);
    expect(fixture.bridge.invoke).toHaveBeenNthCalledWith(4, CLEAR_BACKGROUND_COMMAND);
  });

  it("persists runtime preferences and forwards verified refresh progress", async () => {
    const snapshot = createMockDashboardSnapshot("ready");
    const preferences = await new MockDashboardAdapter("ready", 0).readAppPreferences();
    const fixture = bridgeFixture(snapshot);
    fixture.bridge.invoke.mockResolvedValue(preferences);
    const adapter = new TauriDashboardAdapter(fixture.bridge);

    await expect(adapter.readAppPreferences()).resolves.toEqual(preferences);
    await expect(adapter.setAppPreferences(preferences)).resolves.toEqual(preferences);
    expect(fixture.bridge.invoke).toHaveBeenNthCalledWith(1, APP_PREFERENCES_COMMAND);
    expect(fixture.bridge.invoke).toHaveBeenNthCalledWith(2, SET_APP_PREFERENCES_COMMAND, expect.objectContaining({
      autostartEnabled: false,
      preferences: expect.objectContaining({ closeBehavior: "hideToTray", refreshIntervalMinutes: 1, notificationsEnabled: true }),
    }));

    const progressListener = vi.fn();
    await adapter.subscribeProgress(progressListener);
    expect(fixture.bridge.listen).toHaveBeenCalledWith(PROGRESS_EVENT, expect.any(Function));
    const progress = { phase: "sources", label: "正在索引 Claude Code（4/8 个文件）", processedSources: 1, totalSources: 6, currentSource: "Claude Code", processedFiles: 4, totalFiles: 8 } as const;
    fixture.emit(progress);
    expect(progressListener).toHaveBeenCalledWith(progress);

    const scheduleListener = vi.fn();
    await adapter.subscribeRefreshSchedule(scheduleListener);
    expect(fixture.bridge.listen).toHaveBeenCalledWith(REFRESH_SCHEDULE_EVENT, expect.any(Function));
    fixture.emit({ nextRefreshAtMs: 2_000, intervalSeconds: 300 });
    expect(scheduleListener).toHaveBeenCalledWith({ nextRefreshAtMs: 2_000, intervalSeconds: 300 });
  });

  it("round-trips validated project merge rules through dedicated commands", async () => {
    const snapshot = createMockDashboardSnapshot("ready");
    const rules = [{
      id: "work-log-history",
      displayName: "L.Q记工本",
      members: [
        { sourceId: "codex", sourceLabel: "Codex", projectKey: "L.Q记工本", projectLabel: "L.Q记工本" },
        { sourceId: "claude-code", sourceLabel: "Claude Code", projectKey: "旧记工本", projectLabel: "旧记工本" },
      ],
    }];
    const fixture = bridgeFixture(snapshot);
    fixture.bridge.invoke.mockResolvedValue(rules);
    const adapter = new TauriDashboardAdapter(fixture.bridge);

    await expect(adapter.readProjectMergeRules()).resolves.toEqual(rules);
    await expect(adapter.setProjectMergeRules(rules)).resolves.toEqual(rules);
    expect(fixture.bridge.invoke).toHaveBeenNthCalledWith(1, PROJECT_MERGE_RULES_COMMAND);
    expect(fixture.bridge.invoke).toHaveBeenNthCalledWith(2, SET_PROJECT_MERGE_RULES_COMMAND, { rules });
  });

  it("uses the dedicated command for a test notification", async () => {
    const snapshot = createMockDashboardSnapshot("ready");
    const preferences = await new MockDashboardAdapter("ready", 0).readAppPreferences();
    const sent = { ...preferences, lastNotificationAt: "2026-08-31T08:00:00Z", lastNotificationReason: "测试通知：系统通知链路可用" };
    const fixture = bridgeFixture(snapshot);
    fixture.bridge.invoke.mockResolvedValueOnce(sent);
    const adapter = new TauriDashboardAdapter(fixture.bridge);

    await expect(adapter.sendTestNotification()).resolves.toEqual(sent);
    expect(fixture.bridge.invoke).toHaveBeenCalledWith(TEST_NOTIFICATION_COMMAND);
  });

  it("normalizes index self-check and rebuild results through dedicated commands", async () => {
    const snapshot = createMockDashboardSnapshot("ready");
    const report = await new MockDashboardAdapter("ready", 0).readIndexMaintenanceReport();
    const rebuild = { backupCreatedAt: "2026-08-31T09:00:00Z", report, snapshot };
    const fixture = bridgeFixture(snapshot);
    fixture.bridge.invoke.mockResolvedValueOnce(report).mockResolvedValueOnce(rebuild);
    const adapter = new TauriDashboardAdapter(fixture.bridge);

    await expect(adapter.readIndexMaintenanceReport()).resolves.toEqual(report);
    await expect(adapter.rebuildIndexes()).resolves.toEqual(rebuild);
    expect(fixture.bridge.invoke).toHaveBeenNthCalledWith(1, INDEX_MAINTENANCE_COMMAND);
    expect(fixture.bridge.invoke).toHaveBeenNthCalledWith(2, REBUILD_INDEXES_COMMAND);
  });

  it("selects Tauri before demo mode and keeps browser development mocked", async () => {
    const snapshot = createMockDashboardSnapshot("ready");
    const fixture = bridgeFixture(snapshot);
    const desktop = createDashboardAdapter({
      tauri: true,
      development: true,
      search: "?demo=1",
      bridge: fixture.bridge,
    });
    expect(desktop).toBeInstanceOf(TauriDashboardAdapter);

    const browserDev = createDashboardAdapter({
      tauri: false,
      development: true,
      search: "",
    });
    expect(browserDev).toBeInstanceOf(MockDashboardAdapter);

    const productionBrowser = createDashboardAdapter({
      tauri: false,
      development: false,
      search: "",
    });
    await expect(productionBrowser.readSnapshot()).resolves.toMatchObject({
      status: "unsupported",
      quotaWindows: [],
      planRenewalAt: null,
    });
  });
});
