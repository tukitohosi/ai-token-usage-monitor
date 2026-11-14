import type {
  DeviceUsageDimension,
  DeviceUsageRangeSummary,
  DeviceTokenUsage,
  ProjectMergeMember,
  ProjectMergeRule,
  TokenCost,
  UsageDayDetail,
  UsageRange,
  UsageSourceSummary,
} from "../core/types.js";

export type UsagePeriod = UsageRange | "last24Hours";
export type SourceSelection = UsageSourceSummary["id"][];
export type SourceSelectionMode = "single" | "multiple";

export interface CostDetailRow {
  source: UsageSourceSummary;
  usage: DeviceTokenUsage;
  cost: TokenCost;
}

export interface UsageInsightSummary {
  observedDays: number;
  confidence: "low" | "medium" | "high";
  recentAverageTokens: number;
  baselineAverageTokens: number;
  changePercent: number | null;
  trend: "up" | "down" | "stable" | "insufficient";
  anomaly: "spike" | "drop" | "normal" | "insufficient";
  peakDate: string | null;
  peakTokens: number;
}

export const SOURCE_TREND_COLORS: Record<UsageSourceSummary["id"], string> = {
  codex: "#7c6cff",
  "claude-code": "#3abfe9",
  opencode: "#35c994",
  workbuddy: "#f2a851",
  "workbuddy-ai": "#dd6ea8",
  cursor: "#8b95aa",
};

export interface SourceTrendSegment {
  sourceId: UsageSourceSummary["id"];
  label: string;
  color: string;
  tokens: number;
  share: number;
}

export interface SourceTrendBucket {
  date: string;
  tokens: number;
  segments: SourceTrendSegment[];
}

export interface SourceStackedDimension extends DeviceUsageDimension {
  segments: SourceTrendSegment[];
  mergedMemberCount?: number;
  memberLabels?: string[];
}

export interface HourlySourceTrendBucket {
  hourStartMs: number;
  tokens: number;
  segments: SourceTrendSegment[];
}

