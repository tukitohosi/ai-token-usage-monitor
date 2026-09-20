import type {
  AccountUsageReadResult,
  AccountUsageSummary,
  CreditBalance,
  DailyUsageBucket,
  NormalizedQuotaWindow,
  QuotaWindow,
  RateLimitBucket,
  RateLimitResetCredits,
  RateLimitsReadResult,
} from "../types.js";

type UnknownRecord = Record<string, unknown>;

function isRecord(value: unknown): value is UnknownRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function finiteNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function nonNegativeInteger(value: unknown): number | null {
  const number = finiteNumber(value);
  return number !== null && number >= 0 ? Math.trunc(number) : null;
}

function optionalString(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}

function coerceWindow(value: unknown): QuotaWindow | null {
  if (!isRecord(value)) {
    return null;
  }

  const usedPercent = finiteNumber(value.usedPercent);
  const windowDurationMins = finiteNumber(value.windowDurationMins);
  const resetsAt = finiteNumber(value.resetsAt);
  if (
    usedPercent === null ||
    windowDurationMins === null ||
    windowDurationMins <= 0 ||
    resetsAt === null ||
    resetsAt < 0
  ) {
    return null;
  }

  return {
    usedPercent,
    windowDurationMins,
    resetsAt,
  };
}

function coerceCredits(value: unknown): CreditBalance | null {
  if (
    !isRecord(value) ||
    typeof value.hasCredits !== "boolean" ||
    typeof value.unlimited !== "boolean"
  ) {
    return null;
  }

  return {
    hasCredits: value.hasCredits,
    unlimited: value.unlimited,
    balance: typeof value.balance === "string" ? value.balance : null,
  };
}

function coerceBucket(
  value: unknown,
  fallbackLimitId: string,
): RateLimitBucket | null {
  if (!isRecord(value)) {
    return null;
  }

  return {
    limitId: optionalString(value.limitId) ?? fallbackLimitId,
    limitName: optionalString(value.limitName),
    normalModelSlug: optionalString(value.normalModelSlug),
    primary: coerceWindow(value.primary),
    secondary: coerceWindow(value.secondary),
    credits: coerceCredits(value.credits),
    planType: optionalString(value.planType),
    rateLimitReachedType: optionalString(value.rateLimitReachedType),
  };
}

function coerceResetCredits(value: unknown): RateLimitResetCredits | null {
  if (!isRecord(value)) {
    return null;
  }

  const availableCount = nonNegativeInteger(value.availableCount);
  if (availableCount === null) {
    return null;
  }

  const credits = Array.isArray(value.credits)
    ? value.credits.flatMap((candidate) => {
        if (!isRecord(candidate)) {
          return [];
        }
        const id = optionalString(candidate.id);
        const resetType = optionalString(candidate.resetType);
        const status = optionalString(candidate.status);
        const grantedAt = finiteNumber(candidate.grantedAt);
        if (
          id === null ||
          resetType === null ||
          status === null ||
          grantedAt === null
        ) {
          return [];
        }
        return [
          {
            id,
            resetType,
            status,
            grantedAt,
            expiresAt: finiteNumber(candidate.expiresAt),
            title: optionalString(candidate.title),
            description: optionalString(candidate.description),
          },
        ];
      })
    : null;

  return { availableCount, credits };
}

/** Converts version-dependent JSON into the stable core contract. */
export function coerceRateLimitsReadResult(value: unknown): RateLimitsReadResult {
  if (!isRecord(value)) {
    return {};
  }

  const legacy = coerceBucket(value.rateLimits, "legacy");
  let rateLimitsByLimitId: Record<string, RateLimitBucket> | null = null;
  if (isRecord(value.rateLimitsByLimitId)) {
    rateLimitsByLimitId = {};
    for (const [limitId, rawBucket] of Object.entries(value.rateLimitsByLimitId)) {
      const bucket = coerceBucket(rawBucket, limitId);
      if (bucket) {
        // The map key is the authoritative metered identifier. Do not let an
        // inconsistent inner snapshot merge two independently metered buckets.
        rateLimitsByLimitId[limitId] = {
          ...bucket,
          limitId,
        };
      }
    }
  }

  return {
    rateLimits: legacy,
    rateLimitsByLimitId,
    rateLimitResetCredits: coerceResetCredits(value.rateLimitResetCredits),
  };
}

