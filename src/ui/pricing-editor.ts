import { useEffect, useRef, useState } from "react";
import type { DeviceUsageSummary, ModelPricingRule, PricingSettings } from "../core/types";

const rateFields = ["inputPerMillion", "cachedInputPerMillion", "cacheWritePerMillion", "outputPerMillion"] as const;
type RateField = typeof rateFields[number];
export type PricingDraft = Omit<PricingSettings, "models" | "peak"> & {
  models: Array<Omit<ModelPricingRule, RateField> & Record<RateField, string>>;
  peak: Omit<PricingSettings["peak"], "multiplier"> & { multiplier: string };
};

export function createPricingDraft(settings: PricingSettings): PricingDraft {
  return { ...settings, peak: { ...settings.peak, multiplier: String(settings.peak.multiplier) }, models: settings.models.map(model => ({
    ...model, inputPerMillion: model.inputPerMillion === null ? "" : String(model.inputPerMillion), cachedInputPerMillion: model.cachedInputPerMillion === null ? "" : String(model.cachedInputPerMillion), cacheWritePerMillion: model.cacheWritePerMillion === null ? "" : String(model.cacheWritePerMillion), outputPerMillion: model.outputPerMillion === null ? "" : String(model.outputPerMillion),
  })) };
}

function decimal(value: string, optional: boolean): number | null {
  const text = value.trim();
  if (optional && text === "") return null;
  if (!/^(?:\d+(?:\.\d*)?|\.\d+)$/.test(text) || !Number.isFinite(Number(text))) throw new Error("单价请输入有效的非负数字；留空表示未定价。");
  return Number(text);
}

export function parsePricingDraft(draft: PricingDraft): PricingSettings {
  const multiplier = decimal(draft.peak.multiplier, false)!;
  if (multiplier < 1 || multiplier > 100) throw new Error("高峰倍率必须在 1 到 100 之间。");
  if (![draft.peak.startTime, draft.peak.endTime].every(time => /^(?:[01]\d|2[0-3]):[0-5]\d$/.test(time))) throw new Error("请输入完整的高峰开始和结束时间。");
  return { ...draft, peak: { ...draft.peak, multiplier }, models: draft.models.map(model => ({
    ...model, inputPerMillion: decimal(model.inputPerMillion, true), cachedInputPerMillion: decimal(model.cachedInputPerMillion, true), cacheWritePerMillion: decimal(model.cacheWritePerMillion, true), outputPerMillion: decimal(model.outputPerMillion, true),
  })) };
}

export function appendUsedModels(draft: PricingDraft, usage: DeviceUsageSummary | null | undefined): PricingDraft {
  const totals = new Map<string, { label: string; tokens: number }>();
  for (const source of usage?.sources ?? []) for (const model of source.byModel) {
    const key = model.label.trim().toLowerCase();
    if (key) totals.set(key, { label: model.label, tokens: (totals.get(key)?.tokens ?? 0) + model.usage.totalTokens });
  }
  const ids = new Set(draft.models.map(model => model.modelId.trim().toLowerCase()));
  const added = [...totals].filter(([key]) => !ids.has(key)).sort((a, b) => b[1].tokens - a[1].tokens).map(([, { label }]) => ({
    modelId: label, displayName: label, currency: "USD" as const, peakEnabled: false,
    inputPerMillion: "", cachedInputPerMillion: "", cacheWritePerMillion: "", outputPerMillion: "",
  }));
  return added.length ? { ...draft, models: [...draft.models, ...added] } : draft;
}

function initialDraft(settings: PricingSettings, usage: DeviceUsageSummary | null | undefined): PricingDraft {
  const draft = appendUsedModels(createPricingDraft(settings), usage);
  const totals = new Map<string, number>();
  for (const source of usage?.sources ?? []) for (const model of source.byModel) {
    const id = model.label.trim().toLowerCase();
    totals.set(id, (totals.get(id) ?? 0) + model.usage.totalTokens);
  }
  draft.models.sort((a, b) => (totals.get(b.modelId.trim().toLowerCase()) ?? 0) - (totals.get(a.modelId.trim().toLowerCase()) ?? 0));
  return draft;
}

function comparable(draft: PricingDraft): string {
  return JSON.stringify({ peak: draft.peak, models: [...draft.models].sort((a, b) => a.modelId.localeCompare(b.modelId)) });
}

export function usePricingEditor(settings: PricingSettings, usage: DeviceUsageSummary | null | undefined, loaded: boolean) {
  const [draft, setDraft] = useState<PricingDraft | null>(null);
  const revision = useRef(0);
  useEffect(() => {
    if (loaded) setDraft(current => current ? appendUsedModels(current, usage) : initialDraft(settings, usage));
  }, [loaded, settings, usage]);
  const update = (action: (current: PricingDraft) => PricingDraft) => {
    revision.current += 1;
    setDraft(current => current ? action(current) : current);
  };
  const reset = () => {
    revision.current += 1;
    setDraft(appendUsedModels(createPricingDraft(settings), usage));
  };
  const acceptSaved = (saved: PricingSettings, submittedRevision: number) => {
    if (revision.current === submittedRevision) setDraft(appendUsedModels(createPricingDraft(saved), usage));
  };
  const baseline = appendUsedModels(createPricingDraft(settings), usage);
  const dirty = draft !== null && comparable(draft) !== comparable(baseline);
  return { draft, update, reset, acceptSaved, dirty, revision };
}
