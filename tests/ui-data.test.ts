import { describe, expect, it } from "vitest";

import {
  clampPercent,
  formatCountdown,
  formatChineseCompactTokens,
  formatRefreshCountdown,
  getQuotaPresentation,
  isSnapshotStale,
  toEpochMilliseconds,
} from "../src/ui/format.js";
import { createMockDashboardSnapshot } from "../src/ui/mock-data.js";
import { selectOverviewQuotas } from "../src/ui/quota-view-model.js";

describe("UI data safety and formatting", () => {
  it("formats trend tooltips with only rounded 万/亿 units", () => {
    expect(formatChineseCompactTokens(9_999)).toBe("9999");
    expect(formatChineseCompactTokens(10_000)).toBe("1.00万");
    expect(formatChineseCompactTokens(67_777)).toBe("6.78万");
    expect(formatChineseCompactTokens(677_777)).toBe("67.78万");
    expect(formatChineseCompactTokens(6_777_777)).toBe("677.78万");
    expect(formatChineseCompactTokens(67_777_777)).toBe("6777.78万");
    expect(formatChineseCompactTokens(238_011_617)).toBe("2.38亿");
    expect(formatChineseCompactTokens(99_999_999)).toBe("1.00亿");
    expect(formatChineseCompactTokens(100_000_000)).toBe("1.00亿");
    expect(formatChineseCompactTokens(12_345_678)).not.toMatch(/百万|千万|十亿/);
  });
  it("formats the backend refresh deadline without inventing a schedule", () => {
    expect(formatRefreshCountdown(null, false, 1_000)).toBe("等待首次更新");
    expect(formatRefreshCountdown(6_500, false, 1_000)).toBe("约 6 秒后自动更新");
    expect(formatRefreshCountdown(1_000, false, 1_000)).toBe("即将更新");
    expect(formatRefreshCountdown(6_500, true, 1_000)).toBe("正在更新");
  });
  it("marks every ready mock snapshot as non-real demo data", () => {
    const snapshot = createMockDashboardSnapshot("ready");

    expect(snapshot.message).toContain("演示模式");
    expect(snapshot.message).toContain("不是您的真实");
    expect(snapshot.quotaWindows).toHaveLength(3);
    expect(snapshot.quotaWindows.some((quota) => quota.limitName === "弹性调用")).toBe(false);
    expect(snapshot.planRenewalAt).toBeNull();
    expect(snapshot.planRenewalSource).toBeNull();
  });

  it("selects Codex and Luna quota cards by identity regardless of bucket order", () => {
    const snapshot = createMockDashboardSnapshot("ready");
    const unknown = {
      ...snapshot.quotaWindows[0],
      key: "future:primary",
      limitId: "future",
      limitName: "Future quota",
      normalModelSlug: null,
    };
    const shuffled = [
      snapshot.quotaWindows[2],
      unknown,
      snapshot.quotaWindows[0],
      snapshot.quotaWindows[1],
    ];

    const selected = selectOverviewQuotas(shuffled);
    expect(selected.map(({ title, quota }) => [title, quota.key])).toEqual([
      ["Codex 5 小时", "codex-primary"],
      ["Codex 7 天", "codex-secondary"],
      ["GPT-5.6 Luna 储备 7 天", "base_model_inference:primary"],
    ]);
    expect(selected[1].quota.remainingPercent).toBe(61.5);
    expect(selected[2].quota.remainingPercent).toBe(0);
  });

  it("does not invent missing quota cards and tolerates partial Luna identity", () => {
    const snapshot = createMockDashboardSnapshot("ready");
    const reserve = {
      ...snapshot.quotaWindows[0],
      limitId: "future-id",
      limitName: null,
      normalModelSlug: "gpt-5.6-luna",
    };
    expect(selectOverviewQuotas([reserve]).map((item) => item.title)).toEqual([
      "GPT-5.6 Luna 储备 7 天",
    ]);
    expect(selectOverviewQuotas([])).toEqual([]);
  });

  it("presents remaining quota as primary and warns only when little remains", () => {
    expect(getQuotaPresentation({ usedPercent: 64, remainingPercent: 36 })).toEqual({
      usedPercent: 64,
      remainingPercent: 36,
      tone: "normal",
    });
    expect(getQuotaPresentation({ usedPercent: 76, remainingPercent: 24 }).tone).toBe("warning");
    expect(getQuotaPresentation({ usedPercent: 91, remainingPercent: 9 }).tone).toBe("critical");
  });

  it("supports both Unix seconds and epoch milliseconds", () => {
    expect(toEpochMilliseconds(1_800_000_000)).toBe(1_800_000_000_000);
    expect(toEpochMilliseconds(1_800_000_000_000)).toBe(1_800_000_000_000);
    expect(toEpochMilliseconds(0)).toBeNull();
  });

  it("formats a deterministic countdown", () => {
    const now = Date.parse("2026-08-27T00:00:00.000Z");
    expect(formatCountdown(now + 90 * 60_000, now)).toBe("1 小时 30 分后");
  });

  it("clamps percentages and only marks snapshots stale after five minutes", () => {
    expect(clampPercent(-1)).toBe(0);
    expect(clampPercent(101)).toBe(100);
    const now = Date.parse("2026-08-27T00:10:00.000Z");
    expect(isSnapshotStale("2026-08-27T00:06:00.000Z", now)).toBe(false);
    expect(isSnapshotStale("2026-08-27T00:04:59.000Z", now)).toBe(true);
  });
});
