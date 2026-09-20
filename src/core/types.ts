export interface QuotaWindow {
  usedPercent: number;
  windowDurationMins: number;
  resetsAt: number;
}

export interface CreditBalance {
  hasCredits: boolean;
  unlimited: boolean;
  balance: string | null;
}

export interface RateLimitBucket {
  limitId: string;
  limitName: string | null;
  normalModelSlug?: string | null;
  primary: QuotaWindow | null;
  secondary: QuotaWindow | null;
  credits?: CreditBalance | null;
  planType?: string | null;
  rateLimitReachedType?: string | null;
}

export interface RateLimitResetCredits {
  availableCount: number;
  credits: Array<{
    id: string;
    resetType: string;
    status: string;
    grantedAt: number;
    expiresAt: number | null;
    title: string | null;
    description: string | null;
  }> | null;
}

export interface RateLimitsReadResult {
  rateLimits?: RateLimitBucket | null;
  rateLimitsByLimitId?: Record<string, RateLimitBucket> | null;
  rateLimitResetCredits?: RateLimitResetCredits | null;
}

export interface AccountUsageSummary {
  lifetimeTokens: number | null;
  peakDailyTokens: number | null;
  longestRunningTurnSec: number | null;
  currentStreakDays: number | null;
  longestStreakDays: number | null;
}

export interface DailyUsageBucket {
  startDate: string;
  tokens: number;
}

export interface AccountUsageReadResult {
  summary: AccountUsageSummary | null;
  dailyUsageBuckets: DailyUsageBucket[] | null;
  threadUsage?: unknown | null;
}

export interface TokenUsage {
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
  totalTokens: number;
}

export interface UsageDimension {
  key: string;
  label: string;
  usage: TokenUsage;
}

export interface LocalUsageSummary {
  generatedAt: string;
  sourceFiles: number;
  indexedEvents: number;
  skippedEvents: number;
  filteredParentEvents: number;
  total: TokenUsage;
  today: TokenUsage;
  byModel: UsageDimension[];
  byProject: UsageDimension[];
  warnings: string[];
}

/** A normalized local activity measure across AI applications. */
export interface DeviceTokenUsage extends TokenUsage {
  /** Prompt-cache writes are distinct from cache reads and are not output tokens. */
  cacheWriteTokens: number;
  cacheWrite5mTokens: number;
  cacheWrite1hTokens: number;
  cacheWriteUnknownTokens: number;
}

export interface TokenCost {
  usd: number;
  cny: number;
  unpricedTokens: number;
}

export interface DeviceUsageDimension {
  key: string;
  label: string;
  usage: DeviceTokenUsage;
  cost: TokenCost;
}

export interface ProjectMergeMember {
  sourceId: UsageSourceSummary["id"];
  sourceLabel: string;
  projectKey: string;
  projectLabel: string;
}

export interface ProjectMergeRule {
  id: string;
  displayName: string;
  members: ProjectMergeMember[];
}

export type UsageRange = "today" | "last7Days" | "last30Days" | "total" | "custom";

export interface DeviceDailyUsageBucket {
  date: string;
  usage: DeviceTokenUsage;
  cost: TokenCost;
}

export interface DeviceHourlyUsageBucket {
  hourStartMs: number;
  usage: DeviceTokenUsage;
  cost: TokenCost;
}

export interface DeviceUsageRangeSummary {
  usage: DeviceTokenUsage;
  cost: TokenCost;
  byModel: DeviceUsageDimension[];
  byProject: DeviceUsageDimension[];
  dailyUsage: DeviceDailyUsageBucket[];
  /** Local-clock hour buckets limited to the rolling past 24 hours. */
  hourlyUsage?: DeviceHourlyUsageBucket[];
}

export interface DeviceUsageRanges {
  today: DeviceUsageRangeSummary;
  /** Rolling current local hour plus the preceding 23 local hours. */
  last24Hours?: DeviceUsageRangeSummary;
  last7Days: DeviceUsageRangeSummary;
  last30Days: DeviceUsageRangeSummary;
  total: DeviceUsageRangeSummary;
  /** Built on demand from date-scoped, privacy-safe day details. */
  custom?: DeviceUsageRangeSummary;
}

export type UsageSourceStatus = "ready" | "notDetected" | "unavailable" | "error";

export interface UsageSourceSummary {
  id: "codex" | "claude-code" | "opencode" | "workbuddy" | "workbuddy-ai" | "cursor";
  label: string;
  status: UsageSourceStatus;
  message: string | null;
  sourceFiles: number;
  indexedEvents: number;
  newEvents: number | null;
  skippedRecords: number | null;
  lastIndexedAt: string | null;
  total: DeviceTokenUsage;
  today: DeviceTokenUsage;
  cost: TokenCost;
  todayCost: TokenCost;
  byModel: DeviceUsageDimension[];
  todayByModel: DeviceUsageDimension[];
  byProject: DeviceUsageDimension[];
  /** Provider-specific Credits are a separate unit and never Token totals. */
  credits: { balance: number } | null;
  ranges?: DeviceUsageRanges;
}

export interface DeviceUsageSummary {
  generatedAt: string;
  priceSnapshotDate: string;
  priceCatalog: PriceCatalogEntry[];
  total: DeviceTokenUsage;
  today: DeviceTokenUsage;
  cost: TokenCost;
  todayCost: TokenCost;
  sources: UsageSourceSummary[];
  byModel: DeviceUsageDimension[];
  todayByModel: DeviceUsageDimension[];
  byProject: DeviceUsageDimension[];
  warnings: string[];
  ranges?: DeviceUsageRanges;
}

