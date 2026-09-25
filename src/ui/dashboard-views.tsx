import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";

import type {
  AppPreferences,
  DashboardSnapshot,
  DeviceTokenUsage,
  DeviceUsageSummary,
  DeviceUsageRangeSummary,
  IndexMaintenanceReport,
  NormalizedQuotaWindow,
  ProjectMergeMember,
  ProjectMergeRule,
  PricingSettings,
  RefreshProgress,
  SourceHealthSummary,
  ThemePreference,
  UsageDayDetail,
  UsageRange,
  UsageSourceSummary,
} from "../core/types";
import {
  aggregateCostRows,
  buildDailySourceTrend,
  buildFifteenDaySourceTrend,
  buildProjectCatalog,
  buildProjectDimensionRows,
  buildRollingHourlySourceTrend,
  buildSourceDimensionRows,
  buildUsageInsights,
  costDetailRows,
  SOURCE_TREND_COLORS,
  projectMemberIdentity,
  type CostDetailRow,
  type SourceStackedDimension,
  type SourceTrendSegment,
  type SourceSelection,
  type SourceSelectionMode,
} from "./device-usage-view-model";
import {
  formatCountdown,
  formatChineseCompactTokens,
  formatDateTime,
  formatExactTokens,
  formatPercent,
  formatRelativeTime,
  formatTokens,
  formatWindowDuration,
  getQuotaPresentation,
  toEpochMilliseconds,
} from "./format";
import { selectOverviewQuotas } from "./quota-view-model";

export type AppView = "overview" | "activity" | "cost" | "pricing" | "settings";
export type IconName =
  | "activity"
  | "refresh"
  | "overview"
  | "wallet"
  | "settings"
  | "clock"
  | "info"
  | "shield"
  | "image"
  | "sun"
  | "monitor"
  | "check"
  | "download"
  | "database"
  | "x";

const RANGE_ITEMS: Array<{ id: UsageRange; label: string; short: string }> = [
  { id: "today", label: "今日", short: "今日" },
  { id: "last7Days", label: "最近 7 天", short: "7 天" },
  { id: "last30Days", label: "最近 30 天", short: "30 天" },
  { id: "total", label: "全部累计", short: "累计" },
  { id: "custom", label: "自定义日期", short: "自定义" },
];
function formatRangeLabel(range: UsageRange, customStartDate: string, customEndDate: string): string {
  return range === "custom"
    ? `${customStartDate} 至 ${customEndDate}`
    : RANGE_ITEMS.find((item) => item.id === range)?.label ?? "所选范围";
}
const TOKEN_COLORS = ["#7c6cff", "#3abfe9", "#35c994", "#f2a851", "#dd6ea8"];

export function Icon({ name, size = 18 }: { name: IconName; size?: number }) {
  const common = {
    width: size,
    height: size,
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.8,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
    "aria-hidden": true,
  };
  const paths: Record<IconName, ReactNode> = {
    activity: <path d="M3 12h4l2-6 4 12 2-6h6" />,
    refresh: (
      <>
        <path d="M20 6v5h-5" />
        <path d="M4 18v-5h5" />
        <path d="M6.1 9a7 7 0 0 1 11.7-2.6L20 8.7M4 15.3l2.2 2.3A7 7 0 0 0 18 15" />
      </>
    ),
    overview: (
      <>
        <rect x="3" y="3" width="7" height="7" rx="2" />
        <rect x="14" y="3" width="7" height="7" rx="2" />
        <rect x="3" y="14" width="7" height="7" rx="2" />
        <rect x="14" y="14" width="7" height="7" rx="2" />
      </>
    ),
    wallet: (
      <>
        <path d="M4 7h14a2 2 0 0 1 2 2v9H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h12" />
        <path d="M16 12h4v3h-4a1.5 1.5 0 0 1 0-3Z" />
      </>
    ),
    settings: (
      <>
        <circle cx="12" cy="12" r="3" />
        <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1-2.8 2.8-.1-.1a1.7 1.7 0 0 0-1.9-.3A1.7 1.7 0 0 0 14 21v.2h-4V21a1.7 1.7 0 0 0-1-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1L4.2 17l.1-.1a1.7 1.7 0 0 0 .3-1.9A1.7 1.7 0 0 0 3 14H2.8v-4H3a1.7 1.7 0 0 0 1.6-1 1.7 1.7 0 0 0-.3-1.9L4.2 7 7 4.2l.1.1A1.7 1.7 0 0 0 9 4.6 1.7 1.7 0 0 0 10 3v-.2h4V3a1.7 1.7 0 0 0 1 1.6 1.7 1.7 0 0 0 1.9-.3l.1-.1L19.8 7l-.1.1a1.7 1.7 0 0 0-.3 1.9 1.7 1.7 0 0 0 1.6 1h.2v4H21a1.7 1.7 0 0 0-1.6 1Z" />
      </>
    ),
    clock: (
      <>
        <circle cx="12" cy="12" r="9" />
        <path d="M12 7v5l3 2" />
      </>
    ),
    info: (
      <>
        <circle cx="12" cy="12" r="9" />
        <path d="M12 11v5M12 8h.01" />
      </>
    ),
    shield: (
      <>
        <path d="M12 3 5 6v5c0 4.6 2.8 8.2 7 10 4.2-1.8 7-5.4 7-10V6l-7-3Z" />
        <path d="m9.2 12 1.8 1.8 3.9-4" />
      </>
    ),
    image: (
      <>
        <rect x="3" y="4" width="18" height="16" rx="2" />
        <circle cx="8.5" cy="9" r="1.4" />
        <path d="m4 18 5.2-5.2a2 2 0 0 1 2.8 0l2 2 1.3-1.3a2 2 0 0 1 2.8 0L21 16.4" />
      </>
    ),
    sun: (
      <>
        <circle cx="12" cy="12" r="4" />
        <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2" />
      </>
    ),
    monitor: (
      <>
        <rect x="3" y="4" width="18" height="13" rx="2" />
        <path d="M8 21h8M12 17v4" />
      </>
    ),
    check: <path d="m5 12 4 4L19 6" />,
    download: (
      <>
        <path d="M12 3v12m0 0 4-4m-4 4-4-4" />
        <path d="M5 20h14" />
      </>
    ),
    database: (
      <>
        <ellipse cx="12" cy="5" rx="8" ry="3" />
        <path d="M4 5v6c0 1.7 3.6 3 8 3s8-1.3 8-3V5M4 11v6c0 1.7 3.6 3 8 3s8-1.3 8-3v-6" />
      </>
    ),
    x: <path d="m6 6 12 12M18 6 6 18" />,
  };
  return <svg {...common}>{paths[name]}</svg>;
}

export function StatusPopover({
  snapshot,
  progress,
  now,
  onClose,
}: {
  snapshot: DashboardSnapshot;
  progress: RefreshProgress | null;
  now: number;
  onClose: () => void;
}) {
  return (
    <div className="status-popover" role="dialog" aria-label="数据状态详情">
      <header>
        <strong>数据状态</strong>
        <button type="button" onClick={onClose} aria-label="关闭状态详情">
          <Icon name="x" />
        </button>
      </header>
      <dl>
        <div>
          <dt>当前状态</dt>
          <dd>{statusLabel(snapshot.status)}</dd>
        </div>
        <div>
          <dt>账号最近读取</dt>
          <dd>{formatRelativeTime(snapshot.accountDiagnostics?.readAt ?? snapshot.fetchedAt, now)}</dd>
        </div>
        <div>
          <dt>账号读取 RPC</dt>
          <dd>{snapshot.accountDiagnostics?.methods.join(" · ") || "尚无记录"}</dd>
        </div>
        <div>
          <dt>本机最近索引</dt>
          <dd>{formatRelativeTime(snapshot.deviceUsage?.generatedAt ?? null, now)}</dd>
        </div>
        <div>
          <dt>刷新阶段</dt>
          <dd>{progress?.label ?? "空闲"}</dd>
        </div>
        <div>
          <dt>Codex 版本</dt>
          <dd>{snapshot.codexVersion ?? "未检测到"}</dd>
        </div>
      </dl>
      {snapshot.message && <p>{snapshot.message}</p>}
    </div>
  );
}

export function StatusNotice({
  snapshot,
  stale,
  progress,
  onRefresh,
  onOpenSettings,
}: {
  snapshot: DashboardSnapshot;
  stale: boolean;
  progress: RefreshProgress | null;
  onRefresh: () => void;
  onOpenSettings: () => void;
}) {
  if (snapshot.status === "ready" && !stale && !snapshot.message) return null;
  const tone =
    snapshot.status === "error"
      ? "danger"
      : snapshot.status === "loading"
        ? "info"
        : "warning";
  const needsRecovery =
    snapshot.status !== "ready" && snapshot.status !== "loading";
  return (
    <section className={`notice notice--${tone}`} aria-live="polite">
      <Icon name={snapshot.status === "loading" ? "clock" : "info"} />
      <div>
        <strong>
          {stale && snapshot.status === "ready"
            ? "数据可能已过期"
            : statusLabel(snapshot.status)}
        </strong>
        <p>
          {snapshot.status === "loading"
            ? (progress?.label ?? snapshot.message)
            : (snapshot.message ?? "请刷新后重试。")}
        </p>
      </div>
      {needsRecovery ? (
        <div className="notice__actions">
          <button type="button" onClick={onRefresh}>
            重新刷新
          </button>
          <button type="button" onClick={onOpenSettings}>
            检查设置
          </button>
        </div>
      ) : snapshot.status === "loading" ? (
        <ProgressBar progress={progress} />
      ) : null}
    </section>
  );
}

function ProgressBar({ progress }: { progress: RefreshProgress | null }) {
  const total = progress?.totalSources || 6;
  const fileFraction =
    progress && progress.totalFiles > 0
      ? progress.processedFiles / progress.totalFiles
      : 0;
  const current = Math.min(
    total,
    (progress?.processedSources || 0) + fileFraction,
  );
  return (
    <div
      className="refresh-progress"
      role="progressbar"
      aria-label={progress?.label ?? "刷新进度"}
      aria-valuemin={0}
      aria-valuemax={total}
      aria-valuenow={current}
    >
      <span
        style={{
          width: `${Math.max(8, Math.min(100, (current / total) * 100))}%`,
        }}
      />
    </div>
  );
}

function PageHeading({
  eyebrow,
  title,
  description,
  actions,
}: {
  eyebrow: string;
  title: string;
  description: string;
  actions?: ReactNode;
}) {
  return (
    <header className="page-heading">
      <div>
        <p className="eyebrow">{eyebrow}</p>
        <h1>{title}</h1>
        <p>{description}</p>
      </div>
      {actions}
    </header>
  );
}

export function OverviewView({
  snapshot,
  now,
  onOpenDay,
  onOpenActivity,
}: {
  snapshot: DashboardSnapshot;
  now: number;
  onOpenDay: (date: string) => void;
  onOpenActivity: () => void;
}) {
  const localTrend = buildFifteenDaySourceTrend(snapshot.deviceUsage?.sources ?? [], now);
  const overviewQuotas = selectOverviewQuotas(snapshot.quotaWindows);
  return (
    <section className="view-panel">
      <PageHeading
        eyebrow="AT A GLANCE"
        title="总览"
        description="额度、本机活动和费用保持独立口径，一眼查看最重要的状态。"
        actions={
          <button
            className="secondary-button"
            type="button"
            onClick={onOpenActivity}
          >
            查看本机明细
          </button>
        }
      />
      <div className="overview-grid">
        {overviewQuotas.map(({ quota, title }) => (
          <QuotaSummaryCard key={quota.key} quota={quota} title={title} now={now} />
        ))}
        <MetricCard
          className="overview-grid__local"
          eyebrow="今日本机 Token"
          value={formatTokens(snapshot.deviceUsage?.today.totalTokens ?? null)}
          detail="仅此设备可读取的本地活动"
          icon="monitor"
          tone="cyan"
        />
        <MetricCard
          className="overview-grid__local"
          eyebrow="今日估算费用"
          value={formatCostCompact(snapshot.deviceUsage?.todayCost)}
          detail={
            snapshot.deviceUsage?.todayCost.unpricedTokens
              ? `${formatTokens(snapshot.deviceUsage.todayCost.unpricedTokens)} Token 尚未定价`
              : "按本机手动模型价格估算"
          }
          icon="wallet"
          tone="green"
        />
      </div>
      {(snapshot.accountUsage?.dailyUsageBuckets?.length ?? 0) > 0 && (
        <div className="overview-account-trend">
          <TrendCard
            title="Codex 账号近期每日 Token"
            subtitle="服务端账号日总量；可包含其他设备和云端任务，与下方本机归因趋势分开。"
            buckets={(snapshot.accountUsage?.dailyUsageBuckets ?? []).map((bucket) => ({
              date: bucket.startDate,
              tokens: bucket.tokens,
            }))}
            onOpenDay={onOpenDay}
            color={SOURCE_TREND_COLORS.codex}
          />
        </div>
      )}
      <div className="overview-lower-grid">
        <StackedSourceTrendCard
          buckets={localTrend.buckets}
          legend={localTrend.legend}
          onOpenDay={onOpenDay}
        />
        <AccountFacts snapshot={snapshot} now={now} />
      </div>
    </section>
  );
}

