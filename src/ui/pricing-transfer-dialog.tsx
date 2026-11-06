import { useEffect, useRef, useState } from "react";
import { MAX_PRICING_MODELS, pricingImportSummary, type PricingTransferDocumentV1 } from "../core/pricing-transfer";
import type { PricingDraft } from "./pricing-editor";
import "./pricing-transfer-dialog.css";

export function PricingTransferDialog({ document, draft, onConfirm, onClose }: {
  document: PricingTransferDocumentV1;
  draft: PricingDraft;
  onConfirm: (includePeak: boolean) => void;
  onClose: () => void;
}) {
  const [includePeak, setIncludePeak] = useState(false);
  const dialog = useRef<HTMLElement>(null);
  const close = useRef<HTMLButtonElement>(null);
  const onCloseRef = useRef(onClose); onCloseRef.current = onClose;
  const summary = pricingImportSummary(draft, document);
  const overLimit = summary.total > MAX_PRICING_MODELS;
  useEffect(() => {
    const previousFocus = window.document.activeElement instanceof HTMLElement ? window.document.activeElement : null;
    const previousOverflow = window.document.body.style.overflow;
    window.document.body.style.overflow = "hidden";
    close.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") { event.preventDefault(); onCloseRef.current(); return; }
      if (event.key !== "Tab") return;
      const controls = [...(dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),[tabindex="0"]') ?? [])];
      const first = controls[0]; const last = controls.at(-1);
      if (!first) { event.preventDefault(); dialog.current?.focus(); return; }
      if (event.shiftKey && (window.document.activeElement === first || !dialog.current?.contains(window.document.activeElement))) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && (window.document.activeElement === last || !dialog.current?.contains(window.document.activeElement))) { event.preventDefault(); first.focus(); }
    };
    window.document.addEventListener("keydown", onKey);
    return () => { window.document.removeEventListener("keydown", onKey); window.document.body.style.overflow = previousOverflow; previousFocus?.focus(); };
  }, []);
  return <div className="pricing-transfer-backdrop" onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}>
    <section ref={dialog} className="pricing-transfer-dialog" role="dialog" aria-modal="true" aria-labelledby="pricing-transfer-title" aria-describedby="pricing-transfer-description" tabIndex={-1}>
      <header><div><p className="eyebrow">MODEL PRICING</p><h2 id="pricing-transfer-title">导入模型定价</h2></div><button ref={close} type="button" className="pricing-transfer-close" aria-label="关闭定价导入" onClick={onClose}>×</button></header>
      <p id="pricing-transfer-description">合并到当前编辑内容，同名模型使用文件中的价格；其他本地模型保留。导入后仍需点击“保存价格并重算”才会生效。</p>
      <div className="pricing-transfer-stats" aria-label="导入变更"><div><span>新增模型</span><strong>{summary.added}</strong></div><div><span>覆盖模型</span><strong>{summary.overwritten}</strong></div><div><span>合并后模型</span><strong>{summary.total}<small> / 200</small></strong></div></div>
      <p className="pricing-transfer-meta">文件导出时间：{new Date(document.exportedAt).toLocaleString("zh-CN", { hour12: false })} · 单价单位：每百万 Token</p>
      <label className="pricing-transfer-peak"><input type="checkbox" checked={includePeak} onChange={event => setIncludePeak(event.target.checked)} /><span><strong>同时导入高峰规则</strong><small>包含时段、倍率、时区、特殊日期和导入模型的高峰开关。</small></span></label>
      {includePeak ? <p className="pricing-transfer-warning" role="note">文件中的高峰规则将替换当前规则，也会影响本地其他已开启高峰计费的模型。尚未加入列表的特殊日期会清空。</p> : <p className="pricing-transfer-hint">保留本地高峰规则及已有模型的高峰开关；新增模型默认关闭高峰计费。</p>}
      {overLimit && <p className="pricing-transfer-warning" role="alert">合并后超过 200 个模型上限，不能导入；当前编辑内容未改变。</p>}
      <footer><button type="button" className="secondary-button" onClick={onClose}>取消</button><button type="button" className="primary-button" disabled={overLimit} onClick={() => onConfirm(includePeak)}>导入到编辑页</button></footer>
    </section>
  </div>;
}