function localDateKey(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

export function buildFifteenDaySourceTrend(
  sources: UsageSourceSummary[],
  nowMs: number,
): { buckets: SourceTrendBucket[]; legend: Array<{ sourceId: UsageSourceSummary["id"]; label: string; color: string }> } {
  const dates = Array.from({ length: 15 }, (_, index) => {
    const date = new Date(nowMs);
    date.setHours(12, 0, 0, 0);
    date.setDate(date.getDate() - (14 - index));
    return localDateKey(date);
  });
  const dateSet = new Set(dates);
  const indexed = new Map<UsageSourceSummary["id"], Map<string, number>>();
  const totalsBySource = new Map<UsageSourceSummary["id"], number>();
  for (const source of sources) {
    const daily = new Map<string, number>();
    for (const bucket of sourceRange(source, "last30Days").dailyUsage) {
      if (!dateSet.has(bucket.date)) continue;
      const tokens = Number.isFinite(bucket.usage.totalTokens) ? Math.max(0, bucket.usage.totalTokens) : 0;
      daily.set(bucket.date, (daily.get(bucket.date) ?? 0) + tokens);
      totalsBySource.set(source.id, (totalsBySource.get(source.id) ?? 0) + tokens);
    }
    indexed.set(source.id, daily);
  }
  const buckets = dates.map((date) => {
    const values = sources.map((source) => ({ source, tokens: indexed.get(source.id)?.get(date) ?? 0 }));
    const tokens = values.reduce((sum, value) => sum + value.tokens, 0);
    return {
      date,
      tokens,
      segments: values.map(({ source, tokens: sourceTokens }) => ({
        sourceId: source.id,
        label: source.label,
        color: SOURCE_TREND_COLORS[source.id],
        tokens: sourceTokens,
        share: tokens > 0 ? sourceTokens / tokens * 100 : 0,
      })),
    };
  });
  const legend = sources
    .filter((source) => (totalsBySource.get(source.id) ?? 0) > 0)
    .map((source) => ({ sourceId: source.id, label: source.label, color: SOURCE_TREND_COLORS[source.id] }));
  return { buckets, legend };
}

export function buildRollingHourlyBuckets(summary: DeviceUsageRangeSummary, nowMs: number) {
  const hourMs = 60 * 60_000;
  const current = new Date(nowMs);
  current.setMinutes(0, 0, 0);
  const currentHour = current.getTime();
  const indexed = new Map((summary.hourlyUsage ?? []).map((bucket) => [bucket.hourStartMs, bucket.usage.totalTokens]));
  return Array.from({ length: 24 }, (_, index) => {
    const hourStartMs = currentHour - (23 - index) * hourMs;
    return { hourStartMs, tokens: indexed.get(hourStartMs) ?? 0 };
  });
}

export function buildRollingHourlySourceTrend(
  sources: UsageSourceSummary[],
  selected: SourceSelection,
  nowMs: number,
): HourlySourceTrendBucket[] {
  const hourMs = 60 * 60_000;
  const current = new Date(nowMs);
  current.setMinutes(0, 0, 0);
  const currentHour = current.getTime();
  const active = selected.length === 0
    ? sources
    : sources.filter((source) => selected.includes(source.id));
  const indexed = new Map<UsageSourceSummary["id"], Map<number, number>>();
  for (const source of active) {
    indexed.set(
      source.id,
      new Map(
        (sourceRange(source, "last24Hours").hourlyUsage ?? []).map((bucket) => [
          bucket.hourStartMs,
          Math.max(0, bucket.usage.totalTokens),
        ]),
      ),
    );
  }
  return Array.from({ length: 24 }, (_, index) => {
    const hourStartMs = currentHour - (23 - index) * hourMs;
    const values = active.map((source) => ({
      source,
      tokens: indexed.get(source.id)?.get(hourStartMs) ?? 0,
    }));
    const tokens = values.reduce((sum, value) => sum + value.tokens, 0);
    return {
      hourStartMs,
      tokens,
      segments: values.map(({ source, tokens: sourceTokens }) => ({
        sourceId: source.id,
        label: source.label,
        color: SOURCE_TREND_COLORS[source.id] ?? "#8b95aa",
        tokens: sourceTokens,
        share: tokens > 0 ? sourceTokens / tokens * 100 : 0,
      })),
    };
  });
}

export function buildDailySourceTrend(
  sources: UsageSourceSummary[],
  selected: SourceSelection,
  period: UsagePeriod,
): SourceTrendBucket[] {
  const active = selected.length === 0
    ? sources
    : sources.filter((source) => selected.includes(source.id));
  const dates = new Set<string>();
  const indexed = new Map<UsageSourceSummary["id"], Map<string, number>>();
  for (const source of active) {
    const daily = new Map<string, number>();
    for (const bucket of sourceRange(source, period).dailyUsage) {
      const tokens = Math.max(0, bucket.usage.totalTokens);
      daily.set(bucket.date, (daily.get(bucket.date) ?? 0) + tokens);
      dates.add(bucket.date);
    }
    indexed.set(source.id, daily);
  }
  return [...dates].sort().map((date) => {
    const values = active.map((source) => ({ source, tokens: indexed.get(source.id)?.get(date) ?? 0 }));
    const tokens = values.reduce((sum, value) => sum + value.tokens, 0);
    return {
      date,
      tokens,
      segments: values.map(({ source, tokens: sourceTokens }) => ({
        sourceId: source.id,
        label: source.label,
        color: SOURCE_TREND_COLORS[source.id] ?? "#8b95aa",
        tokens: sourceTokens,
        share: tokens > 0 ? sourceTokens / tokens * 100 : 0,
      })),
    };
  });
}

export function projectMemberIdentity(member: Pick<ProjectMergeMember, "sourceId" | "projectKey">): string {
  return `${member.sourceId}\u0000${member.projectKey}`;
}

function projectMemberLabelIdentity(member: Pick<ProjectMergeMember, "sourceId" | "projectLabel">): string {
  return `${member.sourceId}\u0001${member.projectLabel}`;
}

export function buildProjectCatalog(sources: UsageSourceSummary[]): ProjectMergeMember[] {
  return sources.flatMap((source) => sourceRange(source, "total").byProject.map((project) => ({
    sourceId: source.id,
    sourceLabel: source.label,
    projectKey: project.key,
    projectLabel: project.label,
  }))).sort((left, right) => left.projectLabel.localeCompare(right.projectLabel, "zh-CN"));
}

function sourceSegment(source: UsageSourceSummary, tokens: number, total = tokens): SourceTrendSegment {
  return {
    sourceId: source.id,
    label: source.label,
    color: SOURCE_TREND_COLORS[source.id] ?? "#8b95aa",
    tokens,
    share: total > 0 ? tokens / total * 100 : 0,
  };
}

export function buildSourceDimensionRows(
  sources: UsageSourceSummary[],
  selected: SourceSelection,
  period: UsagePeriod,
  kind: "byModel" | "byProject",
): SourceStackedDimension[] {
  const active = selected.length === 0
    ? sources
    : sources.filter((source) => selected.includes(source.id));
  return active.flatMap((source) => sourceRange(source, period)[kind].map((dimension) => ({
    ...dimension,
    key: `${source.id}:${dimension.key}`,
    label: active.length === 1 ? dimension.label : `${source.label} · ${dimension.label}`,
    segments: [sourceSegment(source, dimension.usage.totalTokens)],
  }))).sort((left, right) => right.usage.totalTokens - left.usage.totalTokens);
}

export function buildProjectDimensionRows(
  sources: UsageSourceSummary[],
  selected: SourceSelection,
  period: UsagePeriod,
  rules: ProjectMergeRule[],
): SourceStackedDimension[] {
  const active = selected.length === 0
    ? sources
    : sources.filter((source) => selected.includes(source.id));
  const membership = new Map<string, ProjectMergeRule>();
  for (const rule of rules) {
    for (const member of rule.members) {
      membership.set(projectMemberIdentity(member), rule);
      membership.set(projectMemberLabelIdentity(member), rule);
    }
  }
  const grouped = new Map<string, {
    rule: ProjectMergeRule;
    usage: DeviceTokenUsage;
    cost: TokenCost;
    bySource: Map<UsageSourceSummary["id"], number>;
  }>();
  const ungrouped: SourceStackedDimension[] = [];
  for (const source of active) {
    for (const dimension of sourceRange(source, period).byProject) {
      const rule = membership.get(projectMemberIdentity({ sourceId: source.id, projectKey: dimension.key }))
        ?? membership.get(projectMemberLabelIdentity({ sourceId: source.id, projectLabel: dimension.label }));
      if (!rule) {
        ungrouped.push({
          ...dimension,
          key: `${source.id}:${dimension.key}`,
          label: active.length === 1 ? dimension.label : `${source.label} · ${dimension.label}`,
          segments: [sourceSegment(source, dimension.usage.totalTokens)],
        });
        continue;
      }
      const current = grouped.get(rule.id) ?? {
        rule,
        usage: { ...EMPTY_USAGE },
        cost: { usd: 0, cny: 0, unpricedTokens: 0 },
        bySource: new Map<UsageSourceSummary["id"], number>(),
      };
      current.usage = addUsage(current.usage, dimension.usage);
      current.cost = addCost(current.cost, dimension.cost);
      current.bySource.set(source.id, (current.bySource.get(source.id) ?? 0) + dimension.usage.totalTokens);
      grouped.set(rule.id, current);
    }
  }
  const merged = [...grouped.values()].map((value): SourceStackedDimension => ({
    key: `merge:${value.rule.id}`,
    label: value.rule.displayName,
    usage: value.usage,
    cost: value.cost,
    mergedMemberCount: value.rule.members.length,
    memberLabels: value.rule.members.map((member) => `${member.sourceLabel} · ${member.projectLabel}`),
    segments: active
      .map((source) => sourceSegment(source, value.bySource.get(source.id) ?? 0, value.usage.totalTokens))
      .filter((segment) => segment.tokens > 0),
  }));
  return [...ungrouped, ...merged].sort((left, right) => right.usage.totalTokens - left.usage.totalTokens);
}

export function toggleSourceSelection(
  current: SourceSelection,
  next: UsageSourceSummary["id"],
  mode: SourceSelectionMode = "multiple",
): SourceSelection {
  if (mode === "single") {
    return current.length === 1 && current[0] === next ? [] : [next];
  }
  return current.includes(next)
    ? current.filter((source) => source !== next)
    : [...current, next];
}

const EMPTY_USAGE: DeviceTokenUsage = {
  inputTokens: 0,
  cachedInputTokens: 0,
  cacheWriteTokens: 0,
  cacheWrite5mTokens: 0,
  cacheWrite1hTokens: 0,
  cacheWriteUnknownTokens: 0,
  outputTokens: 0,
  reasoningOutputTokens: 0,
  totalTokens: 0,
};

function legacyRange(source: UsageSourceSummary, period: UsagePeriod): DeviceUsageRangeSummary {
  const useToday = period === "today" || period === "last24Hours";
  return {
    usage: useToday ? source.today : source.total,
    cost: useToday ? source.todayCost : source.cost,
    byModel: useToday ? source.todayByModel : source.byModel,
    byProject: source.byProject,
    dailyUsage: [],
  };
}

export function sourceRange(source: UsageSourceSummary, period: UsagePeriod): DeviceUsageRangeSummary {
  const value = source.ranges?.[period as keyof typeof source.ranges];
  if (value) return value;
  if (period === "custom") {
    return {
      usage: { ...EMPTY_USAGE },
      cost: { usd: 0, cny: 0, unpricedTokens: 0 },
      byModel: [],
      byProject: [],
      dailyUsage: [],
    };
  }
  return legacyRange(source, period);
}

function addUsage(total: DeviceTokenUsage, usage: DeviceTokenUsage): DeviceTokenUsage {
  return {
    inputTokens: total.inputTokens + usage.inputTokens,
    cachedInputTokens: total.cachedInputTokens + usage.cachedInputTokens,
    cacheWriteTokens: total.cacheWriteTokens + usage.cacheWriteTokens,
    cacheWrite5mTokens: total.cacheWrite5mTokens + usage.cacheWrite5mTokens,
    cacheWrite1hTokens: total.cacheWrite1hTokens + usage.cacheWrite1hTokens,
    cacheWriteUnknownTokens: total.cacheWriteUnknownTokens + usage.cacheWriteUnknownTokens,
    outputTokens: total.outputTokens + usage.outputTokens,
    reasoningOutputTokens: total.reasoningOutputTokens + usage.reasoningOutputTokens,
    totalTokens: total.totalTokens + usage.totalTokens,
  };
}

function mergeDimensions(
  sources: UsageSourceSummary[],
  period: UsagePeriod,
  key: "byModel" | "byProject",
): DeviceUsageDimension[] {
  return sources.flatMap((source) => sourceRange(source, period)[key].map((dimension) => ({
    ...dimension,
    key: `${source.id}:${dimension.key}`,
    label: sources.length === 1 ? dimension.label : `${source.label} · ${dimension.label}`,
  }))).sort((left, right) => right.usage.totalTokens - left.usage.totalTokens);
}

export function aggregateSelectedSources(
  sources: UsageSourceSummary[],
  selected: SourceSelection,
  period: UsagePeriod,
): DeviceUsageRangeSummary {
  const active = selected.length === 0 ? sources : sources.filter((source) => selected.includes(source.id));
  const daily = new Map<string, { usage: DeviceTokenUsage; cost: TokenCost }>();
  const hourly = new Map<number, { usage: DeviceTokenUsage; cost: TokenCost }>();
  let usage = { ...EMPTY_USAGE };
  let cost: TokenCost = { usd: 0, cny: 0, unpricedTokens: 0 };
  for (const source of active) {
    const range = sourceRange(source, period);
    usage = addUsage(usage, range.usage);
    cost = addCost(cost, range.cost);
    for (const bucket of range.dailyUsage) {
      const current = daily.get(bucket.date) ?? { usage: { ...EMPTY_USAGE }, cost: { usd: 0, cny: 0, unpricedTokens: 0 } };
      daily.set(bucket.date, {
        usage: addUsage(current.usage, bucket.usage),
        cost: addCost(current.cost, bucket.cost),
      });
    }
    for (const bucket of range.hourlyUsage ?? []) {
      const current = hourly.get(bucket.hourStartMs) ?? { usage: { ...EMPTY_USAGE }, cost: { usd: 0, cny: 0, unpricedTokens: 0 } };
      hourly.set(bucket.hourStartMs, {
        usage: addUsage(current.usage, bucket.usage),
        cost: addCost(current.cost, bucket.cost),
      });
    }
  }
  return {
    usage,
    cost,
    byModel: mergeDimensions(active, period, "byModel"),
    byProject: mergeDimensions(active, period, "byProject"),
    dailyUsage: [...daily.entries()].sort(([left], [right]) => left.localeCompare(right)).map(([date, value]) => ({ date, ...value })),
    hourlyUsage: [...hourly.entries()].sort(([left], [right]) => left - right).map(([hourStartMs, value]) => ({ hourStartMs, ...value })),
  };
}

export function attachCustomRanges(
  sources: UsageSourceSummary[],
  details: UsageDayDetail[],
): UsageSourceSummary[] {
  const normalizeSource = (value: string) => value.toLocaleLowerCase().replace(/[^a-z0-9]/g, "");
  return sources.map((source) => {
    const tasks = details.flatMap((detail) => detail.tasks
      .filter((task) => {
        const taskSource = normalizeSource(task.source);
        return taskSource === normalizeSource(source.id) || taskSource === normalizeSource(source.label);
      })
      .map((task) => ({ date: detail.date, task })));
    let usage = { ...EMPTY_USAGE };
    let cost: TokenCost = { usd: 0, cny: 0, unpricedTokens: 0 };
    const byModel = new Map<string, DeviceUsageDimension>();
    const byProject = new Map<string, DeviceUsageDimension>();
    const daily = new Map<string, { usage: DeviceTokenUsage; cost: TokenCost }>();
    const addDimension = (
      values: Map<string, DeviceUsageDimension>,
      label: string,
      taskUsage: DeviceTokenUsage,
      taskCost: TokenCost,
    ) => {
      const key = label.trim().toLocaleLowerCase() || "unknown";
      const current = values.get(key) ?? {
        key,
        label: label.trim() || "未知",
        usage: { ...EMPTY_USAGE },
        cost: { usd: 0, cny: 0, unpricedTokens: 0 },
      };
      current.usage = addUsage(current.usage, taskUsage);
      current.cost = addCost(current.cost, taskCost);
      values.set(key, current);
    };
    for (const { date, task } of tasks) {
      usage = addUsage(usage, task.usage);
      cost = addCost(cost, task.cost);
      addDimension(byModel, task.modelLabel, task.usage, task.cost);
      addDimension(byProject, task.projectLabel, task.usage, task.cost);
      const current = daily.get(date) ?? {
        usage: { ...EMPTY_USAGE },
        cost: { usd: 0, cny: 0, unpricedTokens: 0 },
      };
      daily.set(date, {
        usage: addUsage(current.usage, task.usage),
        cost: addCost(current.cost, task.cost),
      });
    }
    const custom: DeviceUsageRangeSummary = {
      usage,
      cost,
      byModel: [...byModel.values()].sort((left, right) => right.usage.totalTokens - left.usage.totalTokens),
      byProject: [...byProject.values()].sort((left, right) => right.usage.totalTokens - left.usage.totalTokens),
      dailyUsage: [...daily.entries()].sort(([left], [right]) => left.localeCompare(right)).map(([date, value]) => ({ date, ...value })),
    };
    return {
      ...source,
      ranges: source.ranges ? { ...source.ranges, custom } : undefined,
    };
  });
}

export function costDetailRows(
  sources: UsageSourceSummary[],
  period: UsagePeriod,
): CostDetailRow[] {
  return sources.map((source) => ({
    source,
    usage: sourceRange(source, period).usage,
    cost: sourceRange(source, period).cost,
  }));
}

function addCost(left: TokenCost, right: TokenCost): TokenCost {
  const unconfirmed = (left.longContextUnconfirmedEvents ?? 0) + (right.longContextUnconfirmedEvents ?? 0);
  return {
    usd: left.usd + right.usd,
    cny: left.cny + right.cny,
    unpricedTokens: left.unpricedTokens + right.unpricedTokens,
    ...(unconfirmed > 0 ? { longContextUnconfirmedEvents: unconfirmed } : {}),
  };
}

export function aggregateCostRows(rows: CostDetailRow[]): TokenCost {
  return rows.reduce((total, row) => addCost(total, row.cost), { usd: 0, cny: 0, unpricedTokens: 0 });
}

export function buildUsageInsights(summary: DeviceUsageRangeSummary): UsageInsightSummary {
  const values = summary.dailyUsage
    .filter((bucket) => Number.isFinite(bucket.usage.totalTokens) && bucket.usage.totalTokens >= 0)
    .sort((left, right) => left.date.localeCompare(right.date));
  const observedDays = values.length;
  const confidence = observedDays >= 21 ? "high" : observedDays >= 7 ? "medium" : "low";
  const peak = values.reduce<(typeof values)[number] | null>(
    (current, bucket) => current === null || bucket.usage.totalTokens > current.usage.totalTokens ? bucket : current,
    null,
  );
  if (observedDays < 2) {
    return { observedDays, confidence, recentAverageTokens: values.at(-1)?.usage.totalTokens ?? 0, baselineAverageTokens: 0, changePercent: null, trend: "insufficient", anomaly: "insufficient", peakDate: peak?.date ?? null, peakTokens: peak?.usage.totalTokens ?? 0 };
  }

  const windowSize = observedDays >= 14 ? 7 : 1;
  const recent = values.slice(-windowSize);
  const baselinePool = values.slice(0, -windowSize);
  const baseline = observedDays >= 14 ? baselinePool.slice(-7) : baselinePool;
  const average = (items: typeof values) => items.length === 0 ? 0 : items.reduce((sum, bucket) => sum + bucket.usage.totalTokens, 0) / items.length;
  const recentAverageTokens = average(recent);
  const baselineAverageTokens = average(baseline);
  const changePercent = baselineAverageTokens > 0
    ? (recentAverageTokens - baselineAverageTokens) / baselineAverageTokens * 100
    : null;
  const trend = changePercent === null
    ? "insufficient"
    : changePercent > 20 ? "up" : changePercent < -20 ? "down" : "stable";
  const latestTokens = values.at(-1)?.usage.totalTokens ?? 0;
  const historicalAverage = average(values.slice(0, -1));
  const anomaly = observedDays < 7 || historicalAverage <= 0
    ? "insufficient"
    : latestTokens >= historicalAverage * 1.5
      ? "spike"
      : latestTokens <= historicalAverage * 0.5
        ? "drop"
        : "normal";
  return { observedDays, confidence, recentAverageTokens, baselineAverageTokens, changePercent, trend, anomaly, peakDate: peak?.date ?? null, peakTokens: peak?.usage.totalTokens ?? 0 };
}
