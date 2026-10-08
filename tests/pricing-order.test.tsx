// @vitest-environment jsdom
import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import type { DeviceUsageSummary, PricingSettings } from "../src/core/types";
import { createPricingDraft } from "../src/ui/pricing-editor";
import { PricingView } from "../src/ui/dashboard-views";
import { pricingTokenTotals, rankPricingModels } from "../src/ui/pricing-order";
import { createMockDashboardSnapshot } from "../src/ui/mock-data";

afterEach(cleanup);
const settings: PricingSettings = {
  updatedAt: null,
  peak: { windows: [], multiplier: 1.5, timeZone: "local", excludeChinaHolidays: false, specialDates: [] },
  models: ["beta", "alpha", "gamma"].map(modelId => ({ modelId, displayName: modelId, currency: "USD", inputPerMillion: 1, cachedInputPerMillion: 0.1, cacheWritePerMillion: null, outputPerMillion: 2, peakEnabled: false })),
};

function usage(alpha: number, beta: number): DeviceUsageSummary {
  const result = createMockDashboardSnapshot("ready").deviceUsage!;
  const base = result.sources[0]!.byModel[0]!;
  result.sources = result.sources.slice(0, 2).map((source, index) => ({ ...source, byModel: index === 0 ? [
    { ...base, key: "alpha", label: "alpha", usage: { ...base.usage, totalTokens: alpha } },
    { ...base, key: "beta", label: "beta", usage: { ...base.usage, totalTokens: beta } },
  ] : [] }));
  return result;
}
function Harness({ value }: { value: DeviceUsageSummary | null }) {
  const [draft, setDraft] = useState(() => createPricingDraft(settings));
  return <PricingView usage={value} draft={draft} setDraft={setDraft} dirty={false} onDiscard={() => setDraft(createPricingDraft(settings))} onRecalculate={null} isSaving={false} message={null} onSave={async () => {}} />;
}
const order = () => [...document.querySelectorAll(".model-price-card h2")].map(node => node.textContent);
const peakOrder = () => [...document.querySelectorAll(".peak-model-option strong")].map(node => node.textContent);

describe("pricing display ordering", () => {
  it("uses full-history cross-source totals and a deterministic tie break without changing drafts", () => {
    const value = usage(60, 100);
    value.sources[1]!.byModel = [{ ...value.sources[0]!.byModel[0]!, label: " ALPHA ", usage: { ...value.sources[0]!.byModel[0]!.usage, totalTokens: 50 } }];
    const draft = createPricingDraft(settings);
    expect(rankPricingModels(draft.models, pricingTokenTotals(value)).map(model => model.modelId)).toEqual(["alpha", "beta", "gamma"]);
    expect(draft.models.map(model => model.modelId)).toEqual(["beta", "alpha", "gamma"]);
    expect(rankPricingModels(draft.models, pricingTokenTotals(usage(100, 100))).map(model => model.modelId)).toEqual(["alpha", "beta", "gamma"]);
  });

  it("corrects a cached or initially missing total on every update and reopening", () => {
    const mounted = render(<Harness value={null} />);
    expect(order()).toEqual(["alpha", "beta", "gamma"]);
    mounted.rerender(<Harness value={usage(10, 500)} />);
    expect(order()).toEqual(["beta", "alpha", "gamma"]);
    mounted.rerender(<Harness value={usage(900, 500)} />);
    expect(order()).toEqual(["alpha", "beta", "gamma"]);
    expect(peakOrder()).toEqual(order());
    expect(screen.getByText("价格已保存")).toBeTruthy();
    mounted.unmount();
    render(<Harness value={usage(10, 500)} />);
    expect(order()).toEqual(["beta", "alpha", "gamma"]);
  });

  it("updates counts but postpones moves until blur, preserving the text and next click target", async () => {
    const user = userEvent.setup();
    const mounted = render(<Harness value={usage(100, 50)} />);
    const input = screen.getByLabelText("alpha 新输入") as HTMLInputElement;
    await user.click(input);
    fireEvent.change(input, { target: { value: "12." } });
    input.setSelectionRange(2, 2);
    mounted.rerender(<Harness value={usage(100, 500)} />);
    expect(order()).toEqual(["alpha", "beta", "gamma"]);
    expect(document.activeElement).toBe(input);
    expect(input.value).toBe("12.");
    expect(input.selectionStart).toBe(2);
    expect(screen.getByText("500 Token · 累计消耗")).toBeTruthy();
    const nextInput = screen.getByLabelText("beta 新输入") as HTMLInputElement;
    await user.click(nextInput);
    await waitFor(() => expect(order()).toEqual(["beta", "alpha", "gamma"]));
    expect(document.activeElement).toBe(nextInput);
    expect(screen.getByLabelText("alpha 新输入")).toBe(input);
    expect(input.value).toBe("12.");
    mounted.rerender(<Harness value={usage(1000, 500)} />);
    expect(order()).toEqual(["beta", "alpha", "gamma"]);
    await user.click(screen.getByText("模型定价", { selector: "h1" }));
    await waitFor(() => expect(order()).toEqual(["alpha", "beta", "gamma"]));
    expect(peakOrder()).toEqual(order());
  });

  it("does not lose focus or edits when an added model outranks existing models", async () => {
    const value = usage(100, 50);
    const mounted = render(<Harness value={value} />);
    const input = screen.getByLabelText("alpha 新输入") as HTMLInputElement;
    act(() => input.focus());
    const next = usage(100, 50);
    next.sources[0]!.byModel.push({ ...next.sources[0]!.byModel[0]!, label: "gamma", usage: { ...next.sources[0]!.byModel[0]!.usage, totalTokens: 300 } });
    mounted.rerender(<Harness value={next} />);
    expect(order()).toEqual(["alpha", "beta", "gamma"]);
    act(() => input.blur());
    await waitFor(() => expect(order()).toEqual(["gamma", "alpha", "beta"]));
  });
});
