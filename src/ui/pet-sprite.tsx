import { useEffect, useState, type CSSProperties } from "react";
import "./pet-sprite.css";

export type PetAnimation = "idle" | "wave" | "working" | "failed" | "waiting" | "running-left" | "running-right";

function action(row: number, frames: number, duration: number, lastDuration: number) {
  return { row, frames, frameDurationsMs: Array.from({ length: frames }, (_, index) => index === frames - 1 ? lastDuration : duration) };
}

// Match Codex's slow idle and its individual action-frame timings.
export const PET_ANIMATIONS = {
  idle: { row: 0, frames: 6, frameDurationsMs: [1680, 660, 660, 840, 840, 1920] },
  wave: action(3, 4, 140, 280),
  working: action(7, 6, 120, 220),
  failed: action(5, 8, 140, 240),
  waiting: action(6, 6, 150, 260),
  "running-left": action(2, 8, 120, 220),
  "running-right": action(1, 8, 120, 220),
} satisfies Record<PetAnimation, { row: number; frames: number; frameDurationsMs: number[] }>;

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
  const [playback, setPlayback] = useState({ requested: animation, url, animation, frame: 0 });
  useEffect(() => {
    const media = window.matchMedia?.("(prefers-reduced-motion: reduce)");
    let timer: ReturnType<typeof setTimeout> | undefined;
    const updateMotion = () => {
      if (timer !== undefined) clearTimeout(timer);
      let active = animation;
      let frame = 0;
      let rounds = 0;
      const show = () => setPlayback({ requested: animation, url, animation: active, frame });
      const advance = () => {
        timer = setTimeout(() => {
          frame += 1;
          if (frame === PET_ANIMATIONS[active].frames) {
            frame = 0;
            rounds += 1;
            // Codex plays transient actions three times, then keeps a slow idle.
            if (active !== "idle" && rounds === 3) active = "idle";
          }
          show();
          advance();
        }, PET_ANIMATIONS[active].frameDurationsMs[frame]);
      };
      show();
      if (!media?.matches) advance();
    };
    updateMotion();
    media?.addEventListener("change", updateMotion);
    return () => { if (timer !== undefined) clearTimeout(timer); media?.removeEventListener("change", updateMotion); };
  }, [animation, url]);
  const current = playback.requested === animation && playback.url === url ? playback : { animation, frame: 0 };
  const contract = PET_ANIMATIONS[current.animation];
  return <div role="img" aria-label={name} className="pet-sprite" data-animation={current.animation} data-frame={current.frame} style={{ ...spriteFrameStyle(size, contract.row, current.frame, version), backgroundImage: url ? `url(${JSON.stringify(url)})` : undefined }} />;
}