function StackedSourceTrendCard({
  buckets,
  legend,
  onOpenDay,
}: {
  buckets: ReturnType<typeof buildFifteenDaySourceTrend>["buckets"];
  legend: ReturnType<typeof buildFifteenDaySourceTrend>["legend"];
  onOpenDay: (date: string) => void;
}) {
  const max = Math.max(1, ...buckets.map((bucket) => bucket.tokens));
  const peak = buckets.reduce<(typeof buckets)[number] | null>(
    (current, bucket) => current === null || bucket.tokens > current.tokens ? bucket : current,
    null,
  );
  return (
    <article className="content-card trend-card stacked-trend-card">
      <header>
        <div>
          <p className="eyebrow">趋势</p>
          <h2>本机近 15 日 Token</h2>
          <p>按本机可归因的 AI 软件用量占比堆叠，缺失日期按 0 显示。</p>
        </div>
        {peak && peak.tokens > 0 && (
          <span className="peak-pill">峰值 {formatChineseCompactTokens(peak.tokens)} · {peak.date.slice(5)}</span>
        )}
      </header>
      {legend.length > 0 && (
        <ul className="stacked-trend__legend" aria-label="AI 软件颜色图例">
          {legend.map((item) => <li key={item.sourceId}><span style={{ background: item.color }} />{item.label}</li>)}
        </ul>
      )}
      <div className="stacked-trend" role="list" aria-label="本机近 15 日 Token">
        {buckets.map((bucket) => {
          const positive = bucket.segments.filter((segment) => segment.tokens > 0);
          const details = positive.map((segment) => `${segment.label} ${formatChineseCompactTokens(segment.tokens)}（${segment.share.toFixed(1)}%）`).join(" · ");
          const tooltip = `${bucket.date} · 总计 ${formatChineseCompactTokens(bucket.tokens)}${details ? ` · ${details}` : ""}`;
          return (
            <button
              key={bucket.date}
              type="button"
              role="listitem"
              className={`stacked-trend__bar${bucket.tokens === 0 ? " is-empty" : ""}`}
              data-value={tooltip}
              title={tooltip}
              aria-label={tooltip}
              onClick={() => onOpenDay(bucket.date)}
            >
              {bucket.tokens > 0 && (
                <span className="stacked-trend__stack" style={{ height: `${bucket.tokens / max * 100}%` }}>
                  {positive.map((segment) => (
                    <i
                      key={segment.sourceId}
                      style={{ background: segment.color, height: `${segment.share}%` }}
                      aria-hidden="true"
                    />
                  ))}
                </span>
              )}
              <small>{bucket.date.slice(5).replace("-", "/")}</small>
            </button>
          );
        })}
      </div>
    </article>
  );
}

