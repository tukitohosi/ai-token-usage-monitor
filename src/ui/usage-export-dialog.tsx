import { useEffect, useRef, useState } from "react";
import {
  EXPORT_SOURCES, createUsageExportQuery, exportDateKey, exportRangeLabel, serializeUsageExport, usageExportFileName,
  type ExportSourceId, type UsageExportDataset, type UsageExportFormat, type UsageExportPreset, type UsageExportReport,
} from "../core/usage-export";
import type { DashboardAdapter } from "./dashboard-adapter";
import "./usage-export-dialog.css";

const datasets: Array<{ id: UsageExportDataset; title: string; description: string; marker: string }> = [
  { id: "models", title: "模型用量", description: "按应用与模型汇总，查看 Token 分类和估算费用。", marker: "01" },
  { id: "daily", title: "每日趋势", description: "每天、每个应用的用量，便于制作趋势图。", marker: "02" },
  { id: "dailyModels", title: "模型每日用量", description: "对照日期、应用和模型，查看消耗变化。", marker: "03" },
  { id: "sources", title: "应用来源", description: "各应用的总量、记录状态和最近采集时间。", marker: "04" },
  { id: "projects", title: "项目用量", description: "按当前项目合并规则汇总，保留应用来源。", marker: "05" },
  { id: "full", title: "完整报表", description: "包含全部五类数据、总计、日期范围与生成时间。", marker: "ALL" },
];
const presets: Array<{ value: UsageExportPreset; label: string }> = [
  { value: "today", label: "今天" }, { value: "last7Days", label: "近 7 天" }, { value: "last30Days", label: "近 30 天" }, { value: "all", label: "全部历史" }, { value: "custom", label: "自定义日期" },
];

function readableTime(value: string | null): string {
  if (!value) return "暂无记录";
  const date = new Date(value);
  return Number.isFinite(date.getTime()) ? date.toLocaleString("zh-CN", { hour12: false }) : value;
}

