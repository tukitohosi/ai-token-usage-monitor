import { useEffect, useState, type CSSProperties } from "react";
import "./pet-sprite.css";

export type PetAnimation = "idle" | "wave" | "working" | "failed";
export const PET_ANIMATIONS: Record<PetAnimation, { row: number; frames: number }> = {
  idle: { row: 0, frames: 6 }, wave: { row: 3, frames: 4 },
  working: { row: 7, frames: 6 }, failed: { row: 5, frames: 8 },
};

export function spriteFrameStyle(size: number, row: number, frame: number, version: 1 | 2): CSSProperties {
  const height = size * 208 / 192;
  return {
    width: size, height,
    backgroundSize: `${size * 8}px ${height * (version === 2 ? 11 : 9)}px`,
    backgroundPosition: `${-frame * size}px ${-row * height}px`,
  };
}

export function PetSprite({ url, version, size = 128, animation = "idle", name }: {
  url: string; version: 1 | 2; size?: number; animation?: PetAnimation; name: string;
}) {
  const [frame, setFrame] = useState(0);
  useEffect(() => {
    setFrame(0);
    const media = window.matchMedia?.("(prefers-reduced-motion: reduce)");
    if (media?.matches) return;
    let timer: ReturnType<typeof setInterval> | undefined;
    const updateMotion = () => {
      if (timer) clearInterval(timer);
      setFrame(0);
      if (!media?.matches) timer = setInterval(() => setFrame(current => (current + 1) % PET_ANIMATIONS[animation].frames), 140);
    };
    updateMotion();
    media?.addEventListener("change", updateMotion);
    return () => { if (timer) clearInterval(timer); media?.removeEventListener("change", updateMotion); };
  }, [animation, url]);
  const contract = PET_ANIMATIONS[animation];
  return <div role="img" aria-label={name} className="pet-sprite" style={{ ...spriteFrameStyle(size, contract.row, frame % contract.frames, version), backgroundImage: url ? `url(${JSON.stringify(url)})` : undefined }} />;
}
