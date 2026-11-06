import { useEffect, useRef, useState } from "react";
import { DEFAULT_PET_STATE, type PetPreferences, type PetSource, type PetState } from "../core/pet-types";
import { petApi, petAssetUrl, type PetApi } from "./pet-api";
import { PetSprite } from "./pet-sprite";
import "./pet-settings.css";

export function PetSettings({ appTheme, api = petApi }: { appTheme: "light" | "dark"; api?: PetApi }) {
  const [state, setState] = useState<PetState>(DEFAULT_PET_STATE);
  const currentState = useRef(state);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [sources, setSources] = useState<PetSource[] | null>(null);
  const [selectedSource, setSelectedSource] = useState("");
  const [previewUrl, setPreviewUrl] = useState("");
  const apply = (next: PetState) => { currentState.current = next; setState(next); };
  useEffect(() => {
    let cancelled = false;
    let unsubscribe: (() => void) | undefined;
    let receivedEvent = false;
    void (async () => {
      try {
        const stop = await api.subscribe(next => { receivedEvent = true; if (!cancelled) apply(next); });
        if (cancelled) { stop(); return; }
        unsubscribe = stop;
        const initial = await api.readState();
        if (!cancelled && !receivedEvent) apply(initial);
      } catch (reason) { if (!cancelled) setError(String(reason instanceof Error ? reason.message : reason)); }
      finally { if (!cancelled) setLoading(false); }
    })();
    return () => { cancelled = true; unsubscribe?.(); };
  }, [api]);
  useEffect(() => {
    let cancelled = false;
    setPreviewUrl("");
    if (selectedSource) void api.preview(selectedSource).then(url => { if (!cancelled) setPreviewUrl(url); }).catch(reason => { if (!cancelled) setError(String(reason instanceof Error ? reason.message : reason)); });
    return () => { cancelled = true; };
  }, [api, selectedSource]);

  const perform = async (action: () => Promise<void>) => {
    if (busy) return;
    setBusy(true); setError(null); setMessage(null);
    try { await action(); }
    catch (reason) { setError(String(reason instanceof Error ? reason.message : reason)); }
    finally { setBusy(false); }
  };
  const changePreferences = (patch: Partial<PetPreferences>) => perform(async () => {
    apply(await api.setPreferences({ ...currentState.current.preferences, ...patch }));
  });
  const loadSources = (otherDirectory: boolean) => perform(async () => {
    const found = otherDirectory ? await api.selectDirectory() : await api.discover();
    if (found === null) return;
    setSources(found); setSelectedSource(found[0]?.sourcePath ?? "");
    if (!found.length) setMessage("这个目录中没有找到可导入的本地宠物。可选择其他 Codex 目录，或手动导入宠物文件。");
  });
  const selectedPet = state.pets.find(pet => pet.id === state.preferences.selectedPetId);
  const source = sources?.find(pet => pet.sourcePath === selectedSource);
  const disabled = loading || busy || !api.native;
  const theme = state.preferences.theme === "app" ? appTheme : state.preferences.theme;
  return <article className="content-card pet-settings" aria-labelledby="pet-settings-title" data-pet-theme={theme}>
    <header className="pet-settings__heading"><div><p className="eyebrow">DESKTOP COMPANION</p><h2 id="pet-settings-title">桌面宠物</h2><p>让宠物和用量条陪伴工作，数据与本应用同步。</p></div>
      <label className="pet-settings__toggle"><input type="checkbox" aria-label="显示桌面宠物" checked={state.preferences.enabled} disabled={disabled || !selectedPet} onChange={event => void changePreferences({ enabled: event.target.checked })} /><span>显示宠物</span></label>
    </header>
    {!api.native && <p className="pet-settings__notice">桌面宠物可在 Windows 应用中使用。</p>}
    <div className="pet-settings__body">
      <div className="pet-settings__preview" aria-label="已选宠物预览">
        {selectedPet ? <PetSprite url={petAssetUrl(selectedPet.spritesheetPath)} version={selectedPet.spriteVersionNumber} size={112} name={selectedPet.displayName} /> : <span className="pet-settings__empty">导入你的第一只宠物</span>}
        <strong>{selectedPet?.displayName ?? "尚未选择宠物"}</strong>
        <span>{selectedPet?.description || "支持 Codex 本地自定义宠物"}</span>
      </div>
      <div className="pet-settings__controls">
        <label><span>当前宠物</span><select aria-label="当前宠物" disabled={disabled || !state.pets.length} value={state.preferences.selectedPetId ?? ""} onChange={event => void changePreferences({ selectedPetId: event.target.value })}>
          {!state.pets.length && <option value="">请先导入宠物</option>}
          {state.pets.map(pet => <option key={pet.id} value={pet.id}>{pet.displayName}</option>)}
        </select></label>
        <div className="pet-settings__row">
          <label><span>宠物大小</span><select aria-label="宠物大小" disabled={disabled} value={state.preferences.size} onChange={event => void changePreferences({ size: Number(event.target.value) })}>
            {[96, 112, 128, 144, 160, 192].map(size => <option key={size} value={size}>{size} px{size === 128 ? " · 默认" : ""}</option>)}
          </select></label>
          <label><span>用量条主题</span><select aria-label="宠物用量条主题" disabled={disabled} value={state.preferences.theme} onChange={event => void changePreferences({ theme: event.target.value as PetPreferences["theme"] })}>
            <option value="app">跟随应用</option><option value="light">浅色</option><option value="dark">深色</option>
          </select></label>
        </div>
        <div className="pet-settings__actions"><button type="button" className="secondary-button" disabled={disabled} onClick={() => void loadSources(false)}>从本机 Codex 导入</button>
          <button type="button" className="secondary-button" disabled={disabled} onClick={() => void perform(async () => { const next = await api.selectFile(); if (next) { apply(next); setMessage("宠物已导入，可打开“显示宠物”。"); } })}>导入宠物文件</button>
          <button type="button" className="secondary-button" disabled={disabled || !selectedPet} onClick={() => void perform(async () => { apply(await api.resetPosition()); setMessage("宠物位置已重置。"); })}>重置位置</button>
        </div>
        <p className="pet-settings__hint">可导入 pet.json、PNG 或 WebP 动画图集（v1 / v2）。资源保存在应用内，之后无需启动 Codex。</p>
      </div>
    </div>
    {sources !== null && <section className="pet-settings__library" aria-label="本机 Codex 宠物库">
      <div className="pet-settings__library-heading"><strong>本机 Codex 宠物库</strong><div className="pet-settings__actions"><button type="button" className="secondary-button" disabled={disabled} onClick={() => void loadSources(true)}>选择其他 Codex 目录</button><button type="button" className="secondary-button" disabled={busy} onClick={() => { setSources(null); setSelectedSource(""); }}>收起</button></div></div>
      {sources.length > 0 && <div className="pet-settings__import-preview"><label><span>选择待导入宠物</span><select aria-label="选择待导入宠物" disabled={disabled} value={selectedSource} onChange={event => setSelectedSource(event.target.value)}>{sources.map(item => <option key={item.sourcePath} value={item.sourcePath}>{item.displayName}</option>)}</select></label>
        <div className="pet-settings__source-preview">{source && previewUrl ? <PetSprite url={previewUrl} version={source.spriteVersionNumber} size={96} name={`${source.displayName} 导入预览`} /> : <span>正在读取预览…</span>}</div>
        <p>{source?.description}</p><button type="button" className="primary-button" disabled={disabled || !source} onClick={() => void perform(async () => { apply(await api.importSource(selectedSource)); setMessage(`${source?.displayName ?? "宠物"}已导入，可打开“显示宠物”。`); })}>导入这只宠物</button>
      </div>}
      <p className="pet-settings__hint">这里只显示本地资源。仅保存在云端的宠物需先导出到本机，再导入文件。</p>
    </section>}
    {loading && <p role="status">正在读取宠物设置…</p>}{busy && <p role="status">正在处理…</p>}
    {error && <p className="pet-settings__error" role="alert">{error}</p>}{message && <p className="pet-settings__message" role="status">{message}</p>}
  </article>;
}
