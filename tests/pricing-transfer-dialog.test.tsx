// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PricingTransferDialog } from "../src/ui/pricing-transfer-dialog";
import { createPricingDraft } from "../src/ui/pricing-editor";
import type { PricingTransferDocumentV1 } from "../src/core/pricing-transfer";

afterEach(cleanup);
const document: PricingTransferDocumentV1 = { format: "ai-token-pricing", version: 1, unit: "perMillionTokens", exportedAt: "2026-11-07T10:00:00Z", peak: { windows: [], multiplier: 2, timeZone: "local", excludeChinaHolidays: false, specialDates: [] }, models: [{ modelId: "new", displayName: "新模型", currency: "USD", inputPerMillion: 0, cachedInputPerMillion: null, cacheWritePerMillion: null, outputPerMillion: 2, peakEnabled: true }] };
const draft = () => createPricingDraft({ updatedAt: null, peak: document.peak, models: [] });

describe("pricing transfer confirmation", () => {
  it("defaults to base prices and requires explicit confirmation", () => {
    const confirm = vi.fn();
    render(<PricingTransferDialog document={document} draft={draft()} onConfirm={confirm} onClose={vi.fn()} />);
    expect(confirm).not.toHaveBeenCalled();
    expect((screen.getByRole("checkbox") as HTMLInputElement).checked).toBe(false);
    expect(screen.getByText("新增模型")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "导入到编辑页" }));
    expect(confirm).toHaveBeenCalledWith(false);
  });
  it("warns about global peak effects and sends the explicit selection", () => {
    const confirm = vi.fn();
    render(<PricingTransferDialog document={document} draft={draft()} onConfirm={confirm} onClose={vi.fn()} />);
    fireEvent.click(screen.getByRole("checkbox"));
    expect(screen.getByRole("note").textContent).toMatch(/其他已开启高峰/);
    fireEvent.click(screen.getByRole("button", { name: "导入到编辑页" }));
    expect(confirm).toHaveBeenCalledWith(true);
  });
  it("updates counts when the live draft changes and blocks an oversized merge", () => {
    const current = draft();
    const confirm = vi.fn();
    const { rerender } = render(<PricingTransferDialog document={document} draft={current} onConfirm={confirm} onClose={vi.fn()} />);
    current.models = Array.from({ length: 200 }, (_, index) => ({ ...createPricingDraft({ updatedAt: null, peak: document.peak, models: document.models }).models[0]!, modelId: `existing-${index}` }));
    rerender(<PricingTransferDialog document={document} draft={current} onConfirm={confirm} onClose={vi.fn()} />);
    expect(screen.getByRole("alert").textContent).toMatch(/超过 200/);
    expect((screen.getByRole("button", { name: "导入到编辑页" }) as HTMLButtonElement).disabled).toBe(true);
  });
  it("cancels without confirming and restores body overflow", () => {
    const close = vi.fn(); const confirm = vi.fn();
    const { unmount } = render(<PricingTransferDialog document={document} draft={draft()} onConfirm={confirm} onClose={close} />);
    fireEvent.keyDown(window.document, { key: "Escape" });
    expect(close).toHaveBeenCalledOnce(); expect(confirm).not.toHaveBeenCalled();
    unmount(); expect(window.document.body.style.overflow).toBe("");
  });
});