function QuotaSummaryCard({
  quota,
  title,
  now,
}: {
  quota: NormalizedQuotaWindow;
  title: string;
  now: number;
}) {
  const value = getQuotaPresentation(quota);
  return (
    <article
      className={`metric-card overview-grid__quota quota-summary quota-summary--${value.tone}`}
    >
      <header>
        <span>
          <Icon name="clock" />
          {title}
        </span>
        <small>{quota.label || "额度窗口"}</small>
      </header>
      <div className="metric-card__value">
        <strong>{formatPercent(value.remainingPercent)}</strong>
        <span>剩余</span>
        {value.remainingPercent <= 0 && (quota.windowDurationMins === 300 || quota.windowDurationMins === 10_080) && (
          <small className="quota-limit-alert">已达 {formatWindowDuration(quota.windowDurationMins)}上限</small>
        )}
      </div>
      <div
        className="quota-track"
        role="progressbar"
        aria-label={`${title}额度剩余`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={value.remainingPercent}
      >
        <span style={{ width: `${value.remainingPercent}%` }} />
      </div>
      <footer>
        <span>已使用 {formatPercent(value.usedPercent)}</span>
        <span>{formatCountdown(quota.resetsAt, now)}重置</span>
      </footer>
    </article>
  );
}

function MetricCard({
  className,
  eyebrow,
  value,
  detail,
  icon,
  tone,
}: {
  className?: string;
  eyebrow: string;
  value: string;
  detail: string;
  icon: IconName;
  tone: "cyan" | "green";
}) {
  return (
    <article className={`metric-card metric-card--${tone}${className ? ` ${className}` : ""}`}>
      <header>
        <span>
          <Icon name={icon} />
          {eyebrow}
        </span>
      </header>
      <div className="metric-card__value">
        <strong>{value}</strong>
      </div>
      <footer>
        <span>{detail}</span>
      </footer>
    </article>
  );
}

function TrendCard({
  title,
  subtitle,
  buckets,
  onOpenDay,
  color,
}: {
  title: string;
  subtitle: string;
  buckets: Array<{ date: string; tokens: number; segments?: SourceTrendSegment[] }>;
  onOpenDay?: (date: string) => void;
  color?: string;
}) {
  const visible = buckets.slice(-30);
  const max = Math.max(1, ...visible.map((bucket) => bucket.tokens));
  const peak = visible.reduce<{ date: string; tokens: number } | null>(
    (current, bucket) =>
      !current || bucket.tokens > current.tokens ? bucket : current,
    null,
  );
  const latest = visible.at(-1);
  const previous = visible.at(-2);
  const change =
    latest && previous && previous.tokens > 0
      ? ((latest.tokens - previous.tokens) / previous.tokens) * 100
      : null;
  return (
    <article className="content-card trend-card">
      <header>
        <div>
          <p className="eyebrow">趋势</p>
          <h2>{title}</h2>
          <p>{subtitle}</p>
        </div>
        <div className="trend-meta">
          {change !== null && (
            <span
              className={`change-pill ${change > 0 ? "is-up" : change < 0 ? "is-down" : ""}`}
            >
              较前一日 {change > 0 ? "+" : ""}
              {change.toFixed(1)}%
            </span>
          )}
          {peak && (
            <span className="peak-pill">
              峰值 {formatTokens(peak.tokens)} · {peak.date.slice(5)}
            </span>
          )}
        </div>
      </header>
      {visible.length > 0 ? (
        <div className="trend-chart" role="list" aria-label={title}>
          {visible.map((bucket) => {
            const positive = bucket.segments?.filter((segment) => segment.tokens > 0) ?? [];
            const details = positive.map((segment) => `${segment.label} ${formatChineseCompactTokens(segment.tokens)}`).join(" · ");
            const tooltip = `${bucket.date} · 总计 ${formatChineseCompactTokens(bucket.tokens)}${details ? ` · ${details}` : ""}`;
            return (
            <button
              key={bucket.date}
              type="button"
              role="listitem"
              className="trend-bar"
              style={
                {
                  "--bar-height": `${bucket.tokens > 0 ? Math.max(3, (bucket.tokens / max) * 100) : 0}%`,
                } as CSSProperties
              }
              data-value={formatChineseCompactTokens(bucket.tokens)}
              title={tooltip}
              onClick={() => onOpenDay?.(bucket.date)}
              disabled={!onOpenDay}
              aria-label={`${bucket.date}，${formatChineseCompactTokens(bucket.tokens)} Token`}
            >
              {bucket.tokens > 0 && (
                <span className={positive.length > 0 ? "token-source-stack" : undefined} style={!positive.length && color ? { background: color } : undefined}>
                  {positive.map((segment) => <i key={segment.sourceId} style={{ background: segment.color, flexGrow: segment.tokens }} />)}
                </span>
              )}
              <small>{bucket.date.slice(5).replace("-", "/")}</small>
            </button>
            );
          })}
        </div>
      ) : (
        <EmptyState
          title="暂无趋势数据"
          description="完成用量读取后，这里会显示每日变化。"
        />
      )}
    </article>
  );
}

function AccountFacts({
  snapshot,
  now,
}: {
  snapshot: DashboardSnapshot;
  now: number;
}) {
  const credits = snapshot.quotaWindows.find((quota) => quota.credits)?.credits;
  return (
    <article className="content-card account-facts">
      <header>
        <div>
          <p className="eyebrow">账号信息</p>
          <h2>服务端附加信息</h2>
        </div>
      </header>
      <dl>
        <div>
          <dt>工作区 Credits</dt>
          <dd>
            {credits?.unlimited ? "不限量" : (credits?.balance ?? "未提供")}
          </dd>
          <small>单独计价，不计入 Token</small>
        </div>
        <div>
          <dt>额度重置券</dt>
          <dd>{snapshot.resetCredits?.availableCount ?? 0} 张</dd>
          <small>不等同于 Credits 余额</small>
        </div>
        <div>
          <dt>套餐续费</dt>
          <dd>
            {snapshot.planRenewalAt
              ? formatDateTime(snapshot.planRenewalAt)
              : "未提供"}
          </dd>
          <small>
            {snapshot.planRenewalSource === "manual"
              ? "手动设置 · 仅本机"
              : "等待官方接口"}
          </small>
        </div>
        <div>
          <dt>最近更新</dt>
          <dd>{formatRelativeTime(snapshot.fetchedAt, now)}</dd>
          <small>
            {snapshot.codexVersion
              ? `Codex ${snapshot.codexVersion}`
              : "版本未知"}
          </small>
        </div>
      </dl>
    </article>
  );
}

interface FilterProps {
  usage: DashboardSnapshot["deviceUsage"];
  range: UsageRange;
  onRangeChange: (range: UsageRange) => void;
  selectedSources: SourceSelection;
  selectionMode: SourceSelectionMode;
  onSelectionModeChange: (mode: SourceSelectionMode) => void;
  onSourcesChange: (source: UsageSourceSummary["id"]) => void;
  onClearSources: () => void;
  customStartDate: string;
  customEndDate: string;
  appliedCustomStartDate: string;
  appliedCustomEndDate: string;
  onCustomStartDateChange: (value: string) => void;
  onCustomEndDateChange: (value: string) => void;
  onApplyCustomRange: () => void;
  isLoadingCustomRange: boolean;
  customRangeError: string | null;
}

export function ActivityView({
  usage,
  sourceHealth,
  range,
  onRangeChange,
  selectedSources,
  selectionMode,
  onSelectionModeChange,
  onSourcesChange,
  onClearSources,
  customStartDate,
  customEndDate,
  appliedCustomStartDate,
  appliedCustomEndDate,
  onCustomStartDateChange,
  onCustomEndDateChange,
  onApplyCustomRange,
  isLoadingCustomRange,
  customRangeError,
  summary,
  insightSummary,
  now,
  onOpenDay,
  projectMergeRules,
  projectMergeMessage,
  onSaveProjectMergeRules,
}: FilterProps & {
  sourceHealth: SourceHealthSummary[];
  summary: DeviceUsageRangeSummary | null;
  insightSummary: DeviceUsageRangeSummary | null;
  now: number;
  onOpenDay: (date: string) => void;
  projectMergeRules: ProjectMergeRule[];
  projectMergeMessage: string | null;
  onSaveProjectMergeRules: (rules: ProjectMergeRule[]) => Promise<ProjectMergeRule[]>;
}) {
  const [activityMode, setActivityMode] = useState<"all" | "codex">("all");
  const [codexRange, setCodexRange] = useState<Exclude<UsageRange, "custom">>("today");
  return (
    <section className="view-panel">
      <PageHeading
        eyebrow="LOCAL ACTIVITY"
        title="本机活动"
        description="只汇总当前电脑可读取的派生数字，不代表账号套餐额度或其他设备。"
      />
      <div className="activity-view-switch" role="group" aria-label="本机活动视图">
        <button type="button" className={activityMode === "all" ? "is-active" : ""} aria-pressed={activityMode === "all"} onClick={() => setActivityMode("all")}>来源总览</button>
        <button type="button" className={activityMode === "codex" ? "is-active" : ""} aria-pressed={activityMode === "codex"} onClick={() => setActivityMode("codex")}>Codex Token</button>
      </div>
      {usage && activityMode === "codex" ? (
        <CodexTokenView usage={usage} range={codexRange} onRangeChange={setCodexRange} />
      ) : usage ? (
        <>
          <UnifiedFilters
            usage={usage}
            range={range}
            onRangeChange={onRangeChange}
            selectedSources={selectedSources}
            selectionMode={selectionMode}
            onSelectionModeChange={onSelectionModeChange}
            onSourcesChange={onSourcesChange}
            onClearSources={onClearSources}
            customStartDate={customStartDate}
            customEndDate={customEndDate}
            appliedCustomStartDate={appliedCustomStartDate}
            appliedCustomEndDate={appliedCustomEndDate}
            onCustomStartDateChange={onCustomStartDateChange}
            onCustomEndDateChange={onCustomEndDateChange}
            onApplyCustomRange={onApplyCustomRange}
            isLoadingCustomRange={isLoadingCustomRange}
            customRangeError={customRangeError}
          />
          {summary && (
            <ActivityDashboard
              summary={summary}
              sources={usage.sources}
              range={range}
              rangeLabel={formatRangeLabel(range, appliedCustomStartDate, appliedCustomEndDate)}
              now={now}
              selectedSources={selectedSources}
              onOpenDay={onOpenDay}
              projectMergeRules={projectMergeRules}
              projectMergeMessage={projectMergeMessage}
              onSaveProjectMergeRules={onSaveProjectMergeRules}
            />
          )}
          {insightSummary && <InsightCard summary={insightSummary} onOpenDay={onOpenDay} />}
          <SourceHealthGrid
            sources={usage.sources}
            health={sourceHealth}
            generatedAt={usage.generatedAt}
          />
          {usage.warnings.length > 0 && (
            <div className="warning-list" role="status">
              <Icon name="info" />
              <div>
                {usage.warnings.map((warning) => (
                  <p key={warning}>{warning}</p>
                ))}
              </div>
            </div>
          )}
        </>
      ) : (
        <EmptyState
          title="尚未建立本机索引"
          description="首次扫描完成后，这里会显示统一的时间范围、来源和趋势。"
        />
      )}
    </section>
  );
}

function CodexTokenView({
  usage,
  range,
  onRangeChange,
}: {
  usage: DeviceUsageSummary;
  range: Exclude<UsageRange, "custom">;
  onRangeChange: (range: Exclude<UsageRange, "custom">) => void;
}) {
  const codex = usage.sources.find((source) => source.id === "codex");
  if (!codex) return <EmptyState title="尚无 Codex 本机数据" description="检测到 Codex 本机日志后，这里会显示 Token 构成。" />;
  const summary = codex.ranges?.[range] ?? (range === "today"
    ? { usage: codex.today, cost: codex.todayCost, byModel: codex.todayByModel, byProject: [], dailyUsage: [] }
    : { usage: codex.total, cost: codex.cost, byModel: codex.byModel, byProject: codex.byProject, dailyUsage: [] });
  const categories = createTokenCategories(summary.usage);
  const cacheBasis = summary.usage.inputTokens + summary.usage.cachedInputTokens + summary.usage.cacheWriteTokens;
  const cacheShare = cacheBasis > 0 ? summary.usage.cachedInputTokens / cacheBasis : null;
  const rangeLabel = RANGE_ITEMS.find((item) => item.id === range)?.label ?? "所选范围";
  return (
    <div className="codex-token-view">
      <div className="codex-range-control segmented-control" role="group" aria-label="Codex Token 时间范围">
        {RANGE_ITEMS.filter((item): item is typeof RANGE_ITEMS[number] & { id: Exclude<UsageRange, "custom"> } => item.id !== "custom").map((item) => (
          <button key={item.id} type="button" className={range === item.id ? "is-active" : ""} aria-pressed={range === item.id} onClick={() => onRangeChange(item.id)}>{item.short}</button>
        ))}
      </div>
      <div className="codex-token-grid">
        <article className="content-card token-card codex-token-card">
          <header>
            <div><p className="eyebrow">Codex · {rangeLabel}</p><h2>{formatTokens(summary.usage.totalTokens)} Token</h2></div>
            <span className="soft-pill">缓存读入 {cacheShare === null ? "—" : formatPercent(cacheShare * 100)}</span>
          </header>
          <div className="token-body">
            <div className="token-donut" role="img" aria-label={categories.map((item) => `${item.label}${formatExactTokens(item.value)}`).join("，")} style={{ background: createDonutGradient(categories) }}>
              <div><strong>{cacheShare === null ? "—" : formatPercent(cacheShare * 100)}</strong><span>缓存读入占比</span></div>
            </div>
            <ul>{categories.map((item, index) => <li key={item.label}><span className="legend-dot" style={{ background: TOKEN_COLORS[index] }} /><span>{item.label}</span><strong>{formatTokens(item.value)}</strong></li>)}</ul>
          </div>
        </article>
        <article className="content-card codex-model-card">
          <header><div><p className="eyebrow">MODEL USAGE</p><h2>Codex 模型消耗</h2><p>按当前时间范围内的 Token 从高到低排列。</p></div></header>
          {summary.byModel.length > 0 ? <ol>{summary.byModel.map((model) => <li key={model.key}><div><strong>{model.label}</strong><span>{formatTokens(model.usage.totalTokens)} Token</span></div><b>{formatPercent(summary.usage.totalTokens > 0 ? model.usage.totalTokens / summary.usage.totalTokens * 100 : 0)}</b></li>)}</ol> : <p className="muted-copy">该时间范围暂无模型明细。</p>}
        </article>
      </div>
      <div className="privacy-callout"><Icon name="shield" /><div><strong>只读本机 Codex 用量</strong><p>此视图读取本机派生日志，不会向 Codex 创建对话、发送消息或发起推理。</p></div></div>
    </div>
  );
}

function UnifiedFilters({
  usage,
  range,
  onRangeChange,
  selectedSources,
  selectionMode,
  onSelectionModeChange,
  onSourcesChange,
  onClearSources,
  customStartDate,
  customEndDate,
  appliedCustomStartDate,
  appliedCustomEndDate,
  onCustomStartDateChange,
  onCustomEndDateChange,
  onApplyCustomRange,
  isLoadingCustomRange,
  customRangeError,
}: FilterProps) {
  if (!usage) return null;
  return (
    <div className="filter-panel">
      <div className="filter-row">
        <span>时间范围</span>
        <div
          className="segmented-control"
          role="group"
          aria-label="本机统计时间范围"
        >
          {RANGE_ITEMS.filter((item) => item.id !== "custom").map((item) => (
            <button
              key={item.id}
              type="button"
              className={range === item.id ? "is-active" : ""}
              aria-pressed={range === item.id}
              onClick={() => onRangeChange(item.id)}
            >
              {item.short}
            </button>
          ))}
        </div>
      </div>
      <div className="filter-row filter-row--custom">
        <span>自定义</span>
        <div className="custom-range-controls">
          <label>
            <span className="sr-only">开始日期</span>
            <input type="date" value={customStartDate} onChange={(event) => onCustomStartDateChange(event.target.value)} />
          </label>
          <span>至</span>
          <label>
            <span className="sr-only">结束日期</span>
            <input type="date" value={customEndDate} onChange={(event) => onCustomEndDateChange(event.target.value)} />
          </label>
          <button type="button" className={range === "custom" ? "is-active" : ""} aria-pressed={range === "custom"} disabled={isLoadingCustomRange} onClick={onApplyCustomRange}>
            {isLoadingCustomRange ? "读取中…" : "应用日期"}
          </button>
        </div>
      </div>
      {customRangeError && <p className="filter-error" role="alert">{customRangeError}</p>}
      <div className="filter-row filter-row--sources">
        <span>AI 来源</span>
        <div className="source-chips">
          <div className="source-mode-control" role="group" aria-label="AI 来源选择模式">
            <button
              type="button"
              className={selectionMode === "single" ? "is-active" : ""}
              aria-pressed={selectionMode === "single"}
              onClick={() => onSelectionModeChange("single")}
            >
              单选
            </button>
            <button
              type="button"
              className={selectionMode === "multiple" ? "is-active" : ""}
              aria-pressed={selectionMode === "multiple"}
              onClick={() => onSelectionModeChange("multiple")}
            >
              多选
            </button>
          </div>
          <button
            type="button"
            className={`source-choice source-choice--all${selectedSources.length === 0 ? " is-active" : ""}`}
            aria-pressed={selectedSources.length === 0}
            onClick={onClearSources}
          >
            全部来源
          </button>
          {usage.sources.map((source) => (
            <button
              key={source.id}
              type="button"
              className={`source-choice source-chip${selectedSources.includes(source.id) ? " is-active" : ""}`}
              style={{ "--source-color": SOURCE_TREND_COLORS[source.id] ?? "#8b95aa" } as CSSProperties}
              aria-label={`${source.label}，状态：${sourceStatusLabel(source.status)}`}
              aria-pressed={selectedSources.includes(source.id)}
              title={`${source.label} · 状态：${sourceStatusLabel(source.status)}`}
              onClick={() => onSourcesChange(source.id)}
            >
              <span className="source-color-swatch" aria-hidden="true" />
              <span className={`source-dot source-dot--${source.status}`} aria-hidden="true" />
              {source.label}
            </button>
          ))}
          {selectedSources.length > 0 && (
            <button
              className="clear-filter"
              type="button"
              onClick={onClearSources}
            >
              一键清除
            </button>
          )}
        </div>
      </div>
      <p className="filter-summary">
        当前：{formatRangeLabel(range, appliedCustomStartDate, appliedCustomEndDate)} ·{" "}
        {selectedSources.length === 0
          ? "全部来源"
          : usage.sources
              .filter((source) => selectedSources.includes(source.id))
              .map((source) => source.label)
              .join("、")}
        {selectedSources.length > 0 ? `（已选 ${selectedSources.length} 个来源）` : ""}
      </p>
    </div>
  );
}

function ActivityDashboard({
  summary,
  sources,
  range,
  rangeLabel,
  now,
  selectedSources,
  onOpenDay,
  projectMergeRules,
  projectMergeMessage,
  onSaveProjectMergeRules,
}: {
  summary: DeviceUsageRangeSummary;
  sources: UsageSourceSummary[];
  range: UsageRange;
  rangeLabel: string;
  now: number;
  selectedSources: SourceSelection;
  onOpenDay: (date: string) => void;
  projectMergeRules: ProjectMergeRule[];
  projectMergeMessage: string | null;
  onSaveProjectMergeRules: (rules: ProjectMergeRule[]) => Promise<ProjectMergeRule[]>;
}) {
  const [mergeManagerOpen, setMergeManagerOpen] = useState(false);
  const [selectedDimension, setSelectedDimension] = useState<{
    kind: "model" | "project";
    item: SourceStackedDimension;
  } | null>(null);
  useEffect(() => setSelectedDimension(null), [range, selectedSources]);
  const dailyTrend = useMemo(
    () => buildDailySourceTrend(sources, selectedSources, range),
    [range, selectedSources, sources],
  );
  const modelDimensions = useMemo(
    () => buildSourceDimensionRows(sources, selectedSources, range, "byModel"),
    [range, selectedSources, sources],
  );
  const projectDimensions = useMemo(
    () => buildProjectDimensionRows(sources, selectedSources, range, projectMergeRules),
    [projectMergeRules, range, selectedSources, sources],
  );
  const projectCatalog = useMemo(() => buildProjectCatalog(sources), [sources]);
  const categories = createTokenCategories(summary.usage);
  const cacheBasis =
    summary.usage.inputTokens +
    summary.usage.cachedInputTokens +
    summary.usage.cacheWriteTokens;
  const cacheShare =
    cacheBasis > 0 ? summary.usage.cachedInputTokens / cacheBasis : null;
  return (
    <div className="activity-grid">
      <article className="content-card token-card">
        <header>
          <div>
            <p className="eyebrow">
              {selectedSources.length === 0
                ? "全部来源"
                : `${selectedSources.length} 个来源`}{" "}
              · {rangeLabel}
            </p>
            <h2 title={formatExactTokens(summary.usage.totalTokens)}>
              {formatTokens(summary.usage.totalTokens)} Token
            </h2>
          </div>
          <span className="soft-pill">
            缓存读入{" "}
            {cacheShare === null ? "—" : formatPercent(cacheShare * 100)}
          </span>
        </header>
        <div className="token-body">
          <div
            className="token-donut"
            role="img"
            aria-label={categories
              .map((item) => `${item.label}${formatExactTokens(item.value)}`)
              .join("，")}
            style={{ background: createDonutGradient(categories) }}
          >
            <div>
              <strong>
                {cacheShare === null ? "—" : formatPercent(cacheShare * 100)}
              </strong>
              <span>缓存读入占比</span>
            </div>
          </div>
          <ul>
            {categories.map((item, index) => (
              <li key={item.label}>
                <span
                  className="legend-dot"
                  style={{ background: TOKEN_COLORS[index] }}
                />
                <span>{item.label}</span>
                <strong>{formatTokens(item.value)}</strong>
              </li>
            ))}
          </ul>
        </div>
      </article>
      {range === "today" ? (
        <HourlyTrendCard sources={sources} selectedSources={selectedSources} now={now} onOpenDay={onOpenDay} />
      ) : (
        <TrendCard
          title={`${rangeLabel}趋势`}
          subtitle="只包含有可靠本地时间的派生事件"
          buckets={dailyTrend}
          onOpenDay={onOpenDay}
        />
      )}
      <DimensionCard title="按模型" kind="model" dimensions={modelDimensions} selectedKey={selectedDimension?.kind === "model" ? selectedDimension.item.key : null} onSelect={(item) => setSelectedDimension({ kind: "model", item })} />
      <DimensionCard title="按项目" kind="project" dimensions={projectDimensions} selectedKey={selectedDimension?.kind === "project" ? selectedDimension.item.key : null} onSelect={(item) => setSelectedDimension({ kind: "project", item })} onManage={() => setMergeManagerOpen(true)} />
      {projectMergeMessage && <p className="project-merge-warning" role="status">{projectMergeMessage}</p>}
      {selectedDimension && (
        <article className="content-card dimension-drilldown">
          <header>
            <div>
              <p className="eyebrow">{selectedDimension.kind === "model" ? "模型下钻" : "项目下钻"}</p>
              <h2>{selectedDimension.item.label}</h2>
              <p>当前来源与日期范围内的归因汇总。</p>
            </div>
            <button type="button" onClick={() => setSelectedDimension(null)}>清除下钻</button>
          </header>
          <div className="diagnostic-metrics">
            <div><span>Token</span><strong>{formatExactTokens(selectedDimension.item.usage.totalTokens)}</strong></div>
            <div><span>当前占比</span><strong>{formatPercent(summary.usage.totalTokens > 0 ? selectedDimension.item.usage.totalTokens / summary.usage.totalTokens * 100 : 0)}</strong></div>
            <div><span>估算费用</span><strong>{formatCostCompact(selectedDimension.item.cost)}</strong></div>
          </div>
        </article>
      )}
      <div className="low-cache-action">
        <span>缓存命中率低于设置阈值的任务会在日明细中标红。</span>
        <button
          className="low-cache-button"
          type="button"
          onClick={() => onOpenDay(localDateKey(new Date()))}
        >
          查看今日低缓存任务
        </button>
      </div>
      {mergeManagerOpen && (
        <ProjectMergeManager
          catalog={projectCatalog}
          rules={projectMergeRules}
          onSave={onSaveProjectMergeRules}
          onClose={() => setMergeManagerOpen(false)}
        />
      )}
    </div>
  );
}

function HourlyTrendCard({
  sources,
  selectedSources,
  now,
  onOpenDay,
}: {
  sources: UsageSourceSummary[];
  selectedSources: SourceSelection;
  now: number;
  onOpenDay: (date: string) => void;
}) {
  const buckets = buildRollingHourlySourceTrend(sources, selectedSources, now);
  const max = Math.max(1, ...buckets.map((bucket) => bucket.tokens));
  const today = localDateKey(new Date(now));
  return (
    <article className="content-card trend-card hourly-trend-card">
      <header>
        <div>
          <p className="eyebrow">滚动趋势</p>
          <h2>过去 24 小时使用量</h2>
          <p>按本机小时汇总，跨越自然日时仍连续显示。</p>
        </div>
      </header>
      <div className="trend-chart trend-chart--hourly" role="list" aria-label="过去 24 小时使用量">
        {buckets.map((bucket, index) => {
          const time = new Date(bucket.hourStartMs);
          const date = localDateKey(time);
          const hour = `${String(time.getHours()).padStart(2, "0")}:00`;
          const label = date === today ? hour : `昨天 ${hour}`;
          const showLabel = index === 0 || index === buckets.length - 1 || time.getHours() % 2 === 0;
          const details = bucket.segments
            .filter((segment) => segment.tokens > 0)
            .map((segment) => `${segment.label} ${formatChineseCompactTokens(segment.tokens)}`)
            .join(" · ");
          const tooltip = `${date} ${hour} · 总计 ${formatChineseCompactTokens(bucket.tokens)}${details ? ` · ${details}` : ""}`;
          return (
            <button
              key={bucket.hourStartMs}
              type="button"
              role="listitem"
              className={`trend-bar${bucket.tokens === 0 ? " trend-bar--empty" : ""}`}
              style={{ "--bar-height": `${bucket.tokens > 0 ? Math.max(3, (bucket.tokens / max) * 100) : 0}%` } as CSSProperties}
              data-value={formatChineseCompactTokens(bucket.tokens)}
              title={tooltip}
              aria-label={tooltip}
              onClick={() => onOpenDay(date)}
            >
              {bucket.tokens > 0 && (
                <span className="hourly-trend__stack" aria-hidden="true">
                  {bucket.segments.filter((segment) => segment.tokens > 0).map((segment) => (
                    <i
                      key={segment.sourceId}
                      style={{ background: segment.color, flexGrow: segment.tokens }}
                    />
                  ))}
                </span>
              )}
              <small>{showLabel ? label : ""}</small>
            </button>
          );
        })}
      </div>
    </article>
  );
}

function InsightCard({ summary, onOpenDay }: { summary: DeviceUsageRangeSummary; onOpenDay: (date: string) => void }) {
  const insight = buildUsageInsights(summary);
  const confidence =
    insight.confidence === "high"
      ? "高"
      : insight.confidence === "medium"
        ? "中"
        : "低";
  const trend =
    insight.trend === "up"
      ? `近期日均上升 ${Math.abs(insight.changePercent ?? 0).toFixed(1)}%`
      : insight.trend === "down"
        ? `近期日均下降 ${Math.abs(insight.changePercent ?? 0).toFixed(1)}%`
        : insight.trend === "stable"
          ? "近期日均基本稳定"
          : "数据积累中";
  const anomaly =
    insight.anomaly === "spike"
      ? "最新一天明显高于历史日均"
      : insight.anomaly === "drop"
        ? "最新一天明显低于历史日均"
        : insight.anomaly === "normal"
          ? "最新一天未见明显异常"
          : "至少积累 7 天后识别异常";
  return (
    <article className="content-card insight-card">
      <header>
        <div>
          <p className="eyebrow">LOCAL INSIGHTS · 30 天</p>
          <h2>趋势洞察</h2>
          <p>根据当前来源的本机日汇总生成，不上传会话内容。</p>
        </div>
        <span
          className={`confidence-pill confidence-pill--${insight.confidence}`}
        >
          置信度 {confidence}
        </span>
      </header>
      <div className="insight-grid">
        <div>
          <span>趋势信号</span>
          <strong>{trend}</strong>
          <small>对比最近 7 天与此前 7 天；数据不足时采用短期基线。</small>
        </div>
        <div>
          <span>异常信号</span>
          <strong>{anomaly}</strong>
          <small>仅标记相对历史日均的明显偏离。</small>
        </div>
        <div>
          <span>30 天峰值</span>
          <strong>
            {insight.peakDate
              ? `${formatTokens(insight.peakTokens)} · ${insight.peakDate.slice(5)}`
              : "尚无数据"}
          </strong>
          <small>已观察 {insight.observedDays} 个有效日期。</small>
        </div>
      </div>
      <footer>
        <span>这是历史信号，不是额度预测、账单承诺或跨设备总量。</span>
        {insight.peakDate && <button type="button" onClick={() => onOpenDay(insight.peakDate!)}>查看峰值构成</button>}
      </footer>
    </article>
  );
}

function DimensionCard({
  title,
  kind,
  dimensions,
  selectedKey,
  onSelect,
  onManage,
}: {
  title: string;
  kind: "model" | "project";
  dimensions: SourceStackedDimension[];
  selectedKey: string | null;
  onSelect: (item: SourceStackedDimension) => void;
  onManage?: () => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const sorted = [...dimensions].sort(
    (left, right) => right.usage.totalTokens - left.usage.totalTokens,
  );
  const shown = expanded ? sorted : sorted.slice(0, 5);
  const max = Math.max(1, ...sorted.map((item) => item.usage.totalTokens));
  return (
    <article className="content-card dimension-card">
      <header>
        <div>
          <p className="eyebrow">归因明细</p>
          <h2>{title}</h2>
        </div>
        <div className="dimension-card__actions">
          <span className="soft-pill">{sorted.length} 项</span>
          {onManage && <button type="button" onClick={onManage}>管理合并</button>}
        </div>
      </header>
      {shown.length > 0 ? (
        <ol>
          {shown.map((item, index) => (
            <li key={item.key} className={selectedKey === item.key ? "is-selected" : ""}>
              <button type="button" aria-pressed={selectedKey === item.key} aria-label={`查看${kind === "model" ? "模型" : "项目"} ${item.label} 的归因汇总`} onClick={() => onSelect(item)}>
                <div>
                <span>{String(index + 1).padStart(2, "0")}</span>
                <strong title={item.memberLabels?.join("；") ?? item.label}>
                  {item.label}
                  {item.mergedMemberCount ? <em>已合并 {item.mergedMemberCount} 项</em> : null}
                </strong>
                <b title={formatExactTokens(item.usage.totalTokens)}>
                  {formatTokens(item.usage.totalTokens)}
                </b>
                </div>
                <div className="dimension-track" title={item.segments.filter((segment) => segment.tokens > 0).map((segment) => `${segment.label} ${formatChineseCompactTokens(segment.tokens)}`).join(" · ")}>
                  <span className="dimension-track__fill"
                    style={{
                      width: `${Math.max(3, (item.usage.totalTokens / max) * 100)}%`,
                    }}
                  >
                    {item.segments.filter((segment) => segment.tokens > 0).map((segment) => (
                      <i key={segment.sourceId} style={{ background: segment.color, flexGrow: segment.tokens }} />
                    ))}
                  </span>
                </div>
              </button>
            </li>
          ))}
        </ol>
      ) : (
        <EmptyState
          title="暂无归因数据"
          description="当前范围内没有可归属的模型或项目。"
        />
      )}
      {sorted.length > 5 && (
        <button
          className="text-button"
          type="button"
          onClick={() => setExpanded((value) => !value)}
        >
          {expanded ? "收起" : `查看其余 ${sorted.length - 5} 项`}
        </button>
      )}
    </article>
  );
}

function ProjectMergeManager({
  catalog,
  rules,
  onSave,
  onClose,
}: {
  catalog: ProjectMergeMember[];
  rules: ProjectMergeRule[];
  onSave: (rules: ProjectMergeRule[]) => Promise<ProjectMergeRule[]>;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [displayMemberId, setDisplayMemberId] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const available = useMemo(() => {
    const values = new Map(catalog.map((member) => [projectMemberIdentity(member), member]));
    for (const member of rules.flatMap((rule) => rule.members)) {
      if (!values.has(projectMemberIdentity(member))) values.set(projectMemberIdentity(member), member);
    }
    return [...values.values()];
  }, [catalog, rules]);
  const assignedByOther = useMemo(() => new Set(rules
    .filter((rule) => rule.id !== editingId)
    .flatMap((rule) => rule.members.map(projectMemberIdentity))), [editingId, rules]);
  const selectedMembers = available.filter((member) => selected.includes(projectMemberIdentity(member)));
  const visible = available.filter((member) => {
    const needle = query.trim().toLocaleLowerCase();
    return !needle || `${member.projectLabel} ${member.sourceLabel}`.toLocaleLowerCase().includes(needle);
  });

  const resetForm = () => {
    setEditingId(null);
    setSelected([]);
    setDisplayMemberId(null);
    setQuery("");
    setMessage(null);
  };
  const editRule = (rule: ProjectMergeRule) => {
    const identities = rule.members.map(projectMemberIdentity);
    const displayMember = rule.members.find((member) => member.projectLabel === rule.displayName);
    setEditingId(rule.id);
    setSelected(identities);
    setDisplayMemberId(displayMember ? projectMemberIdentity(displayMember) : identities[0] ?? null);
    setMessage(null);
  };
  const toggleMember = (member: ProjectMergeMember) => {
    const identity = projectMemberIdentity(member);
    setSelected((current) => {
      const next = current.includes(identity) ? current.filter((value) => value !== identity) : [...current, identity];
      if (displayMemberId === identity && !next.includes(identity)) setDisplayMemberId(next[0] ?? null);
      if (!displayMemberId && next.length > 0) setDisplayMemberId(next[0]);
      return next;
    });
  };
  const persistForm = async () => {
    if (selectedMembers.length < 2) return setMessage("请至少选择两个项目。");
    const displayMember = selectedMembers.find((member) => projectMemberIdentity(member) === displayMemberId);
    if (!displayMember) return setMessage("请选择一个成员原名作为合并后名称。");
    const id = editingId ?? (globalThis.crypto?.randomUUID?.() ?? `project-merge-${Date.now()}`);
    const rule: ProjectMergeRule = { id, displayName: displayMember.projectLabel, members: selectedMembers };
    const next = editingId ? rules.map((item) => item.id === editingId ? rule : item) : [...rules, rule];
    setSaving(true);
    setMessage(null);
    try {
      await onSave(next);
      resetForm();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "无法保存项目合并设置。");
    } finally {
      setSaving(false);
    }
  };
  const removeRule = async (id: string) => {
    setSaving(true);
    setMessage(null);
    try {
      await onSave(rules.filter((rule) => rule.id !== id));
      if (editingId === id) resetForm();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "无法取消项目合并。");
    } finally {
      setSaving(false);
    }
  };

  useEffect(() => {
    const previousFocus = document.activeElement as HTMLElement | null;
    closeRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !saving) {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "Tab" || !dialogRef.current) return;
      const focusable = [...dialogRef.current.querySelectorAll<HTMLElement>('button:not([disabled]),input:not([disabled]),[tabindex]:not([tabindex="-1"])')];
      if (!focusable.length) return;
      if (event.shiftKey && document.activeElement === focusable[0]) {
        event.preventDefault();
        focusable.at(-1)?.focus();
      } else if (!event.shiftKey && document.activeElement === focusable.at(-1)) {
        event.preventDefault();
        focusable[0].focus();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      previousFocus?.focus();
    };
  }, [onClose, saving]);

  return (
    <div className="project-merge-backdrop" role="presentation" onMouseDown={() => !saving && onClose()}>
      <section ref={dialogRef} className="project-merge-dialog" role="dialog" aria-modal="true" aria-labelledby="project-merge-title" onMouseDown={(event) => event.stopPropagation()}>
        <header>
          <div><p className="eyebrow">仅更改界面汇总</p><h2 id="project-merge-title">项目合并管理</h2><p>规则只保存在应用设置中，不修改日志、索引或任务原名。</p></div>
          <button ref={closeRef} type="button" aria-label="关闭项目合并管理" disabled={saving} onClick={onClose}><Icon name="x" /></button>
        </header>
        {rules.length > 0 && (
          <section className="project-merge-groups" aria-label="已有项目合并">
            <h3>已有合并</h3>
            {rules.map((rule) => (
              <div key={rule.id}>
                <span><strong>{rule.displayName}</strong><small>{rule.members.length} 个项目</small></span>
                <span><button type="button" disabled={saving} onClick={() => editRule(rule)}>编辑</button><button type="button" disabled={saving} onClick={() => void removeRule(rule.id)}>取消合并</button></span>
              </div>
            ))}
          </section>
        )}
        <section className="project-merge-editor">
          <header><h3>{editingId ? "编辑合并" : "新建合并"}</h3>{editingId && <button type="button" onClick={resetForm}>退出编辑</button>}</header>
          <label className="project-search"><span className="sr-only">搜索历史项目</span><input type="search" value={query} placeholder="搜索项目名或 AI 来源" onChange={(event) => setQuery(event.target.value)} /></label>
          <div className="project-merge-list" role="group" aria-label="选择要合并的项目">
            {visible.map((member) => {
              const identity = projectMemberIdentity(member);
              const unavailable = assignedByOther.has(identity);
              return (
                <label key={identity} className={unavailable ? "is-disabled" : ""}>
                  <input type="checkbox" checked={selected.includes(identity)} disabled={unavailable || saving} onChange={() => toggleMember(member)} />
                  <span><strong>{member.projectLabel}</strong><small style={{ "--source-color": SOURCE_TREND_COLORS[member.sourceId] } as CSSProperties}>{member.sourceLabel}{unavailable ? " · 已在其他合并中" : ""}</small></span>
                </label>
              );
            })}
          </div>
          {selectedMembers.length > 0 && (
            <fieldset className="project-merge-names"><legend>合并后显示名称</legend>{selectedMembers.map((member) => {
              const identity = projectMemberIdentity(member);
              return <label key={identity}><input type="radio" name="project-merge-name" value={identity} checked={displayMemberId === identity} onChange={() => setDisplayMemberId(identity)} />{member.projectLabel}<small>{member.sourceLabel}</small></label>;
            })}</fieldset>
          )}
          {message && <p className="project-merge-error" role="alert">{message}</p>}
          <footer><span>已选 {selectedMembers.length} 项</span><button className="primary-button" type="button" disabled={saving || selectedMembers.length < 2 || !displayMemberId} onClick={() => void persistForm()}>{saving ? "保存中…" : editingId ? "保存修改" : "创建合并"}</button></footer>
        </section>
      </section>
    </div>
  );
}

