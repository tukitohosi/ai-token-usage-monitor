import { useEffect, useLayoutEffect, useRef, useState, type PointerEvent } from "react";
import { DEFAULT_PET_STATE, type PetLayout, type PetState } from "../core/pet-types";
import type { DashboardSnapshot, ThemePreference } from "../core/types";
import { dashboardAdapter, type DashboardAdapter } from "./dashboard-adapter";
import { petApi, petAssetUrl, type PetApi } from "./pet-api";
import { PetSprite, type PetAnimation } from "./pet-sprite";
import { buildPetUsageView, newestPetSnapshot } from "./pet-view-model";
import "./pet-window.css";

export function PetWindow({ api = petApi, adapter = dashboardAdapter }: { api?: PetApi; adapter?: DashboardAdapter }) {
  const [state, setState] = useState<PetState>(DEFAULT_PET_STATE);
  const [snapshot, setSnapshot] = useState<DashboardSnapshot | null>(null);
  const [themePreference, setThemePreference] = useState<ThemePreference>("system");
  const [systemDark, setSystemDark] = useState(() => window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? true);
  const [now, setNow] = useState(Date.now);
  const [hover, setHover] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const rootRef = useRef<HTMLElement>(null);
  const spriteRef = useRef<HTMLDivElement>(null);
  const barRef = useRef<HTMLButtonElement>(null);
  const lastLayout = useRef("");
  const drag = useRef<{ pointer: number; ready: Promise<void>; frame: number | null; finishing: boolean } | null>(null);

  useEffect(() => {
    document.documentElement.classList.add("pet-window-document");
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    const media = window.matchMedia?.("(prefers-color-scheme: dark)");
    const update = () => setSystemDark(media?.matches ?? true);
    media?.addEventListener("change", update);
    return () => { clearInterval(timer); media?.removeEventListener("change", update); document.documentElement.classList.remove("pet-window-document"); };
  }, []);
  useEffect(() => {
    let cancelled = false;
    const stops: Array<() => void> = [];
    let receivedPetState = false;
    let receivedTheme = false;
    const retain = (stop: () => void) => { if (cancelled) stop(); else stops.push(stop); };
    void (async () => {
      try {
        retain(await api.subscribe(next => { receivedPetState = true; if (!cancelled) setState(next); }));
        if (cancelled) return;
        const initial = await api.readState();
        if (!cancelled && !receivedPetState) setState(initial);
        retain(await api.subscribeAppTheme(next => { receivedTheme = true; if (!cancelled) setThemePreference(next); }));
        const theme = await api.readAppTheme();
        if (!cancelled && !receivedTheme) setThemePreference(theme);
      } catch (reason) { if (!cancelled) setError(String(reason instanceof Error ? reason.message : reason)); }
    })();
    void (async () => {
      try {
        if (adapter.subscribe) retain(await adapter.subscribe(next => { if (!cancelled) setSnapshot(current => newestPetSnapshot(current, next)); }));
        if (cancelled) return;
        const cached = await adapter.readCachedSnapshot?.();
        if (cached && !cancelled) setSnapshot(current => newestPetSnapshot(current, cached));
      } catch (reason) { if (!cancelled) setError(String(reason instanceof Error ? reason.message : reason)); }
    })();
    return () => { cancelled = true; stops.forEach(stop => stop()); };
  }, [api, adapter]);

  const view = buildPetUsageView(snapshot, now);
  const selectedPet = state.pets.find(pet => pet.id === state.preferences.selectedPetId);
  const appTheme = themePreference === "system" ? (systemDark ? "dark" : "light") : themePreference;
  const theme = state.preferences.theme === "app" ? appTheme : state.preferences.theme;
  const animation: PetAnimation = hover ? "wave" : view.refreshing ? "working" : "idle";
  useLayoutEffect(() => {
    const measure = () => {
      const root = rootRef.current?.getBoundingClientRect();
      const sprite = spriteRef.current?.getBoundingClientRect();
      const bar = barRef.current?.getBoundingClientRect();
      if (!root || !sprite || !bar || !root.width || !root.height) return;
      const rect = (box: DOMRect) => ({ x: box.x - root.x, y: box.y - root.y, width: box.width, height: box.height });
      const layout: PetLayout = { width: root.width, height: root.height, sprite: rect(sprite), bar: rect(bar) };
      const key = JSON.stringify(layout);
      if (lastLayout.current === key) return;
      lastLayout.current = key;
      void api.setLayout(layout).catch(reason => { setError(String(reason instanceof Error ? reason.message : reason)); });
    };
    measure();
    if (!window.ResizeObserver || !rootRef.current) return;
    const observer = new ResizeObserver(measure);
    observer.observe(rootRef.current);
    return () => observer.disconnect();
  }, [api, state.preferences.size, selectedPet?.id, view.quotas.length, view.quotaStatus, error]);

  const failDrag = (reason: unknown) => setError(String(reason instanceof Error ? reason.message : reason));
  const startDrag = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || drag.current) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    const ready = api.dragStart(event.clientX, event.clientY);
    drag.current = { pointer: event.pointerId, ready, frame: null, finishing: false };
    void ready.catch(failDrag);
  };
  const updateDrag = (event: PointerEvent<HTMLDivElement>) => {
    const current = drag.current;
    if (!current || current.pointer !== event.pointerId || current.finishing || current.frame !== null) return;
    current.frame = requestAnimationFrame(() => {
      current.frame = null;
      if (!current.finishing) void current.ready.then(() => api.dragUpdate()).catch(failDrag);
    });
  };
  const endDrag = (event: PointerEvent<HTMLDivElement>) => {
    const current = drag.current;
    if (!current || current.pointer !== event.pointerId || current.finishing) return;
    current.finishing = true;
    if (current.frame !== null) cancelAnimationFrame(current.frame);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    void current.ready.then(() => api.dragUpdate()).then(() => api.dragEnd()).catch(failDrag).finally(() => { if (drag.current === current) drag.current = null; });
  };

  return <main ref={rootRef} className="pet-window" data-pet-theme={theme} aria-label="AI Token 桌面宠物">
    <div ref={spriteRef} className="pet-window__sprite-handle" title="拖动宠物可移动宠物和用量条" onPointerDown={startDrag} onPointerMove={updateDrag} onPointerUp={endDrag} onPointerCancel={endDrag} onLostPointerCapture={endDrag} onPointerEnter={() => setHover(true)} onPointerLeave={() => setHover(false)}>
      {selectedPet ? <PetSprite url={petAssetUrl(selectedPet.spritesheetPath)} version={selectedPet.spriteVersionNumber} size={state.preferences.size} animation={animation} name={selectedPet.displayName} /> : <div className="pet-window__loading-sprite" style={{ width: state.preferences.size, height: state.preferences.size * 208 / 192 }} />}
    </div>
    <button ref={barRef} type="button" className="pet-usage-bar" aria-label="打开应用额度详情" title={view.details} onClick={() => void api.showDashboard().catch(failDrag)}>
      <span className="pet-usage-bar__totals"><span className="pet-usage-bar__tokens">今日 <strong>{view.tokensLabel}</strong> Token</span><span className="pet-usage-bar__cost">{view.unpriced && view.costLabel !== "未定价" ? "≥ " : ""}{view.costLabel}</span></span>
      {view.quotas.map(quota => <span className="pet-usage-bar__quota" key={quota.key} data-tone={quota.tone} aria-label={`${quota.title} 剩余 ${quota.remainingLabel}，${quota.resetLabel}`}>
        <span className="pet-usage-bar__quota-label">{quota.label}</span><span className="pet-usage-bar__remaining">剩余 {quota.remainingLabel}</span><span className="pet-usage-bar__track"><span style={{ width: `${quota.remaining}%` }} /></span><span className="pet-usage-bar__reset">{quota.resetLabel}</span>
      </span>)}
      {(error || view.quotaStatus) && <span className="pet-usage-bar__status" data-stale={view.stale || undefined}>{error || view.quotaStatus}</span>}
    </button>
  </main>;
}