export interface PriceCatalogTier {
  label: string;
  condition: string;
  inputPerMillion: number;
  cachedInputPerMillion: number | null;
  cacheWritePerMillion: number | null;
  outputPerMillion: number;
}

export interface PriceCatalogEntry {
  displayName: string;
  modelId: string;
  aliases: string[];
  currency: "USD" | "CNY";
  sourceLabel: string;
  tiers: PriceCatalogTier[];
}

export interface SourceHealthSummary {
  id: UsageSourceSummary["id"];
  label: string;
  status: UsageSourceStatus;
  sourceFiles: number;
  indexedEvents: number;
  newEvents: number | null;
  skippedRecords: number | null;
  lastIndexedAt: string | null;
  message: string | null;
}

export interface RefreshProgress {
  phase: "idle" | "codex" | "sources" | "account" | "complete";
  label: string;
  processedSources: number;
  totalSources: number;
  currentSource: string | null;
  processedFiles: number;
  totalFiles: number;
}

export interface IndexDatabaseDiagnostic {
  label: string;
  sizeBytes: number;
}

export interface IndexDiagnostics {
  totalDurationMs: number;
  codexDurationMs: number;
  sourcesDurationMs: number;
  accountDurationMs: number;
  databases: IndexDatabaseDiagnostic[];
}

export interface IndexDatabaseHealth {
  label: string;
  status: "healthy" | "missing" | "error";
  integrity: "ok" | "failed" | "unavailable";
  sizeBytes: number;
  schemaVersion: number | null;
  expectedSchemaVersion: number;
}

export interface IndexMaintenanceReport {
  checkedAt: string;
  overallStatus: "healthy" | "warning" | "error";
  databases: IndexDatabaseHealth[];
  lastBackupAt: string | null;
  message: string;
}

export interface IndexRebuildResult {
  backupCreatedAt: string | null;
  report: IndexMaintenanceReport;
  snapshot: DashboardSnapshot;
}

export interface TaskUsageDetail {
  occurredAtMs: number | null;
  source: string;
  modelLabel: string;
  projectLabel: string;
  taskLabel: string;
  usage: DeviceTokenUsage;
  cacheMetricsAvailable: boolean;
  cacheHitRate: number | null;
  lowCacheHit: boolean;
  cost: TokenCost;
}

export interface UsageDayDetail {
  date: string;
  tasks: TaskUsageDetail[];
  lowCacheTasks: number;
}

export interface NormalizedQuotaWindow extends QuotaWindow {
  key: string;
  limitId: string;
  limitName: string | null;
  normalModelSlug: string | null;
  lane: "primary" | "secondary";
  label: string;
  remainingPercent: number;
  credits: CreditBalance | null;
  planType: string | null;
  reachedType: string | null;
}

export interface AccountReadDiagnostics {
  readAt: string;
  durationMs: number;
  methods: string[];
}

export interface DashboardSnapshot {
  status: "ready" | "loading" | "offline" | "unauthenticated" | "unsupported" | "error";
  fetchedAt: string | null;
  codexVersion: string | null;
  quotaWindows: NormalizedQuotaWindow[];
  /** Sanitized account-read audit; never contains identifiers, parameters or credentials. */
  accountDiagnostics?: AccountReadDiagnostics | null;
  /**
   * The account plan renewal time, when supplied by a future trusted source.
   * Codex App Server does not currently expose this value, so `null` is the
   * expected value for live snapshots rather than an inferred quota reset.
   */
  planRenewalAt: string | number | null;
  /** `manual` values are user-entered local settings, never official data. */
  planRenewalSource: "official" | "manual" | null;
  resetCredits: RateLimitResetCredits | null;
  accountUsage: AccountUsageReadResult | null;
  /** Legacy phase-one data; the desktop app now uses deviceUsage. */
  localUsage?: LocalUsageSummary | null;
  deviceUsage?: DeviceUsageSummary | null;
  lastSuccessfulAt?: string | null;
  refreshProgress?: RefreshProgress | null;
  refreshSchedule?: RefreshSchedule | null;
  sourceHealth?: SourceHealthSummary[];
  indexDiagnostics?: IndexDiagnostics | null;
  message: string | null;
}

export interface RefreshSchedule {
  nextRefreshAtMs: number | null;
  intervalSeconds: 60 | 300 | 900;
}

export interface PlanRenewalSetting {
  planRenewalAt: string | null;
  planRenewalSource: "manual" | null;
}

export type ThemePreference = "system" | "light" | "dark";

export interface VisualPreferences {
  themePreference: ThemePreference;
  /** Application-owned file path only; the original selected path is never stored. */
  backgroundAssetPath: string | null;
}

export type CloseBehavior = "hideToTray" | "exit";

export interface AppPreferences extends VisualPreferences {
  refreshIntervalMinutes: 1 | 5 | 15;
  closeBehavior: CloseBehavior;
  autostartEnabled: boolean;
  quotaWarningPercent: number;
  cacheWarningPercent: number;
  notificationsEnabled: boolean;
  lastNotificationAt: string | null;
  lastNotificationReason: string | null;
  quietHoursEnabled: boolean;
  quietHoursStart: string;
  quietHoursEnd: string;
}