function SourceHealthGrid({
  sources,
  health,
  generatedAt,
}: {
  sources: UsageSourceSummary[];
  health: SourceHealthSummary[];
  generatedAt: string;
}) {
  const healthById = new Map(health.map((item) => [item.id, item]));
  return (
    <section className="health-section">
      <header>
        <div>
          <p className="eyebrow">SOURCE HEALTH</p>
          <h2>来源健康中心</h2>
          <p>展示脱敏状态、最近成功索引及本轮明确新增或跳过的记录。</p>
        </div>
        <span className="soft-pill">更新于 {formatDateTime(generatedAt)}</span>
      </header>
      <div className="health-grid">
        {sources.map((source) => {
          const detail = healthById.get(source.id);
          const newEvents = detail?.newEvents ?? source.newEvents;
          const skipped = detail?.skippedRecords ?? source.skippedRecords;
          const lastIndexedAt = detail?.lastIndexedAt ?? source.lastIndexedAt;
          return (
            <article key={source.id} className="health-card">
              <header>
                <span className={`source-dot source-dot--${source.status}`} />
                <strong>{source.label}</strong>
                <small>{sourceStatusLabel(source.status)}</small>
              </header>
              <dl>
                <div>
                  <dt>索引文件</dt>
                  <dd>{source.sourceFiles.toLocaleString("zh-CN")}</dd>
                </div>
                <div>
                  <dt>派生事件</dt>
                  <dd>{source.indexedEvents.toLocaleString("zh-CN")}</dd>
                </div>
                <div>
                  <dt>本轮新增</dt>
                  <dd>
                    {newEvents === null
                      ? "—"
                      : newEvents.toLocaleString("zh-CN")}
                  </dd>
                </div>
                <div>
                  <dt>明确跳过</dt>
                  <dd>
                    {skipped === null ? "—" : skipped.toLocaleString("zh-CN")}
                  </dd>
                </div>
              </dl>
              <p>
                {source.message ??
                  (lastIndexedAt
                    ? `最近成功：${formatDateTime(lastIndexedAt)}`
                    : "尚无成功索引记录。")}
              </p>
            </article>
          );
        })}
      </div>
    </section>
  );
}

