import { describe, expect, it } from "vitest";

import {
  coerceAccountUsageReadResult,
  coerceRateLimitsReadResult,
  formatWindowDurationLabel,
  normalizeRateLimits,
} from "../src/core/app-server/index.js";

describe("rate-limit normalization", () => {
  it("prefers multi-bucket data and labels windows from their durations", () => {
    const windows = normalizeRateLimits({
      rateLimits: {
        limitId: "legacy",
        primary: {
          usedPercent: 99,
          windowDurationMins: 60,
          resetsAt: 1,
        },
      },
      rateLimitsByLimitId: {
        codex: {
          limitId: "inconsistent-inner-id",
          limitName: "Codex",
          normalModelSlug: "gpt-5.6-sol",
          primary: {
            usedPercent: 40,
            windowDurationMins: 300,
            resetsAt: 1_800_000_000,
          },
          secondary: {
            usedPercent: 110,
            windowDurationMins: 10_080,
            resetsAt: 1_800_100_000,
          },
          credits: {
            hasCredits: true,
            unlimited: false,
            balance: "12.5",
          },
          planType: "plus",
          rateLimitReachedType: null,
        },
      },
    });

    expect(windows).toHaveLength(2);
    expect(windows.map((window) => window.label)).toEqual([
      "5 小时",
      "7 天",
    ]);
    expect(windows[0]).toMatchObject({
      key: "codex:primary",
      limitId: "codex",
      usedPercent: 40,
      remainingPercent: 60,
      normalModelSlug: "gpt-5.6-sol",
    });
    expect(windows[1]).toMatchObject({
      key: "codex:secondary",
      usedPercent: 100,
      remainingPercent: 0,
    });
    expect(windows.some((window) => window.limitId === "legacy")).toBe(false);
    expect(
      windows.some((window) => window.limitId === "inconsistent-inner-id"),
    ).toBe(false);
  });

  it("preserves Luna reserve model identity even when its bucket is first", () => {
    const windows = normalizeRateLimits({
      rateLimitsByLimitId: {
        base_model_inference: {
          limitName: "gpt-reserve",
          normalModelSlug: "gpt-5.6-luna",
          primary: { usedPercent: 100, windowDurationMins: 10_080, resetsAt: 20 },
        },
        codex: {
          primary: { usedPercent: 15, windowDurationMins: 300, resetsAt: 10 },
          secondary: { usedPercent: 21, windowDurationMins: 10_080, resetsAt: 20 },
        },
      },
    });
    expect(windows).toHaveLength(3);
    expect(windows.map((window) => window.key)).toEqual([
      "codex:primary",
      "codex:secondary",
      "base_model_inference:primary",
    ]);
    expect(windows[2]).toMatchObject({
      limitId: "base_model_inference",
      limitName: "gpt-reserve",
      normalModelSlug: "gpt-5.6-luna",
      remainingPercent: 0,
    });
    expect(windows.filter((window) => window.limitId === "codex")).toHaveLength(2);
  });

  it("falls back to the historical bucket when the multi-bucket map is empty", () => {
    const windows = normalizeRateLimits({
      rateLimits: {
        limitId: null,
        limitName: null,
        primary: {
          usedPercent: -20,
          windowDurationMins: 90,
          resetsAt: 123,
        },
      },
      rateLimitsByLimitId: {},
    });

    expect(windows).toEqual([
      expect.objectContaining({
        key: "legacy:primary",
        label: "90 分钟",
        usedPercent: 0,
        remainingPercent: 100,
      }),
    ]);
  });

  it("tolerates null and unknown fields without inventing quota values", () => {
    expect(() => normalizeRateLimits(null)).not.toThrow();
    expect(
      normalizeRateLimits({
        rateLimits: {
          primary: {
            usedPercent: 25,
            windowDurationMins: null,
            resetsAt: null,
          },
          secondary: "unknown",
          credits: { hasCredits: "maybe" },
        },
        extraFutureField: { arbitrary: true },
      }),
    ).toEqual([]);
  });

  it("coerces reset-credit details while preserving unknown-as-null", () => {
    const result = coerceRateLimitsReadResult({
      rateLimitResetCredits: {
        availableCount: 2,
        credits: [
          {
            id: "credit-1",
            resetType: "weekly",
            status: "available",
            grantedAt: 100,
            expiresAt: null,
            title: null,
            description: "Reset once",
          },
          { malformed: true },
        ],
      },
    });

    expect(result.rateLimitResetCredits).toEqual({
      availableCount: 2,
      credits: [
        {
          id: "credit-1",
          resetType: "weekly",
          status: "available",
          grantedAt: 100,
          expiresAt: null,
          title: null,
          description: "Reset once",
        },
      ],
    });
  });

  it("formats arbitrary durations rather than naming lanes as fixed periods", () => {
    expect(formatWindowDurationLabel(45)).toBe("45 分钟");
    expect(formatWindowDurationLabel(120)).toBe("2 小时");
    expect(formatWindowDurationLabel(2_880)).toBe("2 天");
  });
});

describe("account usage coercion", () => {
  it("keeps supported numeric fields and drops malformed daily buckets", () => {
    expect(
      coerceAccountUsageReadResult({
        summary: {
          lifetimeTokens: 10,
          peakDailyTokens: null,
          longestRunningTurnSec: 20,
          currentStreakDays: -1,
          longestStreakDays: 5,
          futureField: "ignored",
        },
        dailyUsageBuckets: [
          { startDate: "2026-08-27", tokens: 8 },
          { startDate: null, tokens: 3 },
        ],
      }),
    ).toEqual({
      summary: {
        lifetimeTokens: 10,
        peakDailyTokens: null,
        longestRunningTurnSec: 20,
        currentStreakDays: null,
        longestStreakDays: 5,
      },
      dailyUsageBuckets: [{ startDate: "2026-08-27", tokens: 8 }],
      threadUsage: null,
    });
  });
});
