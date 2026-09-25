import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import type {
  AppPreferences,
  DashboardSnapshot,
  IndexMaintenanceReport,
  IndexRebuildResult,
  PlanRenewalSetting,
  PricingSettings,
  ProjectMergeRule,
  RefreshProgress,
  RefreshSchedule,
  ThemePreference,
  UsageDayDetail,
  VisualPreferences,
} from "../core/types";
import { createMockDashboardSnapshot, type MockDashboardState } from "./mock-data";

export const SNAPSHOT_COMMAND = "read_dashboard_snapshot";
export const CACHED_SNAPSHOT_COMMAND = "read_cached_dashboard_snapshot";
export const SNAPSHOT_EVENT = "usage://snapshot";
export const PROGRESS_EVENT = "usage://refresh-progress";
export const REFRESH_SCHEDULE_EVENT = "usage://refresh-schedule";
export const PLAN_RENEWAL_COMMAND = "set_plan_renewal_at";
export const USAGE_DAY_DETAIL_COMMAND = "read_usage_day_detail";
export const VISUAL_PREFERENCES_COMMAND = "read_visual_preferences";
export const THEME_PREFERENCE_COMMAND = "set_theme_preference";
export const SELECT_BACKGROUND_COMMAND = "select_background_image";
export const CLEAR_BACKGROUND_COMMAND = "clear_background_image";
export const APP_PREFERENCES_COMMAND = "read_app_preferences";
export const SET_APP_PREFERENCES_COMMAND = "set_app_preferences";
export const PROJECT_MERGE_RULES_COMMAND = "read_project_merge_rules";
export const SET_PROJECT_MERGE_RULES_COMMAND = "set_project_merge_rules";
export const PRICING_SETTINGS_COMMAND = "read_pricing_settings";
export const SET_PRICING_SETTINGS_COMMAND = "set_pricing_settings";
export const TEST_NOTIFICATION_COMMAND = "send_test_notification";
export const INDEX_MAINTENANCE_COMMAND = "read_index_maintenance_report";
export const REBUILD_INDEXES_COMMAND = "rebuild_indexes";

export type DashboardSnapshotListener = (snapshot: DashboardSnapshot) => void;
export type UnsubscribeDashboard = () => void;

export interface DashboardAdapter {
  /** Kept for compatibility with the phase-one adapter contract. */
  readSnapshot(): Promise<DashboardSnapshot>;
  /** Reads the last complete persisted snapshot without scanning source logs. */
  readCachedSnapshot?(): Promise<DashboardSnapshot | null>;
  /** Explicitly asks the backing data source to build a fresh snapshot. */
  refresh?(): Promise<DashboardSnapshot>;
  /** Receives complete snapshots pushed by the desktop backend. */
  subscribe?(listener: DashboardSnapshotListener): Promise<UnsubscribeDashboard>;
  subscribeProgress?(listener: (progress: RefreshProgress) => void): Promise<UnsubscribeDashboard>;
  subscribeRefreshSchedule?(listener: (schedule: RefreshSchedule) => void): Promise<UnsubscribeDashboard>;
  /** Stores or clears a user-entered renewal time in local app data. */
  setPlanRenewalAt?(value: string | null): Promise<PlanRenewalSetting>;
  /** Reads privacy-safe, local-only attribution for one calendar day. */
  readUsageDayDetail?(date: string): Promise<UsageDayDetail>;
  /** Reads visual preferences persisted in local application data. */
  readVisualPreferences?(): Promise<VisualPreferences>;
  /** Persists an explicit visual theme choice. */
  setThemePreference?(value: ThemePreference): Promise<VisualPreferences>;
  /** Opens the native image picker and stores an application-owned copy. */
  selectBackgroundImage?(): Promise<VisualPreferences>;
  /** Removes the current application-owned wallpaper copy. */
  clearBackgroundImage?(): Promise<VisualPreferences>;
  readAppPreferences?(): Promise<AppPreferences>;
  setAppPreferences?(value: AppPreferences): Promise<AppPreferences>;
  readProjectMergeRules?(): Promise<ProjectMergeRule[]>;
  setProjectMergeRules?(value: ProjectMergeRule[]): Promise<ProjectMergeRule[]>;
  readPricingSettings?(): Promise<PricingSettings>;
  setPricingSettings?(value: PricingSettings): Promise<PricingSettings>;
  sendTestNotification?(): Promise<AppPreferences>;
  readIndexMaintenanceReport?(): Promise<IndexMaintenanceReport>;
  rebuildIndexes?(): Promise<IndexRebuildResult>;
}