function mergeUsedModels(settings: PricingSettings, usage: DeviceUsageSummary | null): PricingSettings {
  if (!usage) return settings;
  const totals = new Map<string, { label: string; tokens: number }>();
  for (const source of usage.sources) {
    for (const model of source.byModel) {
      const identity = model.label.trim().toLowerCase();
      if (!identity) continue;
      const current = totals.get(identity);
      totals.set(identity, {
        label: current?.label ?? model.label,
        tokens: (current?.tokens ?? 0) + model.usage.totalTokens,
      });
    }
  }
  const saved = new Map(settings.models.map((model) => [model.modelId.trim().toLowerCase(), model]));
  const used = [...totals.entries()]
    .sort((left, right) => right[1].tokens - left[1].tokens)
    .map(([identity, value]) => saved.get(identity) ?? {
      modelId: value.label,
      displayName: value.label,
      currency: "USD" as const,
      inputPerMillion: null,
      cachedInputPerMillion: null,
      cacheWritePerMillion: null,
      outputPerMillion: null,
      peakEnabled: false,
    });
  const usedIds = new Set(used.map((model) => model.modelId.trim().toLowerCase()));
  return { ...settings, models: [...used, ...settings.models.filter((model) => !usedIds.has(model.modelId.trim().toLowerCase()))] };
}

export function PricingView({
  usage,
  settings,
  isSaving,
  message,
  onSave,
}: {
  usage: DeviceUsageSummary | null;
  settings: PricingSettings;
  isSaving: boolean;
  message: string | null;
  onSave: (settings: PricingSettings) => Promise<void>;
}) {
  const [draft, setDraft] = useState(() => mergeUsedModels(settings, usage));
  useEffect(() => setDraft(mergeUsedModels(settings, usage)), [settings, usage]);
  const tokenTotals = useMemo(() => {
    const totals = new Map<string, number>();
    for (const source of usage?.sources ?? []) {
      for (const model of source.byModel) {
        const identity = model.label.trim().toLowerCase();
        totals.set(identity, (totals.get(identity) ?? 0) + model.usage.totalTokens);
      }
    }
    return totals;
  }, [usage]);
  const updateModel = (modelId: string, patch: Partial<PricingSettings["models"][number]>) => {
    setDraft((current) => ({
      ...current,
      models: current.models.map((model) => model.modelId === modelId ? { ...model, ...patch } : model),
    }));
  };
  const rateInput = (
    model: PricingSettings["models"][number],
    field: "inputPerMillion" | "cachedInputPerMillion" | "cacheWritePerMillion" | "outputPerMillion",
    label: string,
  ) => (
    <label className="price-field">
      <span>{label}</span>
      <input
        type="number"
        min="0"
        step="0.001"
        inputMode="decimal"
        aria-label={`${model.displayName} ${label}`}
        placeholder="未设置"
        value={model[field] ?? ""}
        onChange={(event) => updateModel(model.modelId, { [field]: event.target.value === "" ? null : Number(event.target.value) })}
      />
    </label>
  );
  const usedModels = draft.models
    .filter((model) => tokenTotals.has(model.modelId.trim().toLowerCase()))
    .sort((left, right) => (tokenTotals.get(right.modelId.trim().toLowerCase()) ?? 0) - (tokenTotals.get(left.modelId.trim().toLowerCase()) ?? 0));
  const peakModelCount = usedModels.filter((model) => model.peakEnabled).length;
  return (
    <section className="view-panel">
      <PageHeading
        eyebrow="MANUAL API PRICING"
        title="模型定价"
        description="价格完全由你在本机维护；应用不再内置或自动回退到任何模型 API 价格。"
        actions={<button className="primary-button" type="button" disabled={isSaving || usedModels.length === 0} onClick={() => void onSave(draft)}>{isSaving ? "保存并重算中…" : "保存价格并重算"}</button>}
      />
      <article className="content-card peak-pricing-card">
        <header><div><p className="eyebrow">PEAK PRICING</p><h2>高峰期定价</h2><p>使用本机时间；只有加入下方白名单的模型才会乘以该倍率。</p></div><span className="soft-pill">跨午夜时段也支持</span></header>
        <div className="peak-pricing-fields">
          <label><span>开始时间</span><input type="time" value={draft.peak.startTime} onChange={(event) => setDraft((current) => ({ ...current, peak: { ...current.peak, startTime: event.target.value } }))} /></label>
          <label><span>结束时间</span><input type="time" value={draft.peak.endTime} onChange={(event) => setDraft((current) => ({ ...current, peak: { ...current.peak, endTime: event.target.value } }))} /></label>
          <label><span>收费倍率</span><div className="multiplier-input"><input type="number" min="1" max="100" step="0.1" value={draft.peak.multiplier} onChange={(event) => setDraft((current) => ({ ...current, peak: { ...current.peak, multiplier: Number(event.target.value) } }))} /><b>×</b></div></label>
        </div>
        <div className="peak-model-selector">
          <div className="peak-model-selector__heading">
            <div><strong>指定适用模型</strong><p>未选中的模型始终使用常规定价，不受高峰时段和倍率影响。</p></div>
            <span>{peakModelCount} 个模型</span>
          </div>
          <div className="peak-model-options" role="group" aria-label="高峰定价适用模型">
            {usedModels.map((model) => (
              <label className={`peak-model-option${model.peakEnabled ? " is-selected" : ""}`} key={model.modelId}>
                <input
                  type="checkbox"
                  aria-label={`${model.displayName} 高峰定价`}
                  checked={model.peakEnabled}
                  onChange={(event) => updateModel(model.modelId, { peakEnabled: event.target.checked })}
                />
                <span><strong>{model.displayName}</strong><small>{model.peakEnabled ? "应用高峰倍率" : "仅常规定价"}</small></span>
              </label>
            ))}
            {usedModels.length === 0 && <span className="muted-copy">发现使用过的模型后，可在这里指定高峰定价白名单。</span>}
          </div>
        </div>
      </article>
      <div className="pricing-page-note" role="note"><Icon name="info" /><span>下列模型来自本机已索引的真实用量，并按累计 Token 从高到低排列。单价单位均为“每 100 万 Token”。留空的类别会保留为未定价，不会按 0 元处理。</span></div>
      {message && <p className="pricing-save-message" role="status">{message}</p>}
      <div className="model-pricing-list">
        {usedModels.map((model, index) => {
          const tokens = tokenTotals.get(model.modelId.trim().toLowerCase()) ?? 0;
          const configured = [model.inputPerMillion, model.cachedInputPerMillion, model.cacheWritePerMillion, model.outputPerMillion].every((value) => value !== null);
          return (
            <article className="content-card model-price-card" key={model.modelId}>
              <header>
                <div className="model-price-rank"><span>{String(index + 1).padStart(2, "0")}</span><div><h2>{model.displayName}</h2><p>{formatTokens(tokens)} Token · 累计消耗</p></div></div>
                <div className="model-price-status"><span className={model.peakEnabled ? "is-peak-assigned" : "is-standard-only"}>{model.peakEnabled ? "高峰白名单" : "仅常规定价"}</span><span className={configured ? "is-configured" : "is-unpriced"}>{configured ? "已完整定价" : "仍有未定价项"}</span><label><span>币种</span><select aria-label={`${model.displayName} 币种`} value={model.currency} onChange={(event) => updateModel(model.modelId, { currency: event.target.value as "USD" | "CNY" })}><option value="USD">USD $</option><option value="CNY">CNY ¥</option></select></label></div>
              </header>
              <div className="model-price-fields">
                {rateInput(model, "inputPerMillion", "新输入")}
                {rateInput(model, "cachedInputPerMillion", "缓存读入")}
                {rateInput(model, "cacheWritePerMillion", "缓存写入")}
                {rateInput(model, "outputPerMillion", "输出")}
              </div>
            </article>
          );
        })}
      </div>
      {usedModels.length === 0 && <EmptyState title="尚未发现使用过的模型" description="完成一次本机用量扫描后，模型会自动出现在这里。" />}
      <div className="privacy-callout"><Icon name="shield" /><div><strong>定价设置只保存在本机</strong><p>保存后会重新汇总本机派生索引；不会修改来源日志，也不会向任何模型供应商发送数据。</p></div></div>
    </section>
  );
}

