import { useEffect, useMemo, useRef, useState, type FocusEvent } from "react";
import type { DeviceUsageSummary } from "../core/types";
import type { PricingDraft } from "./pricing-editor";

const identity = (model: string) => model.trim().toLowerCase();

export function pricingTokenTotals(usage: DeviceUsageSummary | null) {
  const totals = new Map<string, number>();
  for (const source of usage?.sources ?? []) {
    for (const model of source.byModel) {
      const key = identity(model.label);
      totals.set(key, (totals.get(key) ?? 0) + model.usage.totalTokens);
    }
  }
  return totals;
}

export function rankPricingModels(models: PricingDraft["models"], totals: Map<string, number>) {
  return [...models].sort((left, right) => {
    const count = (totals.get(identity(right.modelId)) ?? 0) - (totals.get(identity(left.modelId)) ?? 0);
    if (count !== 0) return count;
    const a = identity(left.modelId);
    const b = identity(right.modelId);
    return a < b ? -1 : a > b ? 1 : 0;
  });
}

function isModelControl(target: EventTarget | null): target is HTMLElement {
  return target instanceof HTMLElement
    && target.matches("input, select")
    && target.closest(".model-price-card, .peak-model-option") !== null;
}

/** Freeze positions only while a model field is being edited, never the draft itself. */
export function usePricingOrder(models: PricingDraft["models"], usage: DeviceUsageSummary | null) {
  const tokenTotals = useMemo(() => pricingTokenTotals(usage), [usage]);
  const ranked = useMemo(() => rankPricingModels(models, tokenTotals), [models, tokenTotals]);
  const latestIds = useRef<string[]>([]);
  latestIds.current = ranked.map(model => model.modelId);
  const [frozenOrder, setFrozenOrder] = useState<string[] | null>(null);
  const blurTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (blurTimer.current !== null) clearTimeout(blurTimer.current); }, []);

  const onFocusCapture = (event: FocusEvent<HTMLElement>) => {
    if (isModelControl(event.target)) setFrozenOrder(current => current ?? [...latestIds.current]);
  };
  const onBlurCapture = (event: FocusEvent<HTMLElement>) => {
    if (!isModelControl(event.target)) return;
    if (blurTimer.current !== null) clearTimeout(blurTimer.current);
    // Let the pointer click/focus transfer finish before moving its target row.
    blurTimer.current = setTimeout(() => {
      blurTimer.current = null;
      setFrozenOrder(isModelControl(document.activeElement) ? [...latestIds.current] : null);
    }, 0);
  };
  const positions = new Map(frozenOrder?.map((id, index) => [id, index]));
  const displayed = frozenOrder === null ? ranked : [...ranked].sort((a, b) =>
    (positions.get(a.modelId) ?? Infinity) - (positions.get(b.modelId) ?? Infinity));
  return { models: displayed, tokenTotals, onFocusCapture, onBlurCapture };
}