interface TauriSnapshotEvent {
  payload: unknown;
}

export interface TauriDashboardBridge {
  invoke(command: string, args?: Record<string, unknown>): Promise<unknown>;
  listen(
    event: string,
    handler: (event: TauriSnapshotEvent) => void,
  ): Promise<UnsubscribeDashboard>;
}

const DEFAULT_TAURI_BRIDGE: TauriDashboardBridge = {
  invoke: (command, args) => invoke(command, args),
  listen: (event, handler) => listen(event, handler),
};

const SNAPSHOT_STATUSES = new Set<DashboardSnapshot["status"]>([
  "ready",
  "loading",
  "offline",
  "unauthenticated",
  "unsupported",
  "error",
]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

const ACCOUNT_READ_METHODS = new Set([
  "account/read",
  "account/rateLimits/read",
  "account/usage/read",
]);

function normalizeAccountDiagnostics(value: unknown): DashboardSnapshot["accountDiagnostics"] {
  if (value === undefined || value === null) return null;
  if (
    !isRecord(value)
    || typeof value.readAt !== "string"
    || typeof value.durationMs !== "number"
    || !Number.isFinite(value.durationMs)
    || value.durationMs < 0
    || !Array.isArray(value.methods)
    || !value.methods.every((method) => typeof method === "string" && ACCOUNT_READ_METHODS.has(method))
  ) {
    throw new Error("桌面端返回了无法识别的账号读取诊断。");
  }
  return {
    readAt: value.readAt,
    durationMs: value.durationMs,
    methods: [...value.methods],
  };
}

function normalizeDeviceUsage(value: unknown): DashboardSnapshot["deviceUsage"] {
  if (value === undefined || value === null) return null;
  if (!isRecord(value)) throw new Error("桌面端返回了无法识别的本机用量数据。");
  const rawCatalog = value.priceCatalog;
  if (!(rawCatalog === undefined || Array.isArray(rawCatalog))) {
    throw new Error("桌面端返回了无法识别的价格目录。");
  }
  const priceCatalog = (rawCatalog ?? []).map((entry) => {
    if (
      !isRecord(entry)
      || typeof entry.displayName !== "string"
      || typeof entry.modelId !== "string"
      || !Array.isArray(entry.aliases)
      || !entry.aliases.every((alias) => typeof alias === "string")
      || (entry.currency !== "USD" && entry.currency !== "CNY")
      || typeof entry.sourceLabel !== "string"
      || !Array.isArray(entry.tiers)
    ) throw new Error("桌面端返回了无法识别的价格目录条目。");
    const tiers = entry.tiers.map((tier) => {
      if (!isRecord(tier)) throw new Error("桌面端返回了无法识别的价格阶梯。");
      const optionalRate = (rate: unknown) => rate === null || (typeof rate === "number" && Number.isFinite(rate));
      if (
        typeof tier.label !== "string"
        || typeof tier.condition !== "string"
        || !optionalRate(tier.inputPerMillion)
        || !optionalRate(tier.cachedInputPerMillion)
        || !optionalRate(tier.cacheWritePerMillion)
        || !optionalRate(tier.outputPerMillion)
      ) throw new Error("桌面端返回了无法识别的价格阶梯。");
      return tier;
    });
    return { ...entry, tiers };
  });
  return { ...value, priceCatalog } as unknown as NonNullable<DashboardSnapshot["deviceUsage"]>;
}

/**
 * Applies a small IPC boundary check before data reaches React. It deliberately
 * does not synthesize quota data when the backend response is unavailable or
 * malformed. `planRenewalAt` remains nullable for older backends and because
 * App Server currently has no subscription-renewal field.
 */
export function normalizeDashboardSnapshot(value: unknown): DashboardSnapshot {
  if (
    !isRecord(value) ||
    typeof value.status !== "string" ||
    !SNAPSHOT_STATUSES.has(value.status as DashboardSnapshot["status"]) ||
    !(value.fetchedAt === null || typeof value.fetchedAt === "string") ||
    !(value.codexVersion === null || typeof value.codexVersion === "string") ||
    !Array.isArray(value.quotaWindows) ||
    !(value.deviceUsage === undefined || value.deviceUsage === null || isRecord(value.deviceUsage)) ||
    !(value.message === null || typeof value.message === "string")
  ) {
    throw new Error("桌面端返回了无法识别的用量快照。");
  }

  const renewal = value.planRenewalAt;
  const planRenewalAt = typeof renewal === "number" && Number.isFinite(renewal)
    ? renewal
    : typeof renewal === "string" && Number.isFinite(new Date(renewal).getTime())
      ? renewal
      : null;
  const planRenewalSource = value.planRenewalSource === "manual" || value.planRenewalSource === "official"
    ? value.planRenewalSource
    : null;
  const refreshSchedule = normalizeRefreshSchedule(value.refreshSchedule);
  const accountDiagnostics = normalizeAccountDiagnostics(value.accountDiagnostics);
  const deviceUsage = normalizeDeviceUsage(value.deviceUsage);

  return {
    ...(value as unknown as DashboardSnapshot),
    planRenewalAt,
    planRenewalSource,
    refreshSchedule,
    accountDiagnostics,
    deviceUsage,
  };
}

export function normalizeRefreshSchedule(value: unknown): RefreshSchedule | null {
  if (value === undefined || value === null) return null;
  if (!isRecord(value)) throw new Error("桌面端返回了无法识别的刷新计划。");
  const nextRefreshAtMs = value.nextRefreshAtMs;
  const intervalSeconds = value.intervalSeconds;
  if (
    !(nextRefreshAtMs === null || (typeof nextRefreshAtMs === "number" && Number.isFinite(nextRefreshAtMs)))
    || (intervalSeconds !== 60 && intervalSeconds !== 300 && intervalSeconds !== 900)
  ) {
    throw new Error("桌面端返回了无法识别的刷新计划。");
  }
  return { nextRefreshAtMs, intervalSeconds };
}

function normalizePlanRenewalSetting(value: unknown): PlanRenewalSetting {
  if (!isRecord(value)) throw new Error("桌面端返回了无法识别的续费时间设置。");
  const renewal = value.planRenewalAt;
  const planRenewalAt = typeof renewal === "string" && Number.isFinite(new Date(renewal).getTime())
    ? renewal
    : renewal === null
      ? null
      : undefined;
  if (planRenewalAt === undefined) throw new Error("桌面端返回了无法识别的续费时间设置。");
  return {
    planRenewalAt,
    planRenewalSource: planRenewalAt === null ? null : "manual",
  };
}

function normalizeVisualPreferences(value: unknown): VisualPreferences {
  if (!isRecord(value)) throw new Error("桌面端返回了无法识别的视觉设置。");
  const themePreference = value.themePreference;
  const backgroundAssetPath = value.backgroundAssetPath;
  if (
    (themePreference !== "system" && themePreference !== "light" && themePreference !== "dark")
    || !(backgroundAssetPath === null || typeof backgroundAssetPath === "string")
  ) {
    throw new Error("桌面端返回了无法识别的视觉设置。");
  }
  return { themePreference, backgroundAssetPath };
}

function normalizeAppPreferences(value: unknown): AppPreferences {
  const visual = normalizeVisualPreferences(value);
  if (!isRecord(value)) throw new Error("桌面端返回了无法识别的应用设置。");
  const refreshIntervalMinutes = value.refreshIntervalMinutes;
  const closeBehavior = value.closeBehavior;
  if (
    (refreshIntervalMinutes !== 1 && refreshIntervalMinutes !== 5 && refreshIntervalMinutes !== 15)
    || (closeBehavior !== "hideToTray" && closeBehavior !== "exit")
    || typeof value.autostartEnabled !== "boolean"
    || typeof value.quotaWarningPercent !== "number"
    || typeof value.cacheWarningPercent !== "number"
    || typeof value.notificationsEnabled !== "boolean"
    || !(value.lastNotificationAt === null || typeof value.lastNotificationAt === "string")
    || !(value.lastNotificationReason === null || typeof value.lastNotificationReason === "string")
    || typeof value.quietHoursEnabled !== "boolean"
    || typeof value.quietHoursStart !== "string"
    || typeof value.quietHoursEnd !== "string"
  ) {
    throw new Error("桌面端返回了无法识别的应用设置。");
  }
  return { ...visual, ...value } as AppPreferences;
}

function normalizeIndexMaintenanceReport(value: unknown): IndexMaintenanceReport {
  if (
    !isRecord(value)
    || typeof value.checkedAt !== "string"
    || !["healthy", "warning", "error"].includes(String(value.overallStatus))
    || !Array.isArray(value.databases)
    || !(value.lastBackupAt === null || typeof value.lastBackupAt === "string")
    || typeof value.message !== "string"
  ) {
    throw new Error("桌面端返回了无法识别的索引自检结果。");
  }
  const databases = value.databases.map((database) => {
    if (
      !isRecord(database)
      || typeof database.label !== "string"
      || !["healthy", "missing", "error"].includes(String(database.status))
      || !["ok", "failed", "unavailable"].includes(String(database.integrity))
      || typeof database.sizeBytes !== "number"
      || !(database.schemaVersion === null || typeof database.schemaVersion === "number")
      || typeof database.expectedSchemaVersion !== "number"
    ) {
      throw new Error("桌面端返回了无法识别的索引数据库状态。");
    }
    return database;
  });
  return { ...value, databases } as unknown as IndexMaintenanceReport;
}

export function normalizeProjectMergeRules(value: unknown): ProjectMergeRule[] {
  if (!Array.isArray(value)) throw new Error("桌面端返回了无法识别的项目合并设置。");
  const assigned = new Set<string>();
  return value.map((rule) => {
    if (!isRecord(rule) || typeof rule.id !== "string" || typeof rule.displayName !== "string" || !Array.isArray(rule.members)) {
      throw new Error("桌面端返回了无法识别的项目合并设置。");
    }
    const members = rule.members.map((member) => {
      if (
        !isRecord(member)
        || !["codex", "claude-code", "opencode", "workbuddy", "workbuddy-ai", "cursor"].includes(String(member.sourceId))
        || typeof member.sourceLabel !== "string"
        || typeof member.projectKey !== "string"
        || typeof member.projectLabel !== "string"
      ) {
        throw new Error("桌面端返回了无法识别的项目合并设置。");
      }
      const identity = `${member.sourceId}\u0000${member.projectKey}`;
      if (assigned.has(identity)) throw new Error("桌面端返回了互相冲突的项目合并设置。");
      assigned.add(identity);
      return member as unknown as ProjectMergeRule["members"][number];
    });
    if (members.length < 2 || !members.some((member) => member.projectLabel === rule.displayName)) {
      throw new Error("桌面端返回了无法识别的项目合并设置。");
    }
    return { id: rule.id, displayName: rule.displayName, members };
  });
}

export function normalizePricingSettings(value: unknown): PricingSettings {
  if (!isRecord(value) || !isRecord(value.peak) || !Array.isArray(value.models)) {
    throw new Error("桌面端返回了无法识别的模型定价设置。");
  }
  const peak = value.peak;
  if (
    !(value.updatedAt === null || typeof value.updatedAt === "string")
    || typeof peak.startTime !== "string"
    || typeof peak.endTime !== "string"
    || typeof peak.multiplier !== "number"
    || !Number.isFinite(peak.multiplier)
    || peak.multiplier < 1
  ) throw new Error("桌面端返回了无法识别的高峰期设置。");
  const optionalRate = (rate: unknown): rate is number | null => rate === null
    || (typeof rate === "number" && Number.isFinite(rate) && rate >= 0);
  const models = value.models.map((model) => {
    if (
      !isRecord(model)
      || typeof model.modelId !== "string"
      || typeof model.displayName !== "string"
      || (model.currency !== "USD" && model.currency !== "CNY")
      || !optionalRate(model.inputPerMillion)
      || !optionalRate(model.cachedInputPerMillion)
      || !optionalRate(model.cacheWritePerMillion)
      || !optionalRate(model.outputPerMillion)
      || typeof model.peakEnabled !== "boolean"
    ) throw new Error("桌面端返回了无法识别的模型单价。");
    return model as unknown as PricingSettings["models"][number];
  });
  return {
    updatedAt: value.updatedAt,
    peak: {
      startTime: peak.startTime,
      endTime: peak.endTime,
      multiplier: peak.multiplier,
    },
    models,
  };
}

function normalizeIndexRebuildResult(value: unknown): IndexRebuildResult {
  if (!isRecord(value) || !(value.backupCreatedAt === null || typeof value.backupCreatedAt === "string")) {
    throw new Error("桌面端返回了无法识别的索引重建结果。");
  }
  return {
    backupCreatedAt: value.backupCreatedAt,
    report: normalizeIndexMaintenanceReport(value.report),
    snapshot: normalizeDashboardSnapshot(value.snapshot),
  };
}

function normalizeProgress(value: unknown): RefreshProgress {
  if (
    !isRecord(value)
    || !["idle", "codex", "sources", "account", "complete"].includes(String(value.phase))
    || typeof value.label !== "string"
    || typeof value.processedSources !== "number"
    || typeof value.totalSources !== "number"
    || !(value.currentSource === null || typeof value.currentSource === "string")
    || typeof value.processedFiles !== "number"
    || typeof value.totalFiles !== "number"
  ) {
    throw new Error("桌面端返回了无法识别的刷新进度。");
  }
  return value as unknown as RefreshProgress;
}

export class TauriDashboardAdapter implements DashboardAdapter {
  constructor(private readonly bridge: TauriDashboardBridge = DEFAULT_TAURI_BRIDGE) {}

  async refresh(): Promise<DashboardSnapshot> {
    const value = await this.bridge.invoke(SNAPSHOT_COMMAND);
    return normalizeDashboardSnapshot(value);
  }

  async readSnapshot(): Promise<DashboardSnapshot> {
    return this.refresh();
  }

  async readCachedSnapshot(): Promise<DashboardSnapshot | null> {
    const value = await this.bridge.invoke(CACHED_SNAPSHOT_COMMAND);
    return value === null ? null : normalizeDashboardSnapshot(value);
  }

  async setPlanRenewalAt(value: string | null): Promise<PlanRenewalSetting> {
    const setting = await this.bridge.invoke(PLAN_RENEWAL_COMMAND, {
      planRenewalAt: value,
    });
    return normalizePlanRenewalSetting(setting);
  }

  async readUsageDayDetail(date: string): Promise<UsageDayDetail> {
    const value = await this.bridge.invoke(USAGE_DAY_DETAIL_COMMAND, { date });
    if (!isRecord(value) || typeof value.date !== "string" || !Array.isArray(value.tasks) || typeof value.lowCacheTasks !== "number") {
      throw new Error("桌面端返回了无法识别的日用量明细。");
    }
    return value as unknown as UsageDayDetail;
  }

  async readVisualPreferences(): Promise<VisualPreferences> {
    return normalizeVisualPreferences(await this.bridge.invoke(VISUAL_PREFERENCES_COMMAND));
  }

  async setThemePreference(value: ThemePreference): Promise<VisualPreferences> {
    return normalizeVisualPreferences(await this.bridge.invoke(THEME_PREFERENCE_COMMAND, {
      themePreference: value,
    }));
  }

  async selectBackgroundImage(): Promise<VisualPreferences> {
    return normalizeVisualPreferences(await this.bridge.invoke(SELECT_BACKGROUND_COMMAND));
  }

  async clearBackgroundImage(): Promise<VisualPreferences> {
    return normalizeVisualPreferences(await this.bridge.invoke(CLEAR_BACKGROUND_COMMAND));
  }

  async readAppPreferences(): Promise<AppPreferences> {
    return normalizeAppPreferences(await this.bridge.invoke(APP_PREFERENCES_COMMAND));
  }

  async setAppPreferences(value: AppPreferences): Promise<AppPreferences> {
    const preferences = {
      refreshIntervalMinutes: value.refreshIntervalMinutes,
      closeBehavior: value.closeBehavior,
      quotaWarningPercent: value.quotaWarningPercent,
      cacheWarningPercent: value.cacheWarningPercent,
      notificationsEnabled: value.notificationsEnabled,
      quietHoursEnabled: value.quietHoursEnabled,
      quietHoursStart: value.quietHoursStart,
      quietHoursEnd: value.quietHoursEnd,
    };
    return normalizeAppPreferences(await this.bridge.invoke(SET_APP_PREFERENCES_COMMAND, {
      preferences,
      autostartEnabled: value.autostartEnabled,
    }));
  }

  async readProjectMergeRules(): Promise<ProjectMergeRule[]> {
    return normalizeProjectMergeRules(await this.bridge.invoke(PROJECT_MERGE_RULES_COMMAND));
  }

  async setProjectMergeRules(value: ProjectMergeRule[]): Promise<ProjectMergeRule[]> {
    return normalizeProjectMergeRules(await this.bridge.invoke(SET_PROJECT_MERGE_RULES_COMMAND, { rules: value }));
  }

  async readPricingSettings(): Promise<PricingSettings> {
    return normalizePricingSettings(await this.bridge.invoke(PRICING_SETTINGS_COMMAND));
  }

  async setPricingSettings(value: PricingSettings): Promise<PricingSettings> {
    return normalizePricingSettings(await this.bridge.invoke(SET_PRICING_SETTINGS_COMMAND, {
      pricingSettings: value,
    }));
  }

  async sendTestNotification(): Promise<AppPreferences> {
    return normalizeAppPreferences(await this.bridge.invoke(TEST_NOTIFICATION_COMMAND));
  }

  async readIndexMaintenanceReport(): Promise<IndexMaintenanceReport> {
    return normalizeIndexMaintenanceReport(await this.bridge.invoke(INDEX_MAINTENANCE_COMMAND));
  }

  async rebuildIndexes(): Promise<IndexRebuildResult> {
    return normalizeIndexRebuildResult(await this.bridge.invoke(REBUILD_INDEXES_COMMAND));
  }

  async subscribe(listener: DashboardSnapshotListener): Promise<UnsubscribeDashboard> {
    return this.bridge.listen(SNAPSHOT_EVENT, (event) => {
      try {
        listener(normalizeDashboardSnapshot(event.payload));
      } catch {
        // Ignore malformed pushes. The last verified snapshot stays visible and
        // the user can still request a fresh snapshot with the refresh button.
      }
    });
  }

  async subscribeProgress(listener: (progress: RefreshProgress) => void): Promise<UnsubscribeDashboard> {
    return this.bridge.listen(PROGRESS_EVENT, (event) => {
      try {
        listener(normalizeProgress(event.payload));
      } catch {
        // Keep the last verified progress state.
      }
    });
  }

  async subscribeRefreshSchedule(listener: (schedule: RefreshSchedule) => void): Promise<UnsubscribeDashboard> {
    return this.bridge.listen(REFRESH_SCHEDULE_EVENT, (event) => {
      const schedule = normalizeRefreshSchedule(event.payload);
      if (schedule) listener(schedule);
    });
  }
}

class UnavailableBridgeAdapter implements DashboardAdapter {
  async refresh(): Promise<DashboardSnapshot> {
    return {
      status: "unsupported",
      fetchedAt: new Date().toISOString(),
      codexVersion: null,
      quotaWindows: [],
      accountDiagnostics: null,
      planRenewalAt: null,
      planRenewalSource: null,
      resetCredits: null,
      accountUsage: null,
      localUsage: null,
      deviceUsage: null,
      message: "当前运行环境没有连接桌面数据桥；不会显示或伪造账号用量。",
    };
  }

  async readSnapshot(): Promise<DashboardSnapshot> {
    return this.refresh();
  }
}

function createMockUsageDayDetail(date: string): UsageDayDetail {
  const usage = createMockDashboardSnapshot("ready").deviceUsage?.today;
  if (!usage) return { date, tasks: [], lowCacheTasks: 0 };
  const firstUsage = Object.fromEntries(
    Object.entries(usage).map(([key, value]) => [key, Math.round(value * 0.62)]),
  ) as unknown as typeof usage;
  const secondUsage = Object.fromEntries(
    Object.entries(usage).map(([key, value]) => [key, Math.max(0, value - firstUsage[key as keyof typeof usage])]),
  ) as unknown as typeof usage;
  return {
    date,
    lowCacheTasks: 1,
    tasks: [
      { occurredAtMs: Date.parse(`${date}T10:24:00`), source: "Codex", modelLabel: "gpt-5.6-sol", projectLabel: "本机项目", taskLabel: "实现与验证应用升级", usage: firstUsage, cacheMetricsAvailable: true, cacheHitRate: 0.94, lowCacheHit: false, cost: { usd: 0.42, cny: 0, unpricedTokens: 0 } },
      { occurredAtMs: Date.parse(`${date}T15:42:00`), source: "Claude Code", modelLabel: "Claude Sonnet", projectLabel: "本机项目", taskLabel: "界面复查", usage: secondUsage, cacheMetricsAvailable: true, cacheHitRate: 0.73, lowCacheHit: true, cost: { usd: 0, cny: 0, unpricedTokens: secondUsage.totalTokens } },
    ],
  };
}

export class MockDashboardAdapter implements DashboardAdapter {
  private manualPlanRenewalAt: string | null = null;
  private visualPreferences: VisualPreferences;
  private appPreferences: AppPreferences;
  private projectMergeRules: ProjectMergeRule[] = [];
  private pricingSettings: PricingSettings = {
    updatedAt: "2026-09-25T09:30:00Z",
    peak: { startTime: "18:00", endTime: "23:00", multiplier: 1.5 },
    models: [
      { modelId: "GPT-5.6 Sol", displayName: "GPT-5.6 Sol", currency: "USD", inputPerMillion: 4, cachedInputPerMillion: 0.4, cacheWritePerMillion: 5, outputPerMillion: 20, peakEnabled: true },
      { modelId: "默认模型", displayName: "默认模型", currency: "USD", inputPerMillion: null, cachedInputPerMillion: null, cacheWritePerMillion: null, outputPerMillion: null, peakEnabled: false },
    ],
  };

  constructor(
    private readonly state: MockDashboardState = "ready",
    private readonly delayMilliseconds = 280,
    themePreference: ThemePreference = "system",
  ) {
    this.visualPreferences = { themePreference, backgroundAssetPath: null };
    this.appPreferences = {
      ...this.visualPreferences,
      refreshIntervalMinutes: 1,
      closeBehavior: "hideToTray",
      autostartEnabled: false,
      quotaWarningPercent: 20,
      cacheWarningPercent: 90,
      notificationsEnabled: true,
      lastNotificationAt: null,
      lastNotificationReason: null,
      quietHoursEnabled: false,
      quietHoursStart: "22:00",
      quietHoursEnd: "08:00",
    };
  }

  async refresh(): Promise<DashboardSnapshot> {
    if (this.delayMilliseconds > 0) {
      await new Promise<void>((resolve) => {
        globalThis.setTimeout(resolve, this.delayMilliseconds);
      });
    }
    const snapshot = createMockDashboardSnapshot(this.state);
    return {
      ...snapshot,
      planRenewalAt: this.manualPlanRenewalAt,
      planRenewalSource: this.manualPlanRenewalAt === null ? null : "manual",
    };
  }

  async readSnapshot(): Promise<DashboardSnapshot> {
    return this.refresh();
  }

  async readUsageDayDetail(date: string): Promise<UsageDayDetail> {
    return createMockUsageDayDetail(date);
  }

  async setPlanRenewalAt(value: string | null): Promise<PlanRenewalSetting> {
    this.manualPlanRenewalAt = value;
    return {
      planRenewalAt: value,
      planRenewalSource: value === null ? null : "manual",
    };
  }

  async readVisualPreferences(): Promise<VisualPreferences> {
    return this.visualPreferences;
  }

  async setThemePreference(value: ThemePreference): Promise<VisualPreferences> {
    this.visualPreferences = { ...this.visualPreferences, themePreference: value };
    this.appPreferences = { ...this.appPreferences, themePreference: value };
    return this.visualPreferences;
  }

  async selectBackgroundImage(): Promise<VisualPreferences> {
    return this.visualPreferences;
  }

  async clearBackgroundImage(): Promise<VisualPreferences> {
    this.visualPreferences = { ...this.visualPreferences, backgroundAssetPath: null };
    this.appPreferences = { ...this.appPreferences, backgroundAssetPath: null };
    return this.visualPreferences;
  }

  async readAppPreferences(): Promise<AppPreferences> {
    return this.appPreferences;
  }

  async setAppPreferences(value: AppPreferences): Promise<AppPreferences> {
    this.appPreferences = { ...value };
    this.visualPreferences = {
      themePreference: value.themePreference,
      backgroundAssetPath: value.backgroundAssetPath,
    };
    return this.appPreferences;
  }

  async readProjectMergeRules(): Promise<ProjectMergeRule[]> {
    return this.projectMergeRules;
  }

  async setProjectMergeRules(value: ProjectMergeRule[]): Promise<ProjectMergeRule[]> {
    this.projectMergeRules = normalizeProjectMergeRules(value);
    return this.projectMergeRules;
  }

  async readPricingSettings(): Promise<PricingSettings> {
    return this.pricingSettings;
  }

  async setPricingSettings(value: PricingSettings): Promise<PricingSettings> {
    this.pricingSettings = normalizePricingSettings({
      ...value,
      updatedAt: new Date().toISOString(),
    });
    return this.pricingSettings;
  }

  async sendTestNotification(): Promise<AppPreferences> {
    this.appPreferences = {
      ...this.appPreferences,
      lastNotificationAt: new Date().toISOString(),
      lastNotificationReason: "测试通知：系统通知链路可用",
    };
    return this.appPreferences;
  }

  async readIndexMaintenanceReport(): Promise<IndexMaintenanceReport> {
    return createMockIndexMaintenanceReport();
  }

  async rebuildIndexes(): Promise<IndexRebuildResult> {
    const backupCreatedAt = new Date().toISOString();
    return {
      backupCreatedAt,
      report: { ...createMockIndexMaintenanceReport(), lastBackupAt: backupCreatedAt },
      snapshot: await this.refresh(),
    };
  }
}

function createMockIndexMaintenanceReport(): IndexMaintenanceReport {
  return {
    checkedAt: new Date().toISOString(),
    overallStatus: "healthy",
    databases: [
      { label: "Codex 索引", status: "healthy", integrity: "ok", sizeBytes: 18_624_512, schemaVersion: 2, expectedSchemaVersion: 2 },
      { label: "多来源索引", status: "healthy", integrity: "ok", sizeBytes: 7_340_032, schemaVersion: 3, expectedSchemaVersion: 3 },
    ],
    lastBackupAt: null,
    message: "两套派生索引均可读取，完整性和版本检查通过。",
  };
}

function stateFromQuery(search: string): MockDashboardState {
  const requestedState = new URLSearchParams(search).get("state");
  const supportedStates: MockDashboardState[] = [
    "ready",
    "loading",
    "offline",
    "unauthenticated",
    "unsupported",
    "error",
    "stale",
  ];
  return supportedStates.includes(requestedState as MockDashboardState)
    ? (requestedState as MockDashboardState)
    : "ready";
}

export interface DashboardAdapterOptions {
  tauri?: boolean;
  development?: boolean;
  search?: string;
  bridge?: TauriDashboardBridge;
}

export function createDashboardAdapter(options: DashboardAdapterOptions = {}): DashboardAdapter {
  const runningInTauri = options.tauri ?? isTauri();
  if (runningInTauri) return new TauriDashboardAdapter(options.bridge);

  const search = options.search ?? (typeof window === "undefined" ? "" : window.location.search);
  const query = new URLSearchParams(search);
  const demoRequested = query.get("demo") === "1";
  const development = options.development ?? import.meta.env.DEV;
  const previewTheme = query.get("previewTheme");
  const themePreference: ThemePreference = previewTheme === "light" || previewTheme === "dark"
    ? previewTheme
    : "system";

  // Vite's browser development server remains a clearly labelled mock. A
  // production browser build shows an unsupported state unless demo=1 is
  // explicitly requested; neither path impersonates a live desktop bridge.
  if (development || demoRequested) {
    return new MockDashboardAdapter(stateFromQuery(search), 280, themePreference);
  }
  return new UnavailableBridgeAdapter();
}

export const dashboardAdapter = createDashboardAdapter();