export function CostView({
  usage,
  range,
  onRangeChange,
  selectedSources,
  selectionMode,
  onSelectionModeChange,
  onSourcesChange,
  onClearSources,
  customStartDate,
  customEndDate,
  appliedCustomStartDate,
  appliedCustomEndDate,
  onCustomStartDateChange,
  onCustomEndDateChange,
  onApplyCustomRange,
  isLoadingCustomRange,
  customRangeError,
  summary,
  onOpenPricing,
}: FilterProps & { summary: DeviceUsageRangeSummary | null; onOpenPricing: () => void }) {
  if (!usage || !summary)
    return (
      <section className="view-panel">
        <PageHeading
          eyebrow="COST"
          title="费用"
          description="本机手动价格与未定价覆盖情况。"
        />
        <EmptyState
          title="暂无费用数据"
          description="本机索引完成后，这里会显示估算费用。"
        />
      </section>
    );
  const rows = costDetailRows(
    selectedSources.length === 0
      ? usage.sources
      : usage.sources.filter((source) => selectedSources.includes(source.id)),
    range,
  );
  const totals = aggregateCostRows(rows);
  const totalTokens = rows.reduce((sum, row) => sum + row.usage.totalTokens, 0);
  const pricedTokens = Math.max(0, totalTokens - totals.unpricedTokens);
  return (
    <section className="view-panel">
      <PageHeading
        eyebrow="COST ESTIMATE"
        title="费用"
        description="只按你保存的本机 API 单价估算，不代表订阅、Credits 或实际账单。"
        actions={<button className="secondary-button" type="button" onClick={onOpenPricing}>管理模型价格</button>}
      />
      <UnifiedFilters
        usage={usage}
        range={range}
        onRangeChange={onRangeChange}
        selectedSources={selectedSources}
        selectionMode={selectionMode}
        onSelectionModeChange={onSelectionModeChange}
        onSourcesChange={onSourcesChange}
        onClearSources={onClearSources}
        customStartDate={customStartDate}
        customEndDate={customEndDate}
        appliedCustomStartDate={appliedCustomStartDate}
        appliedCustomEndDate={appliedCustomEndDate}
        onCustomStartDateChange={onCustomStartDateChange}
        onCustomEndDateChange={onCustomEndDateChange}
        onApplyCustomRange={onApplyCustomRange}
        isLoadingCustomRange={isLoadingCustomRange}
        customRangeError={customRangeError}
      />
      <div className="cost-kpis">
        <MetricCard
          eyebrow="USD 估算"
          value={formatMoney("$", totals.usd)}
          detail={usage.priceSnapshotDate === "未保存" ? "尚未保存模型价格" : "按本机已保存价格重算"}
          icon="wallet"
          tone="green"
        />
        <MetricCard
          eyebrow="CNY 估算"
          value={formatMoney("¥", totals.cny)}
          detail="不同币种不进行自动换算"
          icon="wallet"
          tone="cyan"
        />
        <MetricCard
          eyebrow="已定价覆盖"
          value={formatPercent(
            totalTokens > 0 ? (pricedTokens / totalTokens) * 100 : 0,
          )}
          detail={`${formatTokens(pricedTokens)} Token 已匹配手动价格`}
          icon="check"
          tone="green"
        />
        <MetricCard
          eyebrow="尚未定价"
          value={formatTokens(totals.unpricedTokens)}
          detail="不会显示为免费或零成本"
          icon="info"
          tone="cyan"
        />
      </div>
      <article className="content-card billing-card">
        <header>
          <div>
            <p className="eyebrow">逐来源明细</p>
            <h2>{formatRangeLabel(range, appliedCustomStartDate, appliedCustomEndDate)}费用</h2>
          </div>
          <ExportButtons range={range} rangeLabel={formatRangeLabel(range, appliedCustomStartDate, appliedCustomEndDate)} rows={rows} />
        </header>
        <ol>
          {rows.map((row) => (
            <li
              key={row.source.id}
              className={row.source.status === "ready" ? "" : "is-muted"}
            >
              <div>
                <strong>{row.source.label}</strong>
                <span>
                  {row.source.status === "ready"
                    ? `${formatTokens(row.usage.totalTokens)} Token`
                    : (row.source.message ?? "暂无数据")}
                </span>
              </div>
              <div>
                <b>{formatSourceCost(row)}</b>
                {row.cost.unpricedTokens > 0 && (
                  <small>未定价 {formatTokens(row.cost.unpricedTokens)}</small>
                )}
              </div>
            </li>
          ))}
        </ol>
        <footer>
          <span>USD {formatMoney("$", totals.usd)}</span>
          <span>CNY {formatMoney("¥", totals.cny)}</span>
          <span>未定价 {formatExactTokens(totals.unpricedTokens)}</span>
        </footer>
      </article>
      <div className="privacy-callout">
        <Icon name="shield" />
        <div>
          <strong>费用仍然保持本地优先</strong>
          <p>
            导出只包含来源标签、Token 汇总和估算费用，不包含对话、路径、会话 ID
            或凭据。
          </p>
        </div>
      </div>
    </section>
  );
}

function ExportButtons({
  range,
  rangeLabel,
  rows,
}: {
  range: UsageRange;
  rangeLabel: string;
  rows: ReturnType<typeof costDetailRows>;
}) {
  const exportData = (format: "csv" | "json") => {
    const safe = rows.map((row) => ({
      source: row.source.label,
      tokens: row.usage.totalTokens,
      usd: row.cost.usd,
      cny: row.cost.cny,
      unpricedTokens: row.cost.unpricedTokens,
    }));
    const text =
      format === "json"
        ? JSON.stringify(
            { range, rangeLabel, exportedAt: new Date().toISOString(), sources: safe },
            null,
            2,
          )
        : [
            `range,"${rangeLabel.replaceAll('"', '""')}"`,
            "source,tokens,usd,cny,unpricedTokens",
            ...safe.map((row) =>
              [row.source, row.tokens, row.usd, row.cny, row.unpricedTokens]
                .map((value) => `"${String(value).replaceAll('"', '""')}"`)
                .join(","),
            ),
          ].join("\r\n");
    const url = URL.createObjectURL(
      new Blob([text], {
        type: format === "json" ? "application/json" : "text/csv;charset=utf-8",
      }),
    );
    const link = document.createElement("a");
    link.href = url;
    link.download = `ai-token-usage-${range}-${localDateKey(new Date())}.${format}`;
    link.click();
    URL.revokeObjectURL(url);
  };
  return (
    <div className="export-actions">
      <button type="button" onClick={() => exportData("csv")}>
        <Icon name="download" />
        CSV
      </button>
      <button type="button" onClick={() => exportData("json")}>
        <Icon name="download" />
        JSON
      </button>
    </div>
  );
}

