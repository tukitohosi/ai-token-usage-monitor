import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import { convertFileSrc, isTauri } from "@tauri-apps/api/core";

import type {
  AppPreferences,
  DashboardSnapshot,
  IndexMaintenanceReport,
  ProjectMergeRule,
  RefreshProgress,
  ThemePreference,
  UsageDayDetail,
  UsageRange,
  UsageSourceSummary,
} from "./core/types";
import { dashboardAdapter, type DashboardAdapter } from "./ui/dashboard-adapter";
import { aggregateSelectedSources, attachCustomRanges, toggleSourceSelection, type SourceSelection, type SourceSelectionMode } from "./ui/device-usage-view-model";
import {
  ActivityView,
  CostView,
  Icon,
  OverviewView,
  SettingsView,
  StatusNotice,
  StatusPopover,
  UsageDayDrawer,
  type AppView,
  type IconName,
} from "./ui/dashboard-views";
import { formatRefreshCountdown, isSnapshotStale } from "./ui/format";

const INITIAL_SNAPSHOT: DashboardSnapshot = {
  status: "loading",
  fetchedAt: null,
  codexVersion: null,
  quotaWindows: [],
  accountDiagnostics: null,
  planRenewalAt: null,
  planRenewalSource: null,
  resetCredits: null,
  accountUsage: null,
  deviceUsage: null,
  lastSuccessfulAt: null,
  refreshProgress: null,
  sourceHealth: [],
  message: "正在读取 Codex 用量…",
};

const DEFAULT_PREFERENCES: AppPreferences = {
  themePreference: "system",
  backgroundAssetPath: null,
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

function runtimePreferencesEqual(left: AppPreferences, right: AppPreferences): boolean {
  return left.refreshIntervalMinutes === right.refreshIntervalMinutes
    && left.closeBehavior === right.closeBehavior
    && left.autostartEnabled === right.autostartEnabled
    && left.quotaWarningPercent === right.quotaWarningPercent
    && left.cacheWarningPercent === right.cacheWarningPercent
    && left.notificationsEnabled === right.notificationsEnabled
    && left.quietHoursEnabled === right.quietHoursEnabled
    && left.quietHoursStart === right.quietHoursStart
    && left.quietHoursEnd === right.quietHoursEnd;
}

const VIEW_ITEMS: Array<{ id: AppView; label: string; icon: IconName }> = [
  { id: "overview", label: "总览", icon: "overview" },
  { id: "activity", label: "本机活动", icon: "activity" },
  { id: "cost", label: "费用", icon: "wallet" },
  { id: "settings", label: "设置", icon: "settings" },
];

interface AppProps {
  adapter?: DashboardAdapter;
}

function useClock(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const interval = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(interval);
  }, []);
  return now;
}

function useSystemTheme(): "light" | "dark" {
  const getTheme = () => window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  const [theme, setTheme] = useState<"light" | "dark">(getTheme);
  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const update = () => setTheme(media.matches ? "dark" : "light");
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);
  return theme;
}

function readDevelopmentWallpaperUrl(): string | null {
  if (!import.meta.env.DEV) return null;
  const value = new URLSearchParams(window.location.search).get("previewWallpaper");
  if (!value) return null;
  try {
    const url = new URL(value);
    return url.protocol === "http:" && url.hostname === "127.0.0.1" ? url.toString() : null;
  } catch {
    return null;
  }
}

