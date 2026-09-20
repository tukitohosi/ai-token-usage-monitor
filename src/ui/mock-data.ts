import type {
  DashboardSnapshot,
  DeviceDailyUsageBucket,
  DailyUsageBucket,
  DeviceTokenUsage,
  DeviceUsageDimension,
  DeviceUsageRangeSummary,
  DeviceUsageRanges,
  DeviceUsageSummary,
  LocalUsageSummary,
  NormalizedQuotaWindow,
  TokenCost,
  UsageSourceSummary,
} from "../core/types.js";

export type MockDashboardState = DashboardSnapshot["status"] | "stale";

function localDateKey(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function createDailyBuckets(now: number): DailyUsageBucket[] {
  const values = [186_420, 248_730, 91_580, 326_940, 278_160, 412_680, 340_190];
  return values.map((tokens, index) => {
    const date = new Date(now);
    date.setHours(12, 0, 0, 0);
    date.setDate(date.getDate() - (values.length - 1 - index));
    return { startDate: localDateKey(date), tokens };
  });
}

function createQuotaWindows(now: number): NormalizedQuotaWindow[] {
  return [
    {
      key: "base_model_inference:primary",
      limitId: "base_model_inference",
      limitName: "gpt-reserve",
      normalModelSlug: "gpt-5.6-luna",
      lane: "primary",
      label: "7 天",
      usedPercent: 100,
      remainingPercent: 0,
      windowDurationMins: 10_080,
      resetsAt: now + (2 * 24 * 60 + 23 * 60) * 60_000,
      credits: null,
      planType: "plus",
      reachedType: "rate_limit",
    },
    {
      key: "codex-primary",
      limitId: "codex",
      limitName: "Codex",
      normalModelSlug: null,
      lane: "primary",
      label: "短周期额度",
      usedPercent: 64,
      remainingPercent: 36,
      windowDurationMins: 300,
      resetsAt: now + (2 * 60 + 14) * 60_000,
      credits: { hasCredits: true, unlimited: false, balance: "$18.75" },
      planType: "plus",
      reachedType: null,
    },
    {
      key: "codex-secondary",
      limitId: "codex",
      limitName: "Codex",
      normalModelSlug: null,
      lane: "secondary",
      label: "长周期额度",
      usedPercent: 38.5,
      remainingPercent: 61.5,
      windowDurationMins: 10_080,
      resetsAt: now + (4 * 24 * 60 + 7 * 60 + 31) * 60_000,
      credits: { hasCredits: true, unlimited: false, balance: "$18.75" },
      planType: "plus",
      reachedType: null,
    },
  ];
}

function createLocalUsage(now: number): LocalUsageSummary {
  return {
    generatedAt: new Date(now - 18_000).toISOString(),
    sourceFiles: 243,
    indexedEvents: 13_842,
    skippedEvents: 4,
    filteredParentEvents: 2_310,
    total: {
      inputTokens: 12_481_360,
      cachedInputTokens: 8_902_440,
      outputTokens: 1_642_780,
      reasoningOutputTokens: 618_920,
      totalTokens: 14_124_140,
    },
    today: {
      inputTokens: 298_430,
      cachedInputTokens: 211_920,
      outputTokens: 41_760,
      reasoningOutputTokens: 17_830,
      totalTokens: 340_190,
    },
    byModel: [
      {
        key: "gpt-5.6-sol",
        label: "GPT-5.6 Sol",
        usage: {
          inputTokens: 7_492_600,
          cachedInputTokens: 5_456_180,
          outputTokens: 928_720,
          reasoningOutputTokens: 349_830,
          totalTokens: 8_421_320,
        },
      },
      {
        key: "gpt-5.6-terra",
        label: "GPT-5.6 Terra",
        usage: {
          inputTokens: 3_614_420,
          cachedInputTokens: 2_561_140,
          outputTokens: 512_460,
          reasoningOutputTokens: 196_230,
          totalTokens: 4_126_880,
        },
      },
      {
        key: "gpt-5.4",
        label: "GPT-5.4",
        usage: {
          inputTokens: 1_374_340,
          cachedInputTokens: 885_120,
          outputTokens: 201_600,
          reasoningOutputTokens: 72_860,
          totalTokens: 1_575_940,
        },
      },
    ],
    byProject: [
      {
        key: "codex-usage-monitor",
        label: "用量监控项目",
        usage: {
          inputTokens: 4_902_180,
          cachedInputTokens: 3_719_420,
          outputTokens: 644_860,
          reasoningOutputTokens: 251_400,
          totalTokens: 5_547_040,
        },
      },
      {
        key: "desktop-tool",
        label: "桌面工具项目",
        usage: {
          inputTokens: 3_861_500,
          cachedInputTokens: 2_710_300,
          outputTokens: 478_200,
          reasoningOutputTokens: 182_460,
          totalTokens: 4_339_700,
        },
      },
      {
        key: "sample-workspace",
        label: "示例工作区",
        usage: {
          inputTokens: 2_718_440,
          cachedInputTokens: 1_809_220,
          outputTokens: 365_180,
          reasoningOutputTokens: 131_700,
          totalTokens: 3_083_620,
        },
      },
      {
        key: "other",
        label: "其他本机任务",
        usage: {
          inputTokens: 999_240,
          cachedInputTokens: 663_500,
          outputTokens: 154_540,
          reasoningOutputTokens: 53_360,
          totalTokens: 1_153_780,
        },
      },
    ],
    warnings: ["有 4 条未写完的日志记录已安全跳过，下次扫描会自动重试。"],
  };
}

function deviceUsage(
  inputTokens: number,
  cachedInputTokens: number,
  cacheWriteTokens: number,
  outputTokens: number,
  reasoningOutputTokens: number,
  totalTokens: number,
): DeviceTokenUsage {
  return {
    inputTokens,
    cachedInputTokens,
    cacheWriteTokens,
    cacheWrite5mTokens: 0,
    cacheWrite1hTokens: 0,
    cacheWriteUnknownTokens: cacheWriteTokens,
    outputTokens,
    reasoningOutputTokens,
    totalTokens,
  };
}

function cost(usd = 0, cny = 0, unpricedTokens = 0): TokenCost {
  return { usd, cny, unpricedTokens };
}

function addDeviceUsage(items: DeviceTokenUsage[]): DeviceTokenUsage {
  return items.reduce(
    (total, item) => deviceUsage(
      total.inputTokens + item.inputTokens,
      total.cachedInputTokens + item.cachedInputTokens,
      total.cacheWriteTokens + item.cacheWriteTokens,
      total.outputTokens + item.outputTokens,
      total.reasoningOutputTokens + item.reasoningOutputTokens,
      total.totalTokens + item.totalTokens,
    ),
    deviceUsage(0, 0, 0, 0, 0, 0),
  );
}

function scaleDeviceUsage(usage: DeviceTokenUsage, factor: number): DeviceTokenUsage {
  const scale = (value: number) => Math.max(0, Math.round(value * factor));
  return deviceUsage(
    scale(usage.inputTokens),
    scale(usage.cachedInputTokens),
    scale(usage.cacheWriteTokens),
    scale(usage.outputTokens),
    scale(usage.reasoningOutputTokens),
    scale(usage.totalTokens),
  );
}

function scaleCost(value: TokenCost, factor: number): TokenCost {
  return {
    usd: value.usd * factor,
    cny: value.cny * factor,
    unpricedTokens: Math.round(value.unpricedTokens * factor),
  };
}

function mockDailyUsage(today: DeviceTokenUsage, todayCost: TokenCost): DeviceDailyUsageBucket[] {
  const factors = [0.42, 0.68, 0.35, 0.91, 0.73, 1.12, 0.88, 0.57, 0.76, 0.63, 1.24, 0.96, 0.71, 0.84, 0.52, 0.47, 1.08, 0.79, 0.66, 0.93, 0.58, 1.18, 0.86, 0.74, 1.05, 0.69, 0.82, 1.16, 0.94, 1];
  return factors.map((factor, index) => {
    const date = new Date();
    date.setHours(12, 0, 0, 0);
    date.setDate(date.getDate() - (factors.length - 1 - index));
    return {
      date: localDateKey(date),
      usage: scaleDeviceUsage(today, factor),
      cost: scaleCost(todayCost, factor),
    };
  });
}

function rangeFromDaily(
  buckets: DeviceDailyUsageBucket[],
  modelLabel: string,
  projectLabel: string,
): DeviceUsageRangeSummary {
  const usage = addDeviceUsage(buckets.map((bucket) => bucket.usage));
  const value = buckets.reduce((total, bucket) => ({
    usd: total.usd + bucket.cost.usd,
    cny: total.cny + bucket.cost.cny,
    unpricedTokens: total.unpricedTokens + bucket.cost.unpricedTokens,
  }), cost());
  const dimension = (key: string, label: string): DeviceUsageDimension => ({ key, label, usage, cost: value });
  const hourlyUsage = Array.from({ length: 24 }, (_, index) => {
    const hourStart = new Date();
    hourStart.setMinutes(0, 0, 0);
    hourStart.setHours(hourStart.getHours() - (23 - index));
    const factor = index % 6 === 0 ? 0.11 : index % 3 === 0 ? 0.06 : 0.025;
    return {
      hourStartMs: hourStart.getTime(),
      usage: scaleDeviceUsage(buckets.at(-1)?.usage ?? usage, factor),
      cost: scaleCost(buckets.at(-1)?.cost ?? value, factor),
    };
  });
  return {
    usage,
    cost: value,
    byModel: [dimension("range-model", modelLabel)],
    byProject: [dimension("range-project", projectLabel)],
    dailyUsage: buckets,
    hourlyUsage,
  };
}

function createRanges(
  total: DeviceTokenUsage,
  today: DeviceTokenUsage,
  totalCost: TokenCost,
  todayCost: TokenCost,
  modelLabel: string,
  projectLabel: string,
): DeviceUsageRanges {
  const daily = mockDailyUsage(today, todayCost);
  const todayRange = rangeFromDaily(daily.slice(-1), modelLabel, projectLabel);
  const last7Days = rangeFromDaily(daily.slice(-7), modelLabel, projectLabel);
  const last30Days = rangeFromDaily(daily, modelLabel, projectLabel);
  return {
    today: todayRange,
    last24Hours: todayRange,
    last7Days,
    last30Days,
    total: {
      usage: total,
      cost: totalCost,
      byModel: [{ key: "total-model", label: modelLabel, usage: total, cost: totalCost }],
      byProject: [{ key: "total-project", label: projectLabel, usage: total, cost: totalCost }],
      dailyUsage: daily,
      hourlyUsage: last30Days.hourlyUsage,
    },
  };
}

function combineSourceRanges(sources: UsageSourceSummary[]): DeviceUsageRanges {
  const combine = (period: keyof DeviceUsageRanges): DeviceUsageRangeSummary => {
    const ranges = sources.map((source) => source.ranges![period] ?? source.ranges!.today);
    const daily = new Map<string, DeviceDailyUsageBucket>();
    const hourly = new Map<number, NonNullable<DeviceUsageRangeSummary["hourlyUsage"]>[number]>();
    for (const range of ranges) {
      for (const bucket of range.dailyUsage) {
        const current = daily.get(bucket.date);
        daily.set(bucket.date, current ? {
          date: bucket.date,
          usage: addDeviceUsage([current.usage, bucket.usage]),
          cost: {
            usd: current.cost.usd + bucket.cost.usd,
            cny: current.cost.cny + bucket.cost.cny,
            unpricedTokens: current.cost.unpricedTokens + bucket.cost.unpricedTokens,
          },
        } : bucket);
      }
      for (const bucket of range.hourlyUsage ?? []) {
        const current = hourly.get(bucket.hourStartMs);
        hourly.set(bucket.hourStartMs, current ? {
          hourStartMs: bucket.hourStartMs,
          usage: addDeviceUsage([current.usage, bucket.usage]),
          cost: {
            usd: current.cost.usd + bucket.cost.usd,
            cny: current.cost.cny + bucket.cost.cny,
            unpricedTokens: current.cost.unpricedTokens + bucket.cost.unpricedTokens,
          },
        } : bucket);
      }
    }
    return {
      usage: addDeviceUsage(ranges.map((range) => range.usage)),
      cost: ranges.reduce((total, range) => ({
        usd: total.usd + range.cost.usd,
        cny: total.cny + range.cost.cny,
        unpricedTokens: total.unpricedTokens + range.cost.unpricedTokens,
      }), cost()),
      byModel: sources.flatMap((source) => (source.ranges![period] ?? source.ranges!.today).byModel.map((dimension) => ({
        ...dimension,
        key: `${source.id}:${dimension.key}`,
        label: `${source.label} · ${dimension.label}`,
      }))),
      byProject: sources.flatMap((source) => (source.ranges![period] ?? source.ranges!.today).byProject.map((dimension) => ({
        ...dimension,
        key: `${source.id}:${dimension.key}`,
        label: `${source.label} · ${dimension.label}`,
      }))),
      dailyUsage: [...daily.values()].sort((left, right) => left.date.localeCompare(right.date)),
      hourlyUsage: [...hourly.values()].sort((left, right) => left.hourStartMs - right.hourStartMs),
    };
  };
  return {
    today: combine("today"),
    last24Hours: combine("last24Hours"),
    last7Days: combine("last7Days"),
    last30Days: combine("last30Days"),
    total: combine("total"),
  };
}

function fromLegacyUsage(usage: LocalUsageSummary["total"]): DeviceTokenUsage {
  return deviceUsage(
    Math.max(0, usage.inputTokens - usage.cachedInputTokens),
    usage.cachedInputTokens,
    0,
    usage.outputTokens,
    usage.reasoningOutputTokens,
    usage.totalTokens,
  );
}

function createSource(
  id: UsageSourceSummary["id"],
  label: string,
  total: DeviceTokenUsage,
  today: DeviceTokenUsage,
  status: UsageSourceSummary["status"] = "ready",
  sourceFiles = 0,
  indexedEvents = 0,
  message: string | null = null,
): UsageSourceSummary {
  const totalCost = cost(id === "codex" ? 89.61 : 0, 0, id === "codex" ? 0 : total.totalTokens);
  const todayCost = cost(id === "codex" ? 2.42 : 0, 0, id === "codex" ? 0 : today.totalTokens);
  const modelLabel = id === "codex" ? "GPT-5.6 Sol" : "默认模型";
  const projectLabel = id === "codex" ? "用量监控项目" : "脱敏项目";
  return {
    id,
    label,
    status,
    message,
    sourceFiles,
    indexedEvents,
    newEvents: status === "ready" ? Math.min(indexedEvents, id === "codex" ? 0 : 12) : null,
    skippedRecords: status === "ready" ? (id === "claude-code" ? 2 : 0) : null,
    lastIndexedAt: status === "ready" ? new Date().toISOString() : null,
    total,
    today,
    cost: totalCost,
    todayCost,
    byModel: [{ key: `${id}-default`, label: modelLabel, usage: total, cost: totalCost }],
    todayByModel: [{ key: `${id}-default`, label: modelLabel, usage: today, cost: todayCost }],
    byProject: [{ key: `${id}-project`, label: projectLabel, usage: total, cost: totalCost }],
    credits: null,
    ranges: createRanges(total, today, totalCost, todayCost, modelLabel, projectLabel),
  };
}

function createDeviceUsage(now: number, local: LocalUsageSummary): DeviceUsageSummary {
  const codex = createSource("codex", "Codex", fromLegacyUsage(local.total), fromLegacyUsage(local.today), "ready", local.sourceFiles, local.indexedEvents);
  const claude = createSource(
    "claude-code", "Claude Code", deviceUsage(840_000, 322_000, 71_000, 174_000, 49_000, 1_407_000),
    deviceUsage(45_000, 22_000, 4_000, 13_000, 3_400, 87_400), "ready", 38, 2_440,
  );
  const opencode = createSource(
    "opencode", "OpenCode", deviceUsage(286_000, 96_000, 18_000, 71_000, 14_500, 485_500),
    deviceUsage(29_000, 12_000, 2_400, 7_200, 1_500, 52_100), "ready", 1, 831,
  );
  const workbuddy = createSource(
    "workbuddy", "WorkBuddy", deviceUsage(141_000, 51_000, 8_000, 36_000, 9_000, 245_000),
    deviceUsage(7_600, 2_500, 800, 2_400, 600, 13_900), "ready", 12, 289,
  );
  const workbuddyAi = createSource(
    "workbuddy-ai", "WorkBuddy AI", deviceUsage(0, 0, 0, 0, 0, 0),
    deviceUsage(0, 0, 0, 0, 0, 0), "notDetected", 0, 0, "尚未检测到可索引的本机 trace。",
  );
  const cursor = createSource(
    "cursor", "Cursor", deviceUsage(0, 0, 0, 0, 0, 0),
    deviceUsage(0, 0, 0, 0, 0, 0), "unavailable", 0, 0, "等待官方个人数据来源；不会读取密钥或未公开接口。",
  );
  const sources = [codex, claude, opencode, workbuddy, workbuddyAi, cursor];
  const ranges = combineSourceRanges(sources);

  return {
    generatedAt: new Date(now - 18_000).toISOString(),
    priceSnapshotDate: "2026-09-20",
    priceCatalog: [
      {
        displayName: "GPT-6 Astra",
        modelId: "gpt-6-astra",
        aliases: ["gpt6-astra"],
        currency: "USD",
        sourceLabel: "OpenAI 官方 Standard API",
        tiers: [
          { label: "短上下文", condition: "单次输入不超过 272K", inputPerMillion: 10, cachedInputPerMillion: 1, cacheWritePerMillion: 12.5, outputPerMillion: 50 },
          { label: "长上下文", condition: "单次输入超过 272K", inputPerMillion: 20, cachedInputPerMillion: 2, cacheWritePerMillion: 25, outputPerMillion: 75 },
        ],
      },
      {
        displayName: "DeepSeek V4.1 Flash",
        modelId: "deepseek-flash",
        aliases: ["deepseek-v4.1-flash", "deepseek-v4-flash", "deepseek-v4-flash-vision-exp"],
        currency: "USD",
        sourceLabel: "DeepSeek 官方 API",
        tiers: [
          { label: "谷值", condition: "周末、中国法定节假日及非峰值 UTC 时段", inputPerMillion: 0.15, cachedInputPerMillion: 0.003, cacheWritePerMillion: null, outputPerMillion: 0.6 },
          { label: "峰值", condition: "工作日 01:00-04:00、06:00-10:00 UTC", inputPerMillion: 0.3, cachedInputPerMillion: 0.006, cacheWritePerMillion: null, outputPerMillion: 1.2 },
        ],
      },
      {
        displayName: "Tencent Hy4 preview",
        modelId: "hy4-preview",
        aliases: [],
        currency: "CNY",
        sourceLabel: "腾讯云官方 Standard API",
        tiers: [
          { label: "标准", condition: "缓存写入价格未公布", inputPerMillion: 6, cachedInputPerMillion: 0.3, cacheWritePerMillion: null, outputPerMillion: 18 },
        ],
      },
    ],
    total: addDeviceUsage(sources.map((source) => source.total)),
    today: addDeviceUsage(sources.map((source) => source.today)),
    cost: cost(89.61, 0, sources.filter((source) => source.id !== "codex").reduce((sum, source) => sum + source.total.totalTokens, 0)),
    todayCost: cost(2.42, 0, sources.filter((source) => source.id !== "codex").reduce((sum, source) => sum + source.today.totalTokens, 0)),
    sources,
    byModel: sources.filter((source) => source.status === "ready").map((source) => ({
      key: source.id,
      label: source.label,
      usage: source.total,
      cost: source.cost,
    })),
    todayByModel: sources.filter((source) => source.status === "ready").map((source) => ({
      key: source.id,
      label: source.label,
      usage: source.today,
      cost: source.todayCost,
    })),
    byProject: sources.filter((source) => source.status === "ready").map((source) => ({
      key: `${source.id}-project`,
      label: `${source.label} 脱敏项目`,
      usage: source.total,
      cost: source.cost,
    })),
    warnings: ["演示模式：所有本机活动数字均为示例，Credits 未计入 Token 总量。"],
    ranges,
  };
}

export function createMockDashboardSnapshot(state: MockDashboardState = "ready"): DashboardSnapshot {
  const now = Date.now();
  const isStale = state === "stale" || state === "offline" || state === "error";
  const localUsage = createLocalUsage(now);
  const deviceUsage = createDeviceUsage(now, localUsage);
  const quotaWindows = createQuotaWindows(now);
  const fetchedAt = new Date(now - (isStale ? 12 * 60_000 : 24_000)).toISOString();

  const base: DashboardSnapshot = {
    status: state === "stale" ? "ready" : state,
    fetchedAt,
    codexVersion: "0.150.0-alpha.8",
    quotaWindows,
    accountDiagnostics: {
      readAt: fetchedAt,
      durationMs: 91,
      methods: ["account/read", "account/rateLimits/read", "account/usage/read"],
    },
    // The official App Server has no subscription-renewal field. Keep the
    // demo honest and exercise the prominent "not provided" UI state.
    planRenewalAt: null,
    planRenewalSource: null,
    resetCredits: {
      availableCount: 1,
      credits: [
        {
          id: "reset-credit-demo",
          resetType: "rate_limit",
          status: "available",
          grantedAt: now - 9 * 24 * 60 * 60_000,
          expiresAt: now + 51 * 24 * 60 * 60_000,
          title: "Codex 额度重置券",
          description: "用于重置一次可用额度窗口。",
        },
      ],
    },
    accountUsage: {
      summary: {
        lifetimeTokens: 48_932_740,
        peakDailyTokens: 612_480,
        longestRunningTurnSec: 1_842,
        currentStreakDays: 9,
        longestStreakDays: 21,
      },
      dailyUsageBuckets: createDailyBuckets(now),
      threadUsage: null,
    },
    localUsage,
    deviceUsage,
    lastSuccessfulAt: fetchedAt,
    refreshProgress: {
      phase: state === "loading" ? "codex" : "complete",
      label: state === "loading" ? "正在更新 Claude Code（18/38 个文件）" : "刷新完成",
      processedSources: state === "loading" ? 1 : 6,
      totalSources: 6,
      currentSource: state === "loading" ? "Claude Code" : null,
      processedFiles: state === "loading" ? 18 : 0,
      totalFiles: state === "loading" ? 38 : 0,
    },
    refreshSchedule: {
      nextRefreshAtMs: now + 60_000,
      intervalSeconds: 60,
    },
    sourceHealth: deviceUsage.sources.map((source) => ({
      id: source.id,
      label: source.label,
      status: source.status,
      sourceFiles: source.sourceFiles,
      indexedEvents: source.indexedEvents,
      newEvents: source.newEvents,
      skippedRecords: source.skippedRecords,
      lastIndexedAt: source.status === "ready" ? deviceUsage.generatedAt : null,
      message: source.message,
    })),
    indexDiagnostics: {
      totalDurationMs: 846,
      codexDurationMs: 312,
      sourcesDurationMs: 428,
      accountDurationMs: 91,
      databases: [
        { label: "Codex 索引", sizeBytes: 18_624_512 },
        { label: "多来源索引", sizeBytes: 7_340_032 },
      ],
    },
    message: "演示模式：以下数字均为示例，不是您的真实账号或本机用量。",
  };

  switch (state) {
    case "offline":
      return {
        ...base,
        status: "offline",
        message: "暂时无法连接 Codex，正在显示最近一次成功读取的数据。",
      };
    case "unauthenticated":
      return {
        ...base,
        status: "unauthenticated",
        fetchedAt: new Date(now).toISOString(),
        quotaWindows: [],
        accountDiagnostics: null,
        resetCredits: null,
        accountUsage: null,
        message: "Codex 尚未登录。登录后可读取账号额度；本机日志统计仍可使用。",
      };
    case "unsupported":
      return {
        ...base,
        status: "unsupported",
        fetchedAt: new Date(now).toISOString(),
        quotaWindows: [],
        resetCredits: null,
        accountUsage: null,
        message: "当前 Codex 版本不支持所需的用量接口。本机统计已保留。",
      };
    case "error":
      return {
        ...base,
        status: "error",
        message: "读取服务端额度时出错，已保留上次的可用数据。",
      };
    case "loading":
      return {
        status: "loading",
        fetchedAt: null,
        codexVersion: base.codexVersion,
        quotaWindows: [],
        planRenewalAt: null,
        planRenewalSource: null,
        resetCredits: null,
        accountUsage: null,
        localUsage: null,
        deviceUsage: null,
        lastSuccessfulAt: null,
        refreshProgress: {
          phase: "codex",
          label: "正在更新 Claude Code（18/38 个文件）",
          processedSources: 1,
          totalSources: 6,
          currentSource: "Claude Code",
          processedFiles: 18,
          totalFiles: 38,
        },
        sourceHealth: [],
        indexDiagnostics: null,
        message: "正在读取 Codex 用量…",
      };
    case "ready":
    case "stale":
    default:
      return base;
  }
}