export function SettingsView({
  snapshot,
  preferences,
  indexMaintenance,
  onPreferencesChange,
  onSave,
  onTestNotification,
  onCheckIndexes,
  onRebuildIndexes,
  onThemeChange,
  onSelectWallpaper,
  onClearWallpaper,
  onUpdateRenewal,
  isSaving,
  isMaintainingIndexes,
  hasUnsavedChanges,
  message,
}: {
  snapshot: DashboardSnapshot;
  preferences: AppPreferences;
  indexMaintenance: IndexMaintenanceReport | null;
  onPreferencesChange: (value: AppPreferences) => void;
  onSave: () => void;
  onTestNotification: () => void;
  onCheckIndexes: () => void;
  onRebuildIndexes: () => void;
  onThemeChange: (value: ThemePreference) => void;
  onSelectWallpaper: () => void;
  onClearWallpaper: () => void;
  onUpdateRenewal: (value: string | null) => Promise<void>;
  isSaving: boolean;
  isMaintainingIndexes: boolean;
  hasUnsavedChanges: boolean;
  message: string | null;
}) {
  const update = <K extends keyof AppPreferences>(
    key: K,
    value: AppPreferences[K],
  ) => onPreferencesChange({ ...preferences, [key]: value });
  const diagnostics = createDiagnostics(snapshot, preferences, indexMaintenance);
  const [copied, setCopied] = useState(false);
  const [confirmingRebuild, setConfirmingRebuild] = useState(false);
  const indexDiagnostics = snapshot.indexDiagnostics;
  return (
    <section className="view-panel">
      <PageHeading
        eyebrow="PREFERENCES"
        title="设置"
        description="视觉、后台行为、阈值和隐私诊断集中在这里管理。"
      />
      {message && (
        <p className="settings-message" role="status">
          {message}
        </p>
      )}
      <div className="settings-grid">
        <SettingsCard
          title="外观"
          description="跟随系统模式会随 Windows 主题自动切换。"
          icon="sun"
        >
          <SettingRow label="主题">
            <div
              className="segmented-control"
              role="group"
              aria-label="主题偏好"
            >
              {(["system", "light", "dark"] as ThemePreference[]).map(
                (value) => (
                  <button
                    key={value}
                    type="button"
                    className={
                      preferences.themePreference === value ? "is-active" : ""
                    }
                    aria-pressed={preferences.themePreference === value}
                    onClick={() => onThemeChange(value)}
                  >
                    {value === "system"
                      ? "跟随系统"
                      : value === "light"
                        ? "浅色"
                        : "深色"}
                  </button>
                ),
              )}
            </div>
          </SettingRow>
          <SettingRow label="背景图片">
            <div className="inline-actions">
              <button
                type="button"
                onClick={onSelectWallpaper}
                disabled={isSaving}
              >
                <Icon name="image" />
                选择图片
              </button>
              {preferences.backgroundAssetPath && (
                <button
                  type="button"
                  onClick={onClearWallpaper}
                  disabled={isSaving}
                >
                  恢复默认
                </button>
              )}
            </div>
          </SettingRow>
        </SettingsCard>
        <SettingsCard
          title="后台与启动"
          description="关闭行为和刷新频率会立即应用到原生后台。"
          icon="monitor"
        >
          <SettingRow label="关闭窗口">
            <select
              value={preferences.closeBehavior}
              onChange={(event) =>
                update(
                  "closeBehavior",
                  event.target.value as AppPreferences["closeBehavior"],
                )
              }
            >
              <option value="hideToTray">隐藏到系统托盘</option>
              <option value="exit">直接退出应用</option>
            </select>
          </SettingRow>
          <SettingRow label="后台刷新">
            <select
              value={preferences.refreshIntervalMinutes}
              onChange={(event) =>
                update(
                  "refreshIntervalMinutes",
                  Number(event.target.value) as 1 | 5 | 15,
                )
              }
            >
              <option value={1}>每 1 分钟</option>
              <option value={5}>每 5 分钟</option>
              <option value={15}>每 15 分钟</option>
            </select>
          </SettingRow>
          <SettingRow label="开机启动">
            <Switch
              checked={preferences.autostartEnabled}
              onChange={(value) => update("autostartEnabled", value)}
            />
          </SettingRow>
        </SettingsCard>
        <SettingsCard
          title="通知与阈值"
          description="通知可总开关、测试并记录最近一次成功触发；安静时段内不打扰。"
          icon="info"
        >
          <SettingRow label="系统通知">
            <Switch
              checked={preferences.notificationsEnabled}
              onChange={(value) => update("notificationsEnabled", value)}
            />
          </SettingRow>
          <SettingRow label="额度剩余低于">
            <NumberInput
              value={preferences.quotaWarningPercent}
              max={99}
              onChange={(value) => update("quotaWarningPercent", value)}
            />
          </SettingRow>
          <SettingRow label="缓存命中低于">
            <NumberInput
              value={preferences.cacheWarningPercent}
              max={100}
              onChange={(value) => update("cacheWarningPercent", value)}
            />
          </SettingRow>
          <SettingRow label="安静时段">
            <Switch
              checked={preferences.quietHoursEnabled}
              onChange={(value) => update("quietHoursEnabled", value)}
            />
          </SettingRow>
          {preferences.quietHoursEnabled && (
            <SettingRow label="时段">
              <div className="time-range">
                <input
                  type="time"
                  value={preferences.quietHoursStart}
                  onChange={(event) =>
                    update("quietHoursStart", event.target.value)
                  }
                />
                <span>至</span>
                <input
                  type="time"
                  value={preferences.quietHoursEnd}
                  onChange={(event) =>
                    update("quietHoursEnd", event.target.value)
                  }
                />
              </div>
            </SettingRow>
          )}
          <div className="notification-status">
            <div>
              <strong>
                {preferences.lastNotificationAt
                  ? `最近通知：${formatDateTime(preferences.lastNotificationAt)}`
                  : "尚未发送过通知"}
              </strong>
              <span>
                {preferences.lastNotificationReason ??
                  "可发送一条本机测试通知确认系统链路。"}
              </span>
            </div>
            <button
              type="button"
              onClick={onTestNotification}
              disabled={isSaving || !preferences.notificationsEnabled}
            >
              发送测试通知
            </button>
          </div>
        </SettingsCard>
        <SettingsCard
          title="套餐续费时间"
          description="官方接口未提供时，可手动记录一个仅本机可见的日期。"
          icon="clock"
        >
          <RenewalEditor
            value={snapshot.planRenewalAt}
            source={snapshot.planRenewalSource}
            onChange={onUpdateRenewal}
          />
        </SettingsCard>
        <SettingsCard
          title="索引可靠性"
          description="自检只读；重建只处理应用派生数据库，并在清理前创建可恢复备份。"
          icon="database"
          wide
        >
          {indexDiagnostics ? (
            <div className="diagnostic-metrics">
              <div>
                <span>总刷新</span>
                <strong>
                  {formatDuration(indexDiagnostics.totalDurationMs)}
                </strong>
              </div>
              <div>
                <span>Codex 扫描</span>
                <strong>
                  {formatDuration(indexDiagnostics.codexDurationMs)}
                </strong>
              </div>
              <div>
                <span>多来源扫描</span>
                <strong>
                  {formatDuration(indexDiagnostics.sourcesDurationMs)}
                </strong>
              </div>
              <div>
                <span>账号读取</span>
                <strong>
                  {formatDuration(indexDiagnostics.accountDurationMs)}
                </strong>
              </div>
              {indexDiagnostics.databases.map((database) => (
                <div key={database.label}>
                  <span>{database.label}</span>
                  <strong>{formatBytes(database.sizeBytes)}</strong>
                </div>
              ))}
            </div>
          ) : (
            <EmptyState
              title="尚无索引性能记录"
              description="完成一次刷新后显示各阶段耗时和数据库占用。"
            />
          )}
          {indexMaintenance ? (
            <div className="maintenance-report">
              <header>
                <div>
                  <strong>
                    {indexHealthLabel(indexMaintenance.overallStatus)}
                  </strong>
                  <span>检查于 {formatDateTime(indexMaintenance.checkedAt)}</span>
                </div>
                <span
                  className={`health-status health-status--${indexMaintenance.overallStatus}`}
                >
                  {indexMaintenance.overallStatus === "healthy"
                    ? "正常"
                    : indexMaintenance.overallStatus === "warning"
                      ? "待建立"
                      : "需维护"}
                </span>
              </header>
              <p>{indexMaintenance.message}</p>
              <div className="maintenance-databases">
                {indexMaintenance.databases.map((database) => (
                  <div key={database.label}>
                    <span>
                      <b
                        className={`source-dot source-dot--${
                          database.status === "healthy"
                            ? "ready"
                            : database.status === "missing"
                              ? "notDetected"
                              : "error"
                        }`}
                      />
                      {database.label}
                    </span>
                    <strong>
                      {database.status === "healthy"
                        ? "完整性通过"
                        : database.status === "missing"
                          ? "尚未建立"
                          : database.integrity === "failed"
                            ? "完整性失败"
                            : "版本或读取异常"}
                    </strong>
                    <small>
                      {formatBytes(database.sizeBytes)} · 版本{" "}
                      {database.schemaVersion ?? "—"}/
                      {database.expectedSchemaVersion}
                    </small>
                  </div>
                ))}
              </div>
              <p className="maintenance-backup">
                最近备份：
                {indexMaintenance.lastBackupAt
                  ? formatDateTime(indexMaintenance.lastBackupAt)
                  : "尚无维护备份"}
              </p>
            </div>
          ) : (
            <EmptyState
              title="尚未执行索引自检"
              description="自检不会修改数据库或重新扫描来源日志。"
            />
          )}
          <div className="inline-actions maintenance-actions">
            <button
              type="button"
              onClick={onCheckIndexes}
              disabled={isMaintainingIndexes}
              aria-busy={isMaintainingIndexes}
            >
              {isMaintainingIndexes && <span className="spin"><Icon name="refresh" /></span>}
              {isMaintainingIndexes ? "正在自检…" : "立即自检"}
            </button>
            <button
              type="button"
              className="danger-button"
              onClick={() => setConfirmingRebuild(true)}
              disabled={isMaintainingIndexes}
            >
              备份后重建
            </button>
          </div>
          {confirmingRebuild && (
            <div
              className="maintenance-confirm"
              role="alertdialog"
              aria-modal="true"
              aria-labelledby="maintenance-confirm-title"
            >
              <strong id="maintenance-confirm-title">
                确认重建两套派生索引？
              </strong>
              <p>
                应用会先备份 usage-index-v1/v2 及其 WAL/SHM，再删除旧派生索引并全量重扫。本机原始会话日志、设置库和费用价格表不会被修改。
              </p>
              <div className="inline-actions">
                <button
                  type="button"
                  onClick={() => setConfirmingRebuild(false)}
                >
                  取消
                </button>
                <button
                  type="button"
                  className="danger-button"
                  onClick={() => {
                    setConfirmingRebuild(false);
                    onRebuildIndexes();
                  }}
                >
                  确认备份并重建
                </button>
              </div>
            </div>
          )}
        </SettingsCard>
        <SettingsCard
          title="隐私与诊断"
          description="诊断内容经过脱敏，可复制用于排查来源状态。"
          icon="shield"
          wide
        >
          <pre className="diagnostics-preview">{diagnostics}</pre>
          <div className="inline-actions">
            <button
              type="button"
              onClick={() => {
                void navigator.clipboard.writeText(diagnostics).then(() => {
                  setCopied(true);
                  window.setTimeout(() => setCopied(false), 1600);
                });
              }}
            >
              <Icon name="database" />
              {copied ? "已复制" : "复制脱敏诊断"}
            </button>
          </div>
        </SettingsCard>
      </div>
      <div className="settings-save">
        <span>{hasUnsavedChanges ? "有未保存的运行设置；主题与壁纸仍会即时保存。" : "运行设置已保存；主题与壁纸会即时保存。"}</span>
        <button
          className="primary-button"
          type="button"
          onClick={onSave}
          disabled={isSaving || !hasUnsavedChanges}
        >
          {isSaving ? "保存中…" : "保存运行设置"}
        </button>
      </div>
    </section>
  );
}

function SettingsCard({
  title,
  description,
  icon,
  wide = false,
  children,
}: {
  title: string;
  description: string;
  icon: IconName;
  wide?: boolean;
  children: ReactNode;
}) {
  return (
    <article
      className={`content-card settings-card${wide ? " settings-card--wide" : ""}`}
    >
      <header>
        <span className="settings-icon">
          <Icon name={icon} />
        </span>
        <div>
          <h2>{title}</h2>
          <p>{description}</p>
        </div>
      </header>
      <div className="settings-card__body">{children}</div>
    </article>
  );
}
function SettingRow({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <div className="setting-row">
      <span>{label}</span>
      <div>{children}</div>
    </div>
  );
}
function Switch({
  checked,
  onChange,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <label className="switch">
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span />
      {checked ? "已开启" : "已关闭"}
    </label>
  );
}
function NumberInput({
  value,
  max,
  onChange,
}: {
  value: number;
  max: number;
  onChange: (value: number) => void;
}) {
  return (
    <label className="number-input">
      <input
        type="number"
        min={1}
        max={max}
        value={value}
        onChange={(event) => onChange(Number(event.target.value))}
      />
      <span>%</span>
    </label>
  );
}

function RenewalEditor({
  value,
  source,
  onChange,
}: {
  value: string | number | null;
  source: DashboardSnapshot["planRenewalSource"];
  onChange: (value: string | null) => Promise<void>;
}) {
  const [input, setInput] = useState(() => toDateTimeLocal(value));
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  useEffect(() => setInput(toDateTimeLocal(value)), [value]);
  return (
    <div className="renewal-editor">
      <div>
        <strong>{value ? formatDateTime(value) : "未提供"}</strong>
        <small>
          {source === "manual"
            ? "手动设置 · 仅本机"
            : "官方接口未返回套餐续费日期"}
        </small>
      </div>
      <label>
        <span className="sr-only">本地续费时间</span>
        <input
          type="datetime-local"
          value={input}
          min={toDateTimeLocal(Date.now())}
          onChange={(event) => setInput(event.target.value)}
        />
      </label>
      <div className="inline-actions">
        <button
          type="button"
          disabled={saving || !input}
          onClick={() => {
            setSaving(true);
            setMessage(null);
            void onChange(new Date(input).toISOString())
              .then(() => setMessage("续费时间已保存。"))
              .catch(() => setMessage("保存失败。"))
              .finally(() => setSaving(false));
          }}
        >
          保存日期
        </button>
        {value && source === "manual" && (
          <button
            type="button"
            disabled={saving}
            onClick={() => {
              setSaving(true);
              void onChange(null)
                .then(() => setMessage("已清除本机日期。"))
                .finally(() => setSaving(false));
            }}
          >
            清除
          </button>
        )}
      </div>
      {message && <p role="status">{message}</p>}
    </div>
  );
}