export function formatWindowDurationLabel(windowDurationMins: number): string {
  if (windowDurationMins >= 1_440 && windowDurationMins % 1_440 === 0) {
    return `${windowDurationMins / 1_440} 天`;
  }
  if (windowDurationMins >= 60 && windowDurationMins % 60 === 0) {
    return `${windowDurationMins / 60} 小时`;
  }
  return `${windowDurationMins} 分钟`;
}

/**
 * Produces display windows. The multi-bucket response is authoritative whenever
 * it contains at least one bucket; the historical single bucket is only a
 * compatibility fallback.
 */
export function normalizeRateLimits(value: unknown): NormalizedQuotaWindow[] {
  const response = coerceRateLimitsReadResult(value);
  const multiBucketEntries = Object.entries(response.rateLimitsByLimitId ?? {});
  const buckets: Array<[string, RateLimitBucket]> =
    multiBucketEntries.length > 0
      ? multiBucketEntries
      : response.rateLimits
        ? [[response.rateLimits.limitId || "legacy", response.rateLimits]]
        : [];

  const windows: NormalizedQuotaWindow[] = [];
  for (const [fallbackLimitId, bucket] of buckets) {
    const limitId = bucket.limitId || fallbackLimitId;
    for (const lane of ["primary", "secondary"] as const) {
      const window = bucket[lane];
      if (!window) {
        continue;
      }
      const boundedUsedPercent = Math.min(100, Math.max(0, window.usedPercent));
      windows.push({
        ...window,
        usedPercent: boundedUsedPercent,
        key: `${limitId}:${lane}`,
        limitId,
        limitName: bucket.limitName,
        normalModelSlug: bucket.normalModelSlug ?? null,
        lane,
        label: formatWindowDurationLabel(window.windowDurationMins),
        remainingPercent: 100 - boundedUsedPercent,
        credits: bucket.credits ?? null,
        planType: bucket.planType ?? null,
        reachedType: bucket.rateLimitReachedType ?? null,
      });
    }
  }

  return windows.sort((left, right) => {
    const priority = quotaWindowPriority(left) - quotaWindowPriority(right);
    return priority || left.key.localeCompare(right.key);
  });
}

function quotaWindowPriority(window: NormalizedQuotaWindow): number {
  const limitId = window.limitId.toLocaleLowerCase();
  const limitName = window.limitName?.toLocaleLowerCase() ?? "";
  const model = window.normalModelSlug?.toLocaleLowerCase() ?? "";
  const codex = limitId === "codex" || limitName === "codex";
  if (codex && window.windowDurationMins === 300) return 0;
  if (codex && window.windowDurationMins === 10_080) return 1;
  const lunaReserve = limitId === "base_model_inference"
    || limitName === "gpt-reserve"
    || model === "gpt-5.6-luna";
  if (lunaReserve) return 2;
  return 3;
}

function coerceUsageSummary(value: unknown): AccountUsageSummary | null {
  if (!isRecord(value)) {
    return null;
  }

  const field = (name: keyof AccountUsageSummary): number | null => {
    const number = finiteNumber(value[name]);
    return number !== null && number >= 0 ? number : null;
  };
  return {
    lifetimeTokens: field("lifetimeTokens"),
    peakDailyTokens: field("peakDailyTokens"),
    longestRunningTurnSec: field("longestRunningTurnSec"),
    currentStreakDays: field("currentStreakDays"),
    longestStreakDays: field("longestStreakDays"),
  };
}

function coerceDailyUsage(value: unknown): DailyUsageBucket[] | null {
  if (!Array.isArray(value)) {
    return null;
  }

  return value.flatMap((candidate) => {
    if (!isRecord(candidate)) {
      return [];
    }
    const startDate = optionalString(candidate.startDate);
    const tokens = finiteNumber(candidate.tokens);
    return startDate && tokens !== null && tokens >= 0
      ? [{ startDate, tokens }]
      : [];
  });
}

export function coerceAccountUsageReadResult(value: unknown): AccountUsageReadResult {
  if (!isRecord(value)) {
    return { summary: null, dailyUsageBuckets: null, threadUsage: null };
  }

  return {
    summary: coerceUsageSummary(value.summary),
    dailyUsageBuckets: coerceDailyUsage(value.dailyUsageBuckets),
    threadUsage: value.threadUsage ?? null,
  };
}