function localDateInput(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function enumerateLocalDates(start: string, end: string): string[] {
  const first = new Date(`${start}T12:00:00`);
  const last = new Date(`${end}T12:00:00`);
  if (!Number.isFinite(first.getTime()) || !Number.isFinite(last.getTime()) || first > last) return [];
  const dates: string[] = [];
  for (const cursor = new Date(first); cursor <= last && dates.length <= 31; cursor.setDate(cursor.getDate() + 1)) {
    dates.push(localDateInput(cursor));
  }
  return dates;
}

export default function App({ adapter = dashboardAdapter }: AppProps) {
  const [snapshot, setSnapshot] = useState<DashboardSnapshot>(INITIAL_SNAPSHOT);
  const [view, setView] = useState<AppView>("overview");
  const [range, setRange] = useState<UsageRange>("today");
  const [activitySources, setActivitySources] = useState<SourceSelection>([]);
  const [costSources, setCostSources] = useState<SourceSelection>([]);
  const [activitySourceMode, setActivitySourceMode] = useState<SourceSelectionMode>("single");
  const [costSourceMode, setCostSourceMode] = useState<SourceSelectionMode>("single");
  const [customStartDate, setCustomStartDate] = useState(() => {
    const date = new Date();
    date.setDate(date.getDate() - 6);
    return localDateInput(date);
  });
  const [customEndDate, setCustomEndDate] = useState(() => localDateInput(new Date()));
  const [appliedCustomStartDate, setAppliedCustomStartDate] = useState(customStartDate);
  const [appliedCustomEndDate, setAppliedCustomEndDate] = useState(customEndDate);
  const [customSources, setCustomSources] = useState<UsageSourceSummary[] | null>(null);
  const [isLoadingCustomRange, setIsLoadingCustomRange] = useState(false);
  const [customRangeError, setCustomRangeError] = useState<string | null>(null);
  const [preferences, setPreferences] = useState<AppPreferences>(DEFAULT_PREFERENCES);
  const [savedPreferences, setSavedPreferences] = useState<AppPreferences>(DEFAULT_PREFERENCES);
  const [pendingView, setPendingView] = useState<AppView | null>(null);
  const [progress, setProgress] = useState<RefreshProgress | null>(null);
  const [isRefreshing, setIsRefreshing] = useState(true);
  const [statusOpen, setStatusOpen] = useState(false);
  const [settingsMessage, setSettingsMessage] = useState<string | null>(null);
  const [isSavingSettings, setIsSavingSettings] = useState(false);
  const [indexMaintenance, setIndexMaintenance] = useState<IndexMaintenanceReport | null>(null);
  const [isMaintainingIndexes, setIsMaintainingIndexes] = useState(false);
  const [successToast, setSuccessToast] = useState<string | null>(null);
  const [selectedDay, setSelectedDay] = useState<string | null>(null);
  const [dayDetail, setDayDetail] = useState<UsageDayDetail | null>(null);
  const [dayDetailError, setDayDetailError] = useState<string | null>(null);
  const [isDayDetailLoading, setIsDayDetailLoading] = useState(false);
  const [projectMergeRules, setProjectMergeRules] = useState<ProjectMergeRule[]>([]);
  const [projectMergeMessage, setProjectMergeMessage] = useState<string | null>(null);
  const requestSequence = useRef(0);
  const now = useClock();
  const systemTheme = useSystemTheme();

  const refresh = useCallback(async () => {
    const requestId = ++requestSequence.current;
    setIsRefreshing(true);
    setProgress({ phase: "codex", label: "正在读取本机与账号用量", processedSources: 0, totalSources: 6, currentSource: "Codex", processedFiles: 0, totalFiles: 0 });
    try {
      const next = adapter.refresh ? await adapter.refresh() : await adapter.readSnapshot();
      if (requestSequence.current === requestId) {
        setSnapshot(next);
        setProgress(next.refreshProgress ?? { phase: "complete", label: "刷新完成", processedSources: 6, totalSources: 6, currentSource: null, processedFiles: 0, totalFiles: 0 });
      }
    } catch {
      if (requestSequence.current === requestId) {
        setSnapshot((current) => ({ ...current, status: "error", message: "无法更新真实用量数据，请稍后重试。" }));
      }
    } finally {
      if (requestSequence.current === requestId) setIsRefreshing(false);
    }
  }, [adapter]);

  useEffect(() => {
    let disposed = false;
    const stops: Array<() => void> = [];
    if (adapter.subscribe) {
      void adapter.subscribe((next) => {
        if (disposed) return;
        requestSequence.current += 1;
        setSnapshot(next);
        setProgress(next.refreshProgress ?? null);
        setIsRefreshing(false);
      }).then((stop) => disposed ? stop() : stops.push(stop)).catch(() => {
        if (!disposed) setSnapshot((current) => ({ ...current, message: "后台推送通道暂不可用，可继续手动刷新。" }));
      });
    }
    if (adapter.subscribeProgress) {
      void adapter.subscribeProgress((next) => {
        if (!disposed) {
          setProgress(next);
          setIsRefreshing(next.phase !== "complete");
        }
      }).then((stop) => disposed ? stop() : stops.push(stop)).catch(() => {
        // Progress events are optional; the completed snapshot remains authoritative.
      });
    }
    if (adapter.subscribeRefreshSchedule) {
      void adapter.subscribeRefreshSchedule((refreshSchedule) => {
        if (!disposed) setSnapshot((current) => ({ ...current, refreshSchedule }));
      }).then((stop) => disposed ? stop() : stops.push(stop)).catch(() => {
        // Completed snapshots still carry the authoritative refresh schedule.
      });
    }
    const start = async () => {
      if (adapter.readCachedSnapshot) {
        try {
          const cached = await adapter.readCachedSnapshot();
          if (!disposed && cached) {
            setSnapshot(cached);
            setProgress(cached.refreshProgress ?? null);
          }
        } catch {
          // A missing or damaged derived cache must never block a fresh refresh.
        }
      }
      if (!disposed) void refresh();
    };
    void start();
    return () => {
      disposed = true;
      requestSequence.current += 1;
      stops.forEach((stop) => stop());
    };
  }, [adapter, refresh]);

  useEffect(() => {
    if (!successToast) return;
    const timeout = window.setTimeout(() => setSuccessToast(null), 4_000);
    return () => window.clearTimeout(timeout);
  }, [successToast]);

  useEffect(() => {
    let disposed = false;
    const read = adapter.readAppPreferences
      ? adapter.readAppPreferences()
      : adapter.readVisualPreferences?.().then((visual) => ({ ...DEFAULT_PREFERENCES, ...visual }));
    void read?.then((value) => {
      if (!disposed) {
        setPreferences(value);
        setSavedPreferences(value);
      }
    }).catch(() => {
      if (!disposed) setSettingsMessage("无法读取本机设置，已使用安全默认值。");
    });
    return () => { disposed = true; };
  }, [adapter]);

  useEffect(() => {
    if (!adapter.readProjectMergeRules) return;
    let disposed = false;
    void adapter.readProjectMergeRules().then((rules) => {
      if (!disposed) {
        setProjectMergeRules(rules);
        setProjectMergeMessage(null);
      }
    }).catch((error) => {
      if (!disposed) setProjectMergeMessage(error instanceof Error ? error.message : "无法读取项目合并设置，当前未应用合并。");
    });
    return () => { disposed = true; };
  }, [adapter]);

  useEffect(() => {
    if (view !== "settings" || !adapter.readAppPreferences) return;
    let disposed = false;
    void adapter.readAppPreferences().then((value) => {
      if (!disposed) {
        setPreferences(value);
        setSavedPreferences(value);
      }
    }).catch(() => {
      if (!disposed) setSettingsMessage("无法同步系统托盘与开机启动状态。");
    });
    return () => { disposed = true; };
  }, [adapter, view]);

  useEffect(() => {
    if (view !== "settings" || !adapter.readIndexMaintenanceReport) return;
    let disposed = false;
    void adapter.readIndexMaintenanceReport().then((report) => {
      if (!disposed) setIndexMaintenance(report);
    }).catch(() => {
      if (!disposed) setSettingsMessage("索引自检暂不可用；用量读取仍可继续使用。");
    });
    return () => { disposed = true; };
  }, [adapter, view]);

  const updatePlanRenewal = useCallback(async (value: string | null) => {
    if (!adapter.setPlanRenewalAt) throw new Error("当前环境不支持保存续费时间。");
    const setting = await adapter.setPlanRenewalAt(value);
    setSnapshot((current) => ({ ...current, ...setting }));
  }, [adapter]);

  const openDayDetail = useCallback(async (date: string) => {
    setSelectedDay(date);
    setDayDetail(null);
    setDayDetailError(null);
    if (!adapter.readUsageDayDetail) {
      setDayDetailError("当前数据桥不支持读取本机日明细。");
      return;
    }
    setIsDayDetailLoading(true);
    try {
      setDayDetail(await adapter.readUsageDayDetail(date));
    } catch {
      setDayDetailError("无法读取该日的本机用量明细，请稍后重试。");
    } finally {
      setIsDayDetailLoading(false);
    }
  }, [adapter]);
  const closeDayDetail = useCallback(() => setSelectedDay(null), []);

  const saveProjectMergeRules = useCallback(async (rules: ProjectMergeRule[]) => {
    if (!adapter.setProjectMergeRules) throw new Error("当前环境不支持保存项目合并设置。");
    const saved = await adapter.setProjectMergeRules(rules);
    setProjectMergeRules(saved);
    setProjectMergeMessage(null);
    setSuccessToast("项目合并设置已保存；来源日志和 Token 索引未修改。");
    return saved;
  }, [adapter]);

  const saveTheme = useCallback(async (themePreference: ThemePreference) => {
    setSettingsMessage(null);
    setPreferences((current) => ({ ...current, themePreference }));
    if (!adapter.setThemePreference) return;
    try {
      const visual = await adapter.setThemePreference(themePreference);
      setPreferences((current) => ({ ...current, ...visual }));
      setSavedPreferences((current) => ({ ...current, ...visual }));
    } catch {
      setSettingsMessage("无法保存主题设置。");
    }
  }, [adapter]);

  const selectWallpaper = useCallback(async () => {
    if (!adapter.selectBackgroundImage) return setSettingsMessage("当前环境不支持选择背景图片。");
    setIsSavingSettings(true);
    setSettingsMessage(null);
    try {
      const visual = await adapter.selectBackgroundImage();
      setPreferences((current) => ({ ...current, ...visual }));
      setSavedPreferences((current) => ({ ...current, ...visual }));
    } catch (error) {
      setSettingsMessage(error instanceof Error ? error.message : "无法保存背景图片。");
    } finally {
      setIsSavingSettings(false);
    }
  }, [adapter]);

  const clearWallpaper = useCallback(async () => {
    if (!adapter.clearBackgroundImage) return;
    setIsSavingSettings(true);
    try {
      const visual = await adapter.clearBackgroundImage();
      setPreferences((current) => ({ ...current, ...visual }));
      setSavedPreferences((current) => ({ ...current, ...visual }));
      setSettingsMessage("已恢复默认背景。");
    } catch {
      setSettingsMessage("无法恢复默认背景。");
    } finally {
      setIsSavingSettings(false);
    }
  }, [adapter]);

  const savePreferences = useCallback(async (next = preferences): Promise<boolean> => {
    if (!adapter.setAppPreferences) {
      setSettingsMessage("当前环境不支持保存完整应用设置。");
      return false;
    }
    setIsSavingSettings(true);
    setSettingsMessage(null);
    try {
      const saved = await adapter.setAppPreferences(next);
      setPreferences(saved);
      setSavedPreferences(saved);
      setSettingsMessage("设置已保存，并将在后台刷新与关闭行为中立即生效。");
      return true;
    } catch (error) {
      setSettingsMessage(error instanceof Error ? error.message : "无法保存应用设置。");
      return false;
    } finally {
      setIsSavingSettings(false);
    }
  }, [adapter, preferences]);

  const hasUnsavedRuntimePreferences = useMemo(
    () => !runtimePreferencesEqual(preferences, savedPreferences),
    [preferences, savedPreferences],
  );

  const requestViewChange = useCallback((nextView: AppView) => {
    if (nextView === view) return;
    if (view === "settings" && hasUnsavedRuntimePreferences) {
      setPendingView(nextView);
      return;
    }
    setView(nextView);
  }, [hasUnsavedRuntimePreferences, view]);

  useEffect(() => {
    if (!hasUnsavedRuntimePreferences) return;
    const guard = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener("beforeunload", guard);
    return () => window.removeEventListener("beforeunload", guard);
  }, [hasUnsavedRuntimePreferences]);

  const sendTestNotification = useCallback(async () => {
    if (!adapter.sendTestNotification) {
      setSettingsMessage("当前环境不支持系统通知测试。");
      return;
    }
    setIsSavingSettings(true);
    setSettingsMessage(null);
    try {
      const saved = await adapter.sendTestNotification();
      setPreferences((current) => ({
        ...current,
        lastNotificationAt: saved.lastNotificationAt,
        lastNotificationReason: saved.lastNotificationReason,
      }));
      setSavedPreferences((current) => ({
        ...current,
        lastNotificationAt: saved.lastNotificationAt,
        lastNotificationReason: saved.lastNotificationReason,
      }));
      setSettingsMessage("测试通知已发送，并记录了本次触发原因。");
    } catch (error) {
      setSettingsMessage(error instanceof Error ? error.message : "无法发送测试通知。");
    } finally {
      setIsSavingSettings(false);
    }
  }, [adapter]);

  const checkIndexes = useCallback(async () => {
    if (!adapter.readIndexMaintenanceReport) {
      setSettingsMessage("当前环境不支持索引自检。");
      return;
    }
    setIsMaintainingIndexes(true);
    setSettingsMessage(null);
    try {
      const report = await adapter.readIndexMaintenanceReport();
      setIndexMaintenance(report);
      setSettingsMessage(report.message);
      if (report.overallStatus === "healthy") {
        setSuccessToast("索引自检通过，未发现问题。");
      }
    } catch (error) {
      setSettingsMessage(error instanceof Error ? error.message : "索引自检失败。");
    } finally {
      setIsMaintainingIndexes(false);
    }
  }, [adapter]);

  const rebuildIndexes = useCallback(async () => {
    if (!adapter.rebuildIndexes) {
      setSettingsMessage("当前环境不支持索引重建。");
      return;
    }
    setIsMaintainingIndexes(true);
    setSettingsMessage(null);
    try {
      const result = await adapter.rebuildIndexes();
      setSnapshot(result.snapshot);
      setProgress(result.snapshot.refreshProgress ?? null);
      setIndexMaintenance(result.report);
      setSettingsMessage(result.backupCreatedAt
        ? "派生索引已备份并重建完成；原始来源日志未修改。"
        : "当前没有旧索引需要备份，已完成全量重建。");
    } catch (error) {
      setSettingsMessage(error instanceof Error ? error.message : "无法备份并重建派生索引。");
    } finally {
      setIsMaintainingIndexes(false);
    }
  }, [adapter]);

  const stale = isSnapshotStale(snapshot.fetchedAt, now);
  const effectiveTheme = preferences.themePreference === "system" ? systemTheme : preferences.themePreference;
  const developmentWallpaperUrl = readDevelopmentWallpaperUrl();
  const wallpaperUrl = preferences.backgroundAssetPath && isTauri()
    ? convertFileSrc(preferences.backgroundAssetPath)
    : developmentWallpaperUrl;
  const appStyle = wallpaperUrl
    ? { "--wallpaper-image": `url("${wallpaperUrl.replaceAll('"', "%22")}")` } as CSSProperties
    : undefined;
  const deviceUsage = snapshot.deviceUsage ?? null;
  const loadCustomRange = useCallback(async (startDate: string, endDate: string) => {
    const dates = enumerateLocalDates(startDate, endDate);
    if (dates.length === 0) {
      setCustomRangeError("开始日期不能晚于结束日期。");
      return;
    }
    if (dates.length > 31) {
      setCustomRangeError("自定义范围最多支持连续 31 天。");
      return;
    }
    if (!deviceUsage || !adapter.readUsageDayDetail) {
      setCustomRangeError("当前环境不支持读取自定义日期明细。");
      return;
    }
    setIsLoadingCustomRange(true);
    setCustomRangeError(null);
    try {
      const details = await Promise.all(dates.map((date) => adapter.readUsageDayDetail!(date)));
      setCustomSources(attachCustomRanges(deviceUsage.sources, details));
      setAppliedCustomStartDate(startDate);
      setAppliedCustomEndDate(endDate);
      setRange("custom");
    } catch {
      setCustomRangeError("无法读取所选日期范围，请稍后重试。");
    } finally {
      setIsLoadingCustomRange(false);
    }
  }, [adapter, deviceUsage]);
  const applyCustomRange = useCallback(
    () => loadCustomRange(customStartDate, customEndDate),
    [customEndDate, customStartDate, loadCustomRange],
  );
  const effectiveDeviceUsage = useMemo(() => deviceUsage && range === "custom" && customSources
    ? { ...deviceUsage, sources: customSources }
    : deviceUsage, [customSources, deviceUsage, range]);
  const customRangeSnapshotAt = useRef<string | null>(null);
  useEffect(() => {
    const generatedAt = deviceUsage?.generatedAt ?? null;
    if (customRangeSnapshotAt.current === null) {
      customRangeSnapshotAt.current = generatedAt;
      return;
    }
    if (generatedAt === customRangeSnapshotAt.current) return;
    customRangeSnapshotAt.current = generatedAt;
    if (range === "custom" && customSources) {
      void loadCustomRange(appliedCustomStartDate, appliedCustomEndDate);
    }
  }, [appliedCustomEndDate, appliedCustomStartDate, customSources, deviceUsage?.generatedAt, loadCustomRange, range]);
  const activityRange = useMemo(() => effectiveDeviceUsage
    ? aggregateSelectedSources(effectiveDeviceUsage.sources, activitySources, range)
    : null, [effectiveDeviceUsage, activitySources, range]);
  const costRange = useMemo(() => effectiveDeviceUsage
    ? aggregateSelectedSources(effectiveDeviceUsage.sources, costSources, range)
    : null, [effectiveDeviceUsage, costSources, range]);
  const insightRange = useMemo(() => effectiveDeviceUsage
    ? aggregateSelectedSources(effectiveDeviceUsage.sources, activitySources, "last30Days")
    : null, [effectiveDeviceUsage, activitySources]);

  return (
    <div className="app-shell" data-theme={effectiveTheme} data-wallpaper={wallpaperUrl ? "active" : undefined} style={appStyle}>
      <div className="background-base" aria-hidden="true" />
      {wallpaperUrl && <><div className="wallpaper-layer" aria-hidden="true" /><div className="wallpaper-scrim" aria-hidden="true" /></>}
      <header className="topbar">
        <div className="topbar__row">
          <button className="brand-lockup" type="button" onClick={() => requestViewChange("overview")} aria-label="返回总览">
            <span className="brand-mark"><Icon name="activity" size={22} /></span>
            <span><small>AI TOKEN PULSE</small><strong>AI Token 用量监控</strong></span>
          </button>
          <nav className="view-tabs" aria-label="应用视图">
            {VIEW_ITEMS.map((item) => <button key={item.id} type="button" className={view === item.id ? "is-active" : ""} aria-label={item.label} title={item.label} aria-current={view === item.id ? "page" : undefined} onClick={() => requestViewChange(item.id)}><Icon name={item.icon} /><span>{item.label}</span></button>)}
          </nav>
          <div className="topbar-actions">
            <div className="status-menu">
              <button className={`status-chip status-chip--${stale && snapshot.status === "ready" ? "warning" : snapshot.status}`} type="button" aria-expanded={statusOpen} onClick={() => setStatusOpen((value) => !value)}><span className="status-chip__dot" />{stale && snapshot.status === "ready" ? "数据已过期" : snapshot.status === "ready" ? "数据已就绪" : snapshot.status === "loading" ? "正在同步" : snapshot.status === "offline" ? "离线" : snapshot.status === "unauthenticated" ? "未登录" : snapshot.status === "unsupported" ? "版本不支持" : "读取失败"}</button>
              {statusOpen && <StatusPopover snapshot={snapshot} progress={progress} now={now} onClose={() => setStatusOpen(false)} />}
            </div>
            <span className="refresh-countdown" role="status" aria-live="polite">
              {formatRefreshCountdown(snapshot.refreshSchedule?.nextRefreshAtMs, isRefreshing, now)}
            </span>
            <button className="refresh-button" type="button" onClick={() => void refresh()} disabled={isRefreshing} aria-label={isRefreshing ? "正在刷新数据" : "刷新数据"}><span className={isRefreshing ? "spin" : ""}><Icon name="refresh" /></span><span>{isRefreshing ? "同步中" : "刷新"}</span></button>
          </div>
        </div>
      </header>
      <main>
        <StatusNotice snapshot={snapshot} stale={stale} progress={progress} onRefresh={() => void refresh()} onOpenSettings={() => requestViewChange("settings")} />
        {view === "overview" && <OverviewView snapshot={snapshot} now={now} onOpenDay={openDayDetail} onOpenActivity={() => requestViewChange("activity")} />}
        {view === "activity" && <ActivityView usage={effectiveDeviceUsage} sourceHealth={snapshot.sourceHealth ?? []} range={range} onRangeChange={setRange} selectedSources={activitySources} selectionMode={activitySourceMode} onSelectionModeChange={(mode) => { setActivitySourceMode(mode); if (mode === "single") setActivitySources((current) => current.slice(-1)); }} onSourcesChange={(source) => setActivitySources((current) => toggleSourceSelection(current, source, activitySourceMode))} onClearSources={() => setActivitySources([])} summary={activityRange} insightSummary={insightRange} now={now} onOpenDay={openDayDetail} customStartDate={customStartDate} customEndDate={customEndDate} appliedCustomStartDate={appliedCustomStartDate} appliedCustomEndDate={appliedCustomEndDate} onCustomStartDateChange={setCustomStartDate} onCustomEndDateChange={setCustomEndDate} onApplyCustomRange={() => void applyCustomRange()} isLoadingCustomRange={isLoadingCustomRange} customRangeError={customRangeError} projectMergeRules={projectMergeRules} projectMergeMessage={projectMergeMessage} onSaveProjectMergeRules={saveProjectMergeRules} />}
        {view === "cost" && <CostView usage={effectiveDeviceUsage} range={range} onRangeChange={setRange} selectedSources={costSources} selectionMode={costSourceMode} onSelectionModeChange={(mode) => { setCostSourceMode(mode); if (mode === "single") setCostSources((current) => current.slice(-1)); }} onSourcesChange={(source) => setCostSources((current) => toggleSourceSelection(current, source, costSourceMode))} onClearSources={() => setCostSources([])} summary={costRange} customStartDate={customStartDate} customEndDate={customEndDate} appliedCustomStartDate={appliedCustomStartDate} appliedCustomEndDate={appliedCustomEndDate} onCustomStartDateChange={setCustomStartDate} onCustomEndDateChange={setCustomEndDate} onApplyCustomRange={() => void applyCustomRange()} isLoadingCustomRange={isLoadingCustomRange} customRangeError={customRangeError} />}
        {view === "settings" && <SettingsView snapshot={snapshot} preferences={preferences} indexMaintenance={indexMaintenance} onPreferencesChange={setPreferences} onSave={() => void savePreferences()} onTestNotification={() => void sendTestNotification()} onCheckIndexes={() => void checkIndexes()} onRebuildIndexes={() => void rebuildIndexes()} onThemeChange={(value) => void saveTheme(value)} onSelectWallpaper={() => void selectWallpaper()} onClearWallpaper={() => void clearWallpaper()} onUpdateRenewal={updatePlanRenewal} isSaving={isSavingSettings} isMaintainingIndexes={isMaintainingIndexes} hasUnsavedChanges={hasUnsavedRuntimePreferences} message={settingsMessage} />}
      </main>
      {pendingView && (
        <div className="unsaved-dialog-backdrop" role="presentation">
          <section className="unsaved-dialog" role="alertdialog" aria-modal="true" aria-labelledby="unsaved-dialog-title">
            <p className="eyebrow">未保存的运行设置</p>
            <h2 id="unsaved-dialog-title">离开设置前要保存吗？</h2>
            <p>刷新、关闭行为、启动或通知设置已修改。你可以保存后离开，也可以放弃本次修改。</p>
            <div className="inline-actions">
              <button type="button" onClick={() => setPendingView(null)}>继续编辑</button>
              <button type="button" onClick={() => {
                const nextView = pendingView;
                setPreferences(savedPreferences);
                setPendingView(null);
                setView(nextView);
              }}>放弃更改</button>
              <button className="primary-button" type="button" disabled={isSavingSettings} onClick={() => {
                const nextView = pendingView;
                void savePreferences().then((saved) => {
                  if (!saved) return;
                  setPendingView(null);
                  setView(nextView);
                });
              }}>{isSavingSettings ? "保存中…" : "保存并离开"}</button>
            </div>
          </section>
        </div>
      )}
      {selectedDay && <UsageDayDrawer date={selectedDay} accountTokens={snapshot.accountUsage?.dailyUsageBuckets?.find((bucket) => bucket.startDate === selectedDay)?.tokens ?? null} detail={dayDetail} loading={isDayDetailLoading} error={dayDetailError} cacheWarningPercent={preferences.cacheWarningPercent} projectMergeRules={projectMergeRules} sources={snapshot.deviceUsage?.sources ?? []} onClose={closeDayDetail} />}
      {successToast && <div className="success-toast" role="status"><Icon name="check" /><span>{successToast}</span></div>}
    </div>
  );
}