export function UsageDayDrawer({
  date,
  accountTokens,
  detail,
  loading,
  error,
  cacheWarningPercent,
  projectMergeRules,
  sources,
  onClose,
}: {
  date: string;
  accountTokens: number | null;
  detail: UsageDayDetail | null;
  loading: boolean;
  error: string | null;
  cacheWarningPercent: number;
  projectMergeRules: ProjectMergeRule[];
  sources: UsageSourceSummary[];
  onClose: () => void;
}) {
  const drawerRef = useRef<HTMLElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const [detailMode, setDetailMode] = useState<"tasks" | "projects">("tasks");
  const projects = summarizeDayProjects(detail, projectMergeRules, sources);
  useEffect(() => {
    const previousFocus = document.activeElement as HTMLElement | null;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    closeRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "Tab" || !drawerRef.current) return;
      const items = [
        ...drawerRef.current.querySelectorAll<HTMLElement>(
          'button:not([disabled]),input:not([disabled]),select:not([disabled]),[tabindex]:not([tabindex="-1"])',
        ),
      ];
      if (!items.length) return;
      if (event.shiftKey && document.activeElement === items[0]) {
        event.preventDefault();
        items.at(-1)?.focus();
      } else if (!event.shiftKey && document.activeElement === items.at(-1)) {
        event.preventDefault();
        items[0].focus();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.body.style.overflow = previousOverflow;
      previousFocus?.focus();
    };
  }, [onClose]);
  return (
    <div
      className="usage-drawer-backdrop"
      role="presentation"
      onMouseDown={onClose}
    >
      <aside
        ref={drawerRef}
        className="usage-drawer"
        role="dialog"
        aria-modal="true"
        aria-labelledby="usage-drawer-title"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header>
          <div>
            <p className="eyebrow">日用量明细</p>
            <h2 id="usage-drawer-title">{date}</h2>
          </div>
          <button
            ref={closeRef}
            type="button"
            className="drawer-close"
            onClick={onClose}
            aria-label="关闭日用量明细"
          >
            <Icon name="x" />
          </button>
        </header>
        <section className="drawer-account">
          <span>账号 Token（服务端精确日总量）</span>
          <strong>{formatTokens(accountTokens)}</strong>
          <p>可能包含其他设备与云端任务；下方只列出本机可归因事件。</p>
        </section>
        <section className="drawer-tasks">
          <header>
            <div>
              <p className="eyebrow">仅此设备可归因</p>
              <h3>{detailMode === "tasks" ? "模型、项目与任务明细" : "项目用量汇总"}</h3>
            </div>
            {detail && (
              <span className="soft-pill">低缓存 {detail.lowCacheTasks}</span>
            )}
          </header>
          <div className="drawer-view-toggle" role="group" aria-label="日用量明细展示方式">
            <button type="button" className={detailMode === "tasks" ? "is-active" : ""} aria-pressed={detailMode === "tasks"} onClick={() => setDetailMode("tasks")}>任务</button>
            <button type="button" className={detailMode === "projects" ? "is-active" : ""} aria-pressed={detailMode === "projects"} onClick={() => setDetailMode("projects")}>项目</button>
          </div>
          {loading && <p className="drawer-state">正在读取本机脱敏任务明细…</p>}
          {error && (
            <p className="drawer-error" role="alert">
              {error}
            </p>
          )}
          {detail?.tasks.length === 0 && (
            <p className="drawer-state">该日没有可归因的本机任务。</p>
          )}
          {detailMode === "tasks" && detail && detail.tasks.length > 0 && (
            <ol>
              {detail.tasks.map((task, index) => {
                const lowCache =
                  task.cacheMetricsAvailable &&
                  task.cacheHitRate !== null &&
                  task.cacheHitRate * 100 < cacheWarningPercent;
                return (
                  <li
                    key={`${task.taskLabel}-${index}`}
                    className={lowCache ? "is-low-cache" : ""}
                  >
                    <header>
                      <div>
                        <strong>{task.taskLabel}</strong>
                        <span>
                          {task.occurredAtMs === null
                            ? "时间不可用"
                            : formatDateTime(task.occurredAtMs)}{" "}
                          · {task.source}
                        </span>
                      </div>
                      <b>{formatTokens(task.usage.totalTokens)}</b>
                    </header>
                    <p>
                      {task.modelLabel} · {task.projectLabel}
                    </p>
                    <footer>
                      <span className={lowCache ? "is-danger" : ""}>
                        {task.cacheMetricsAvailable &&
                        task.cacheHitRate !== null
                          ? `缓存命中率 ${formatPercent(task.cacheHitRate * 100)}${lowCache ? `（低于 ${cacheWarningPercent}%）` : ""}`
                          : "缓存数据不可用"}
                      </span>
                      <span>{formatCostCompact(task.cost)}</span>
                    </footer>
                  </li>
                );
              })}
            </ol>
          )}
          {detailMode === "projects" && projects.length > 0 && (
            <ol className="drawer-projects">
              {projects.map((project) => (
                <li key={project.label}>
                  <header>
                    <div>
                      <strong>{project.label}</strong>
                      <span>{project.taskCount} 条本机记录 · {[...project.sources].join("、")}{project.mergedMemberCount ? ` · 已合并 ${project.mergedMemberCount} 项` : ""}</span>
                    </div>
                    <b>{formatTokens(project.usage.totalTokens)} Token</b>
                  </header>
                  <p>输入 {formatTokens(project.usage.inputTokens + project.usage.cachedInputTokens + project.usage.cacheWriteTokens)} · 输出 {formatTokens(project.usage.outputTokens)}</p>
                </li>
              ))}
            </ol>
          )}
        </section>
      </aside>
    </div>
  );
}

function summarizeDayProjects(detail: UsageDayDetail | null, rules: ProjectMergeRule[], sourceSummaries: UsageSourceSummary[]) {
  const normalizeSource = (value: string) => value.toLocaleLowerCase().replace(/[^a-z0-9]/g, "");
  const projects = new Map<string, { label: string; taskCount: number; sources: Set<string>; usage: DeviceTokenUsage; mergedMemberCount?: number }>();
  for (const task of detail?.tasks ?? []) {
    const label = task.projectLabel.trim() || "未知项目";
    const source = sourceSummaries.find((item) => normalizeSource(item.id) === normalizeSource(task.source) || normalizeSource(item.label) === normalizeSource(task.source));
    const rule = source ? rules.find((candidate) => candidate.members.some((member) => member.sourceId === source.id && (member.projectKey === label || member.projectLabel === label))) : undefined;
    const key = rule ? `merge:${rule.id}` : `${source?.id ?? task.source}:${label}`;
    const current = projects.get(key) ?? {
      label: rule?.displayName ?? label,
      taskCount: 0,
      sources: new Set<string>(),
      mergedMemberCount: rule?.members.length,
      usage: {
        inputTokens: 0,
        cachedInputTokens: 0,
        cacheWriteTokens: 0,
        cacheWrite5mTokens: 0,
        cacheWrite1hTokens: 0,
        cacheWriteUnknownTokens: 0,
        outputTokens: 0,
        reasoningOutputTokens: 0,
        totalTokens: 0,
      },
    };
    current.taskCount += 1;
    current.sources.add(task.source);
    for (const key of Object.keys(current.usage) as Array<keyof DeviceTokenUsage>) {
      current.usage[key] += task.usage[key];
    }
    projects.set(key, current);
  }
  return [...projects.values()].sort((left, right) => right.usage.totalTokens - left.usage.totalTokens);
}

function EmptyState({
  title,
  description,
}: {
  title: string;
  description: string;
}) {
  return (
    <div className="empty-state">
      <span className="empty-state__icon" aria-hidden="true">
        <Icon name="activity" />
      </span>
      <div className="empty-state__content">
        <strong>{title}</strong>
        <p>{description}</p>
      </div>
    </div>
  );
}
function createTokenCategories(usage: DeviceTokenUsage) {
  return [
    { label: "新输入", value: usage.inputTokens },
    { label: "缓存输入", value: usage.cachedInputTokens },
    { label: "缓存写入", value: usage.cacheWriteTokens },
    {
      label: "可见输出",
      value: Math.max(0, usage.outputTokens - usage.reasoningOutputTokens),
    },
    { label: "推理输出", value: usage.reasoningOutputTokens },
  ];
}
function createDonutGradient(
  categories: ReturnType<typeof createTokenCategories>,
): string {
  const total = categories.reduce((sum, item) => sum + item.value, 0);
  if (total <= 0) return "conic-gradient(var(--surface-soft) 0 100%)";
  let cursor = 0;
  return `conic-gradient(${categories
    .map((item, index) => {
      const start = cursor;
      cursor += (item.value / total) * 100;
      return `${TOKEN_COLORS[index]} ${start}% ${cursor}%`;
    })
    .join(",")})`;
}
function formatMoney(symbol: "$" | "¥", value: number): string {
  return `${symbol}${new Intl.NumberFormat("zh-CN", { minimumFractionDigits: 2, maximumFractionDigits: 4 }).format(value)}`;
}
function formatCostCompact(
  cost: { usd: number; cny: number; unpricedTokens: number } | null | undefined,
): string {
  if (!cost) return "—";
  const parts: string[] = [];
  if (cost.usd > 0) parts.push(formatMoney("$", cost.usd));
  if (cost.cny > 0) parts.push(formatMoney("¥", cost.cny));
  return parts.length
    ? parts.join(" · ")
    : cost.unpricedTokens > 0
      ? "部分未定价"
      : "$0.00";
}
function formatSourceCost(row: CostDetailRow): string {
  if (row.source.status === "ready") return formatCostCompact(row.cost);
  if (row.source.status === "notDetected") {
    return "暂无数据";
  }
  return "不可用";
}
function formatDuration(milliseconds: number): string {
  return milliseconds >= 1000
    ? `${(milliseconds / 1000).toFixed(milliseconds >= 10_000 ? 1 : 2)} 秒`
    : `${Math.round(milliseconds)} 毫秒`;
}
function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(value >= 10 ? 1 : 2)} ${units[index]}`;
}
function indexHealthLabel(status: IndexMaintenanceReport["overallStatus"]): string {
  return status === "healthy"
    ? "索引状态正常"
    : status === "warning"
      ? "索引尚未完整建立"
      : "索引需要维护";
}
function sourceStatusLabel(status: UsageSourceSummary["status"]): string {
  return status === "ready"
    ? "已就绪"
    : status === "notDetected"
      ? "未检测到"
      : status === "unavailable"
        ? "暂不支持"
        : "读取异常";
}
function statusLabel(status: DashboardSnapshot["status"]): string {
  return status === "ready"
    ? "数据已就绪"
    : status === "loading"
      ? "正在同步"
      : status === "offline"
        ? "离线"
        : status === "unauthenticated"
          ? "未登录"
          : status === "unsupported"
            ? "版本不支持"
            : "读取失败";
}
function localDateKey(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}
function toDateTimeLocal(value: string | number | null): string {
  if (value === null) return "";
  const milliseconds =
    typeof value === "number"
      ? toEpochMilliseconds(value)
      : new Date(value).getTime();
  if (milliseconds === null || !Number.isFinite(milliseconds)) return "";
  const local = new Date(
    milliseconds - new Date(milliseconds).getTimezoneOffset() * 60_000,
  );
  return local.toISOString().slice(0, 16);
}
function createDiagnostics(
  snapshot: DashboardSnapshot,
  preferences: AppPreferences,
  indexMaintenance: IndexMaintenanceReport | null,
): string {
  const lines = [
    "AI Token 用量监控诊断",
    `状态: ${snapshot.status}`,
    `Codex 版本: ${snapshot.codexVersion ?? "未知"}`,
    `账号最近读取: ${snapshot.accountDiagnostics?.readAt ?? snapshot.fetchedAt ?? "无"}`,
    `账号读取耗时: ${snapshot.accountDiagnostics ? `${snapshot.accountDiagnostics.durationMs}ms` : "无"}`,
    `账号读取 RPC: ${snapshot.accountDiagnostics?.methods.join(", ") || "无"}`,
    `本机最近索引: ${snapshot.deviceUsage?.generatedAt ?? "无"}`,
    `刷新间隔: ${preferences.refreshIntervalMinutes} 分钟`,
    `关闭行为: ${preferences.closeBehavior}`,
    `系统通知: ${preferences.notificationsEnabled ? "enabled" : "disabled"}`,
    `最近通知: ${preferences.lastNotificationAt ?? "none"}`,
    `最近通知原因: ${preferences.lastNotificationReason ?? "none"}`,
  ];
  const index = snapshot.indexDiagnostics;
  if (index) {
    lines.push(
      `索引耗时: total=${index.totalDurationMs}ms, codex=${index.codexDurationMs}ms, sources=${index.sourcesDurationMs}ms, account=${index.accountDurationMs}ms`,
    );
    for (const database of index.databases)
      lines.push(
        `- database ${database.label}: size=${database.sizeBytes} bytes`,
      );
  }
  if (indexMaintenance) {
    lines.push(
      `索引自检: status=${indexMaintenance.overallStatus}, checked=${indexMaintenance.checkedAt}, lastBackup=${indexMaintenance.lastBackupAt ?? "none"}`,
    );
    for (const database of indexMaintenance.databases) {
      lines.push(
        `- index ${database.label}: status=${database.status}, integrity=${database.integrity}, schema=${database.schemaVersion ?? "none"}/${database.expectedSchemaVersion}, size=${database.sizeBytes} bytes`,
      );
    }
  }
  lines.push("来源:");
  const health = snapshot.sourceHealth ?? snapshot.deviceUsage?.sources ?? [];
  for (const source of health)
    lines.push(
      `- ${source.label}: ${source.status}, files=${source.sourceFiles}, events=${source.indexedEvents}, new=${source.newEvents ?? "unknown"}, skipped=${source.skippedRecords ?? "unknown"}, last=${source.lastIndexedAt ?? "none"}`,
    );
  lines.push("隐私: 未包含对话正文、完整路径、会话 ID、凭据或原始响应");
  return lines.join("\n");
}
