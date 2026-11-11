import { useEffect, useLayoutEffect, useRef, useState, type PointerEvent } from "react";
import { DEFAULT_PET_STATE, type PetLayout, type PetState } from "../core/pet-types";
import type { DashboardSnapshot, RefreshSchedule, ThemePreference } from "../core/types";
import { dashboardAdapter, type DashboardAdapter } from "./dashboard-adapter";
import { petApi, petAssetUrl, type PetApi } from "./pet-api";
import { PetSprite, type PetAnimation } from "./pet-sprite";
import { buildPetUsageView, newestPetSnapshot } from "./pet-view-model";
import { formatRefreshCountdown } from "./format";
import "./pet-window.css";

interface PetUsageState {
  snapshot: DashboardSnapshot | null;
  schedule: RefreshSchedule | null;
  localRefreshing: boolean;
  channelFailed: boolean;
}

export function PetWindow({ api = petApi, adapter = dashboardAdapter }: { api?: PetApi; adapter?: DashboardAdapter }) {
  const [state, setState] = useState<PetState>(DEFAULT_PET_STATE);
  const [usageState, setUsageState] = useState<PetUsageState>({ snapshot: null, schedule: null, localRefreshing: false, channelFailed: false });
  const [themePreference, setThemePreference] = useState<ThemePreference>("system");
  const [systemDark, setSystemDark] = useState(() => window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? true);
  const [now, setNow] = useState(Date.now);
  const [hover, setHover] = useState(false);
  const [dragAnimation, setDragAnimation] = useState<PetAnimation | null>(null);
  const [error, setError] = useState<string | null>(null);
  const rootRef = useRef<HTMLElement>(null);
  const spriteRef = useRef<HTMLDivElement>(null);
  const barRef = useRef<HTMLButtonElement>(null);
  const lastLayout = useRef("");
  const [placement, setPlacement] = useState<PetLayout | null>(null);
  const placementRevision = useRef(0);
  const drag = useRef<{ pointer: number; ready: Promise<void>; frame: number | null; finishing: boolean; screenX: number; screenY: number } | null>(null);

  useEffect(() => {
    let cancelled = false;
    let unsubscribe: (() => void) | undefined;
    void api.subscribeLayout?.(next => {
      if (!cancelled) { placementRevision.current += 1; setPlacement(next); }
    }).then(stop => { if (cancelled) stop(); else unsubscribe = stop; }).catch(reason => {
      if (!cancelled) setError(String(reason instanceof Error ? reason.message : reason));
    });
    return () => { cancelled = true; unsubscribe?.(); };
  }, [api]);

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
    return () => { cancelled = true; stops.forEach(stop => stop()); };
  }, [api]);

  useEffect(() => {
    let cancelled = false;
    const stops: Array<() => void> = [];
    let lastSnapshotAt = Date.now();
    const retain = (stop: () => void) => { if (cancelled) stop(); else stops.push(stop); };
    const applySnapshot = (next: DashboardSnapshot, cached = false) => {
      if (cancelled) return;
      lastSnapshotAt = Date.now();
      setUsageState(current => {
        if (current.snapshot && (cached || newestPetSnapshot(current.snapshot, next) !== next)) return current;
        const active = !!next.refreshProgress && !["idle", "complete"].includes(next.refreshProgress.phase);
        // Account-only pushes reuse fetchedAt and cannot finish a newer local scan.
        const localChanged = next.fetchedAt !== current.snapshot?.fetchedAt || !adapter.subscribeProgress;
        return {
          ...current,
          snapshot: next,
          channelFailed: cached ? current.channelFailed : false,
          schedule: cached ? current.schedule ?? next.refreshSchedule ?? null : next.refreshSchedule ?? null,
          localRefreshing: cached ? current.localRefreshing || active : localChanged ? active : current.localRefreshing,
        };
      });
    };
    const subscribe = async (register: () => Promise<() => void>) => {
      try { retain(await register()); }
      catch {
        if (!cancelled) setUsageState(current => ({ ...current, channelFailed: true, localRefreshing: false }));
      }
    };
    setUsageState(current => ({ ...current, channelFailed: false }));
    const subscriptions = [
      ...(adapter.subscribe ? [subscribe(() => adapter.subscribe!(next => applySnapshot(next)))] : []),
      ...(adapter.subscribeProgress ? [subscribe(() => adapter.subscribeProgress!(next => {
        // A completion event alone cannot make the displayed data current.
        if (!cancelled && next.phase !== "complete" && next.phase !== "idle") {
          setUsageState(current => current.channelFailed ? current : { ...current, localRefreshing: true });
        }
      }))] : []),
      ...(adapter.subscribeRefreshSchedule ? [subscribe(() => adapter.subscribeRefreshSchedule!(schedule => {
        if (!cancelled) setUsageState(current => ({ ...current, schedule }));
      }))] : []),
    ];
    void (async () => {
      await Promise.all(subscriptions);
      if (cancelled) return;
      try {
        const cached = await adapter.readCachedSnapshot?.();
        if (cached) applySnapshot(cached, true);
      } catch (reason) { if (!cancelled) setError(String(reason instanceof Error ? reason.message : reason)); }
    })();
    let checking = false;
    const recovery = window.setInterval(() => {
      if (cancelled || checking || !adapter.readCachedSnapshot || Date.now() - lastSnapshotAt < 45_000) return;
      checking = true;
      void adapter.readCachedSnapshot().then(next => { if (next) applySnapshot(next); })
        .catch(() => { if (!cancelled) setUsageState(current => ({ ...current, channelFailed: true, localRefreshing: false })); })
        .finally(() => { checking = false; });
    }, 15_000);
    return () => { cancelled = true; clearInterval(recovery); stops.forEach(stop => stop()); };
  }, [adapter]);

  const snapshot = usageState.snapshot;
  const view = buildPetUsageView(snapshot, now);
  const selectedPet = state.pets.find(pet => pet.id === state.preferences.selectedPetId);
  const appTheme = themePreference === "system" ? (systemDark ? "dark" : "light") : themePreference;
  const theme = state.preferences.theme === "app" ? appTheme : state.preferences.theme;
  const refreshing = !usageState.channelFailed && (usageState.localRefreshing || view.refreshing);
  const refreshLabel = usageState.channelFailed ? "后台更新通道中断"
    : snapshot?.accountSync?.status === "syncing" && !usageState.localRefreshing ? "正在更新账号用量"
    : formatRefreshCountdown(usageState.schedule?.nextRefreshAtMs, refreshing, now);
  const animation: PetAnimation = dragAnimation ?? (hover ? "waiting" : refreshing ? "working" : "idle");
  useLayoutEffect(() => {
    const measure = () => {
      const root = rootRef.current?.getBoundingClientRect();
      const sprite = spriteRef.current?.getBoundingClientRect();
      const bar = barRef.current?.getBoundingClientRect();
      if (!root || !sprite || !bar || !root.width || !root.height) return;
      const rect = (box: DOMRect) => ({ x: box.x - root.x, y: box.y - root.y, width: box.width, height: box.height });
      const spriteRect = rect(sprite);
      const barRect = rect(bar);
      const layout: PetLayout = {
        width: Math.max(root.width, spriteRect.x + spriteRect.width, barRect.x + barRect.width),
        height: Math.max(root.height, spriteRect.y + spriteRect.height, barRect.y + barRect.height),
        sprite: spriteRect, bar: barRect,
      };
      // Position comes from native layout events; only content dimensions need remeasurement.
      const key = JSON.stringify([sprite.width, sprite.height, bar.width, bar.height]);
      if (lastLayout.current === key) return;
      lastLayout.current = key;
      const revision = placementRevision.current;
      void api.setLayout(layout).then(next => {
        if (next && revision === placementRevision.current) { placementRevision.current += 1; setPlacement(next); }
      }).catch(reason => { lastLayout.current = ""; setError(String(reason instanceof Error ? reason.message : reason)); });
    };
    measure();
    if (!window.ResizeObserver || !rootRef.current) return;
    const observer = new ResizeObserver(measure);
    observer.observe(rootRef.current);
    if (spriteRef.current) observer.observe(spriteRef.current);
    if (barRef.current) observer.observe(barRef.current);
    return () => observer.disconnect();
  }, [api, state.preferences.size, selectedPet?.id, view.quotas.length, view.quotaStatus, error]);

  const failDrag = (reason: unknown) => setError(String(reason instanceof Error ? reason.message : reason));
  const startDrag = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || drag.current) return;
    event.preventDefault();
    const handle = event.currentTarget;
    handle.setPointerCapture(event.pointerId);
    const ready = api.dragStart(event.clientX, event.clientY);
    setDragAnimation(null);
    drag.current = { pointer: event.pointerId, ready, frame: null, finishing: false, screenX: event.screenX, screenY: event.screenY };
    void ready.catch(reason => {
      failDrag(reason);
      const current = drag.current;
      if (current?.ready !== ready) return;
      current.finishing = true;
      if (current.frame !== null) cancelAnimationFrame(current.frame);
      drag.current = null;
      setDragAnimation(null);
      if (handle.hasPointerCapture(event.pointerId)) handle.releasePointerCapture(event.pointerId);
    });
  };
  const updateDrag = (event: PointerEvent<HTMLDivElement>) => {
    const current = drag.current;
    if (!current || current.pointer !== event.pointerId || current.finishing) return;
    const dx = event.screenX - current.screenX;
    const dy = event.screenY - current.screenY;
    if (Math.abs(dx) >= 4 || Math.abs(dy) >= 4) {
      current.screenX = event.screenX;
      current.screenY = event.screenY;
      if (dx >= 4) setDragAnimation("running-right");
      else if (dx <= -4) setDragAnimation("running-left");
    }
    if (current.frame !== null) return;
    current.frame = requestAnimationFrame(() => {
      current.frame = null;
      if (!current.finishing) void current.ready.then(() => api.dragUpdate()).catch(failDrag);
    });
  };
  const endDrag = (event: PointerEvent<HTMLDivElement>) => {
    const current = drag.current;
    if (!current || current.pointer !== event.pointerId || current.finishing) return;
    current.finishing = true;
    setDragAnimation(null);
    if (current.frame !== null) cancelAnimationFrame(current.frame);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    void current.ready.then(() => api.dragUpdate()).then(() => api.dragEnd()).catch(failDrag).finally(() => { if (drag.current === current) drag.current = null; });
  };

  return <main ref={rootRef} className="pet-window" style={placement ? { width: placement.width, height: placement.height } : undefined} data-pet-theme={theme} aria-label="AI Token 桌面宠物">
    <div ref={spriteRef} className="pet-window__sprite-handle" style={placement ? { position: "absolute", left: placement.sprite.x, top: placement.sprite.y } : undefined} title="拖动宠物可移动宠物和用量条" onPointerDown={startDrag} onPointerMove={updateDrag} onPointerUp={endDrag} onPointerCancel={endDrag} onLostPointerCapture={endDrag} onPointerEnter={() => setHover(true)} onPointerLeave={() => setHover(false)}>
      {selectedPet ? <PetSprite url={petAssetUrl(selectedPet.spritesheetPath)} version={selectedPet.spriteVersionNumber} size={state.preferences.size} animation={animation} name={selectedPet.displayName} /> : <div className="pet-window__loading-sprite" style={{ width: state.preferences.size, height: state.preferences.size * 208 / 192 }} />}
    </div>
    <button ref={barRef} type="button" className="pet-usage-bar" style={placement ? { position: "absolute", left: placement.bar.x, top: placement.bar.y } : undefined} aria-label="打开应用额度详情" title={`${view.details}\n${refreshLabel}`} onClick={() => void api.showDashboard().catch(failDrag)}>
      <span className="pet-usage-bar__totals"><span className="pet-usage-bar__tokens">今日 <strong>{view.tokensLabel}</strong> Token</span><span className="pet-usage-bar__cost">{view.unpriced && view.costLabel !== "未定价" ? "≥ " : ""}{view.costLabel}</span></span>
      <span className="pet-usage-bar__refresh" aria-label="自动更新状态" data-interrupted={usageState.channelFailed || undefined}>{refreshLabel}</span>
      {view.quotas.map(quota => <span className="pet-usage-bar__quota" key={quota.key} data-tone={quota.tone} aria-label={`${quota.title} 剩余 ${quota.remainingLabel}，${quota.resetLabel}`}>
        <span className="pet-usage-bar__quota-label">{quota.label}</span><span className="pet-usage-bar__remaining">剩余 {quota.remainingLabel}</span><span className="pet-usage-bar__track"><span style={{ width: `${quota.expired ? 0 : quota.remaining}%` }} /></span><span className="pet-usage-bar__reset">{quota.resetLabel}</span>
      </span>)}
      {(error || view.quotaStatus) && <span className="pet-usage-bar__status" data-stale={view.stale || undefined}>{error || view.quotaStatus}</span>}
    </button>
  </main>;
}
