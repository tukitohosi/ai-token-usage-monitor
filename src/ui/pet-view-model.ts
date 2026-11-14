import type { DashboardSnapshot } from "../core/types";
import { formatCountdown, formatDateTime, formatExactTokens, formatPercent, formatTokens, getQuotaPresentation, isSnapshotStale, toEpochMilliseconds } from "./format";
import { selectOverviewQuotas } from "./quota-view-model";

export function newestPetSnapshot(current: DashboardSnapshot | null, next: DashboardSnapshot): DashboardSnapshot {
  return current && (next.revision ?? 0) < (current.revision ?? 0) ? current : next;
}

function money(value: number, currency: "USD" | "CNY"): string {
  return new Intl.NumberFormat("zh-CN", { style: "currency", currency, minimumFractionDigits: 2, maximumFractionDigits: 2 }).format(value);
}

export function buildPetUsageView(snapshot: DashboardSnapshot | null, now: number) {
  const usage = snapshot?.deviceUsage;
  const cost = usage?.todayCost;
  const costs = cost ? [cost.usd > 0 ? money(cost.usd, "USD") : null, cost.cny > 0 ? money(cost.cny, "CNY") : null].filter((value): value is string => !!value) : [];
  if (cost && !costs.length && !cost.unpricedTokens) costs.push(money(0, "USD"));
  const costLabel = !cost ? "费用 —" : costs.length ? costs.join(" · ") : "未定价";
  const unpriced = cost?.unpricedTokens ?? 0;
  const quotaTime = snapshot?.accountSync?.quota.lastSuccessfulAt ?? snapshot?.fetchedAt ?? null;
  let stale = !!snapshot?.accountSync?.quota.stale || isSnapshotStale(quotaTime, now);
  const quotas = selectOverviewQuotas(snapshot?.quotaWindows ?? []).map(({ quota, title }) => {
    const presentation = getQuotaPresentation(quota);
    const resetMs = toEpochMilliseconds(quota.resetsAt);
    const expired = resetMs !== null && resetMs <= now;
    stale ||= expired;
    return {
      key: quota.key, title,
      label: title === "Codex 5 小时" ? "5h" : title === "Codex 7 天" ? "7天" : "储备",
      remaining: presentation.remainingPercent,
      remainingLabel: expired ? "待更新" : formatPercent(presentation.remainingPercent), tone: expired ? "normal" : presentation.tone, expired,
      resetLabel: resetMs !== null && resetMs <= now ? "等待更新" : formatCountdown(quota.resetsAt, now),
      resetDate: formatDateTime(quota.resetsAt),
    };
  });
  const status = snapshot?.accountSync?.status ?? snapshot?.status;
  const unavailable = status === "localOnly" ? "纯本地模式 · Codex 额度未同步"
    : status === "unauthenticated" ? "登录 Codex 后可查看套餐额度"
    : status === "offline" || status === "error" ? "Codex 额度暂不可用"
    : "等待 Codex 额度更新";
  const quotaStatus = quotas.length ? (stale ? `旧额度 · ${formatDateTime(quotaTime)} 更新` : "") : unavailable;
  const details = [
    `今日全部来源：${formatExactTokens(usage?.today.totalTokens)}`,
    `今日 API 估算费用：${costLabel}`,
    unpriced ? `其中 ${formatExactTokens(unpriced)} 尚未定价，费用未计全。` : "费用按应用中已保存的价格估算。",
    ...(cost?.longContextUnconfirmedEvents ? [`长上下文档位未确认，${cost.longContextUnconfirmedEvents} 条记录按基础档估算。`] : []),
    `本机用量更新时间：${formatDateTime(usage?.generatedAt ?? null)}`,
    ...quotas.map(quota => `${quota.title}：剩余 ${quota.remainingLabel}；重置 ${quota.resetDate}`),
    quotas.length ? `额度更新时间：${formatDateTime(quotaTime)}${stale ? "（旧数据）" : ""}` : unavailable,
    "点击打开应用额度详情",
  ].join("\n");
  const refreshing = !!snapshot?.refreshProgress && !["idle", "complete"].includes(snapshot.refreshProgress.phase)
    || snapshot?.accountSync?.status === "syncing";
  return { tokensLabel: formatTokens(usage?.today.totalTokens), costLabel, unpriced, quotas, quotaStatus, stale, details, refreshing };
}