export function UsageExportDialog({ adapter, onClose, isDemo = false }: { adapter: DashboardAdapter; onClose: () => void; isDemo?: boolean }) {
  const [preset, setPreset] = useState<UsageExportPreset>("last30Days");
  const [startDate, setStartDate] = useState(() => { const date = new Date(); date.setDate(date.getDate() - 29); return exportDateKey(date); });
  const [endDate, setEndDate] = useState(() => exportDateKey(new Date()));
  const [selectedSources, setSelectedSources] = useState<ExportSourceId[]>(EXPORT_SOURCES.map(source => source.id));
  const [report, setReport] = useState<UsageExportReport | null>(null);
  const [isGenerating, setIsGenerating] = useState(false);
  const [saving, setSaving] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [filtersChanged, setFiltersChanged] = useState(false);
  const dialogRef = useRef<HTMLElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const onCloseRef = useRef(onClose);
  const savingRef = useRef(saving);
  const mounted = useRef(true);
  const inFlight = useRef(false);
  onCloseRef.current = onClose;
  savingRef.current = saving;

  useEffect(() => {
    mounted.current = true;
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    closeRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !savingRef.current) { event.preventDefault(); onCloseRef.current(); return; }
      if (event.key !== "Tab") return;
      const controls = [...(dialogRef.current?.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),select:not(:disabled),[tabindex="0"]') ?? [])];
      const first = controls[0]; const last = controls.at(-1);
      if (!first) { event.preventDefault(); dialogRef.current?.focus(); return; }
      if (event.shiftKey && (document.activeElement === first || !dialogRef.current?.contains(document.activeElement))) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && (document.activeElement === last || !dialogRef.current?.contains(document.activeElement))) { event.preventDefault(); first.focus(); }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => { mounted.current = false; document.removeEventListener("keydown", onKeyDown); document.body.style.overflow = previousOverflow; previousFocus?.focus(); };
  }, []);

  const changeFilters = (action: () => void) => { action(); setFiltersChanged(true); setError(null); setMessage(null); };
  const generate = async () => {
    if (inFlight.current || saving) return;
    setError(null); setMessage(null);
    try {
      const query = createUsageExportQuery(preset, selectedSources, startDate, endDate);
      if (!adapter.readUsageExport) throw new Error("当前环境暂不支持读取导出报表。");
      inFlight.current = true; setIsGenerating(true);
      const next = await adapter.readUsageExport(query);
      if (mounted.current) { setReport(next); setFiltersChanged(false); }
    } catch (error) {
      if (mounted.current) setError(error instanceof Error ? error.message : String(error || "无法生成报表，请稍后重试。"));
    } finally {
      inFlight.current = false;
      if (mounted.current) setIsGenerating(false);
    }
  };
  const save = async (dataset: UsageExportDataset, format: UsageExportFormat) => {
    if (!report || inFlight.current || saving) return;
    if (!adapter.saveUsageExport) { setError("当前环境暂不支持保存导出文件。"); return; }
    inFlight.current = true;
    savingRef.current = `${dataset}-${format}`;
    setSaving(`${dataset}-${format}`); setError(null); setMessage(null);
    try {
      const result = await adapter.saveUsageExport({ format, suggestedFileName: `${isDemo ? "demo-" : ""}${usageExportFileName(report, dataset, format)}`, content: serializeUsageExport(report, dataset, format) });
      if (mounted.current) setMessage(result.status === "cancelled" ? "已取消保存，报表仍可继续导出。" : result.path ? `已保存至 ${result.path}` : "已交给浏览器下载，请查看下载列表。");
    } catch (error) {
      if (mounted.current) setError(error instanceof Error ? error.message : String(error || "保存失败，请重试。"));
    } finally {
      inFlight.current = false;
      savingRef.current = null;
      if (mounted.current) setSaving(null);
    }
  };
  const busy = isGenerating || saving !== null;
  const noData = report?.summary.usage.totalTokens === 0;
  const unavailable = report?.sources.filter(source => source.indexReadStatus === "notApplicable" || ["unavailable", "error", "unknown"].includes(source.status)) ?? [];

  return (
    <div className="usage-export-backdrop" role="presentation" onMouseDown={event => { if (event.target === event.currentTarget && !saving) onClose(); }}>
      <section className="usage-export-dialog" ref={dialogRef} role="dialog" aria-modal="true" aria-labelledby="usage-export-title" aria-describedby="usage-export-description" tabIndex={-1}>
        <header className="usage-export-header">
          <div><p className="eyebrow">用量数据 · 本机导出</p><h2 id="usage-export-title">导出报表</h2><p id="usage-export-description">选好日期与应用，生成一份可反复导出的报表。</p>{isDemo && <span className="usage-export-demo" role="note">演示报表 · 数字为示例</span>}</div>
          <button ref={closeRef} type="button" className="usage-export-close" aria-label="关闭导出报表" disabled={saving !== null} onClick={onClose}>×</button>
        </header>
        <fieldset className="usage-export-filters" disabled={busy}>
          <legend className="usage-export-sr-only">报表范围</legend>
          <div className="usage-export-filter-row"><label htmlFor="usage-export-preset">日期范围</label><select id="usage-export-preset" value={preset} onChange={event => changeFilters(() => setPreset(event.target.value as UsageExportPreset))}>{presets.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}</select></div>
          {preset === "custom" && <div className="usage-export-dates"><label>开始日期<input type="date" value={startDate} onChange={event => changeFilters(() => setStartDate(event.target.value))} /></label><span aria-hidden="true">—</span><label>结束日期<input type="date" value={endDate} onChange={event => changeFilters(() => setEndDate(event.target.value))} /></label></div>}
          <div className="usage-export-source-heading"><span>应用来源</span><button type="button" onClick={() => changeFilters(() => setSelectedSources(selectedSources.length === EXPORT_SOURCES.length ? [] : EXPORT_SOURCES.map(source => source.id)))}>{selectedSources.length === EXPORT_SOURCES.length ? "取消全选" : "选择全部"}</button></div>
          <div className="usage-export-sources">{EXPORT_SOURCES.map(source => <label key={source.id} className={selectedSources.includes(source.id) ? "is-selected" : ""}><input type="checkbox" checked={selectedSources.includes(source.id)} onChange={event => changeFilters(() => setSelectedSources(current => event.target.checked ? [...current, source.id] : current.filter(id => id !== source.id)))} /><span>{source.label}</span></label>)}</div>
        </fieldset>
        <div className="usage-export-generate"><p>读取已采集的本机记录，不触发新的扫描。{preset === "all" ? "无日期记录也会计入全部历史。" : "按系统本地日期统计，不限制自定义区间长度。"}</p><button className="primary-button" type="button" disabled={busy || !selectedSources.length} onClick={() => void generate()}>{isGenerating ? "正在生成…" : report ? "重新生成报表" : "生成报表"}</button></div>
        {error && <p className="usage-export-error" role="alert">{error}</p>}
        {message && <p className="usage-export-message" role="status">{message}</p>}
        {report ? <>
          <section className="usage-export-summary" aria-label="已生成报表">
            <div className="usage-export-summary-heading"><strong>{exportRangeLabel(report.query.range)}</strong><span>{report.query.sourceIds.length} 个应用来源</span></div>
            <div className="usage-export-stats"><div><span>已记录 Token</span><strong>{report.summary.usage.totalTokens.toLocaleString("zh-CN")}</strong></div><div><span>模型记录</span><strong>{report.models.length}<small> 项</small></strong></div><div><span>项目</span><strong>{report.projects.length}<small> 个</small></strong></div></div>
            <p>生成于 {readableTime(report.generatedAt)} · 最近已知采集 {readableTime(report.lastCollectionAt)}</p>
          </section>
          {filtersChanged && <p className="usage-export-notice" role="status">范围已更改。下方仍导出上面这份报表，点击“重新生成报表”应用新范围。</p>}
          {noData && <p className="usage-export-notice">所选范围内没有已记录用量，仍可导出空报表和来源状态。</p>}
          {unavailable.length > 0 && <p className="usage-export-notice">{unavailable.map(source => source.sourceLabel).join("、")}的采集状态不可用或未确认。数值仅代表已记录用量，不能据此判断实际用量为零。</p>}
          <div className="usage-export-datasets">{datasets.map(dataset => <article key={dataset.id} className={dataset.id === "full" ? "usage-export-card usage-export-card--full" : "usage-export-card"}><div className="usage-export-card-heading"><span>{dataset.marker}</span><h3>{dataset.title}</h3>{dataset.id !== "full" && <small>{report[dataset.id].length.toLocaleString("zh-CN")} 行</small>}</div><p>{dataset.description}</p><div className="usage-export-buttons">{(dataset.id === "full" ? ["json"] : ["csv", "json"] as const).map(format => <button key={format} type="button" className="secondary-button" disabled={busy} aria-label={`导出${dataset.title} ${format.toUpperCase()}`} onClick={() => void save(dataset.id, format as UsageExportFormat)}>{saving === `${dataset.id}-${format}` ? "正在保存…" : `导出 ${format.toUpperCase()}`}</button>)}</div></article>)}</div>
          <footer className="usage-export-footnote">CSV 可用于 Excel，JSON 适合归档和进一步分析。Token 保留原始整数，USD 与 CNY 分列；未定价用量单独保留。报表不含对话内容、账号信息或本机完整路径。</footer>
        </> : <div className="usage-export-empty"><span aria-hidden="true">↗</span><strong>把用量留成一份报表</strong><p>模型、日期、应用与项目共五类数据，支持单独导出或保存完整 JSON。</p></div>}
      </section>
    </div>
  );
}
