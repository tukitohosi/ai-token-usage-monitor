// @vitest-environment jsdom
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PetSprite } from "../src/ui/pet-sprite";

let reducedMotion = false;
let motionChanged: (() => void) | undefined;
beforeEach(() => {
  vi.useFakeTimers();
  reducedMotion = false;
  motionChanged = undefined;
  Object.defineProperty(window, "matchMedia", { configurable: true, value: vi.fn(() => ({
    get matches() { return reducedMotion; },
    addEventListener: (_event: string, listener: () => void) => { motionChanged = listener; },
    removeEventListener: () => { motionChanged = undefined; },
  })) });
});
afterEach(() => { cleanup(); vi.useRealTimers(); vi.restoreAllMocks(); });

const pet = { url: "data:image/webp;base64,AAAA", version: 2 as const, name: "狐仙" };
function advance(milliseconds: number) { act(() => vi.advanceTimersByTime(milliseconds)); }

describe("Codex pet animation timing", () => {
  it("holds every idle frame for the Codex duration and repeats at 6.6 seconds", () => {
    render(<PetSprite {...pet} />);
    const sprite = screen.getByRole("img", { name: "狐仙" });
    const durations = [1680, 660, 660, 840, 840, 1920];
    for (let frame = 0; frame < durations.length; frame += 1) {
      expect(sprite.dataset.frame).toBe(String(frame));
      advance(durations[frame] - 1);
      expect(sprite.dataset.frame).toBe(String(frame));
      advance(1);
    }
    expect(sprite.dataset.frame).toBe("0");
    expect(sprite.dataset.animation).toBe("idle");
  });

  it("plays waiting three times then continues the slow idle", () => {
    render(<PetSprite {...pet} animation="waiting" />);
    const sprite = screen.getByRole("img", { name: "狐仙" });
    advance(750);
    expect(sprite.dataset.animation).toBe("waiting");
    expect(sprite.dataset.frame).toBe("5");
    advance(259);
    expect(sprite.dataset.frame).toBe("5");
    advance(1);
    expect(sprite.dataset.frame).toBe("0");
    advance(2019);
    expect(sprite.dataset.animation).toBe("waiting");
    advance(1);
    expect(sprite.dataset.animation).toBe("idle");
    expect(sprite.style.backgroundPosition).toBe("0px 0px");
    advance(1679);
    expect(sprite.dataset.frame).toBe("0");
    advance(1);
    expect(sprite.dataset.frame).toBe("1");
  });

  it("preserves a running sequence on rerender and restarts only when its direction changes", () => {
    const view = render(<PetSprite {...pet} animation="running-right" />);
    const sprite = screen.getByRole("img", { name: "狐仙" });
    advance(840);
    expect(sprite.dataset.frame).toBe("7");
    expect(sprite.style.backgroundPosition).toBe("-896px -138.66666666666666px");
    view.rerender(<PetSprite {...pet} size={128} animation="running-right" />);
    advance(219);
    expect(sprite.dataset.frame).toBe("7");
    advance(1);
    expect(sprite.dataset.frame).toBe("0");
    advance(120);
    view.rerender(<PetSprite {...pet} animation="running-left" />);
    expect(sprite.dataset.animation).toBe("running-left");
    expect(sprite.dataset.frame).toBe("0");
    advance(3179);
    expect(sprite.dataset.animation).toBe("running-left");
    advance(1);
    expect(sprite.dataset.animation).toBe("idle");
  });

  it("uses a static first frame for reduced motion and responds when the preference changes", () => {
    reducedMotion = true;
    const view = render(<PetSprite {...pet} animation="waiting" />);
    const sprite = screen.getByRole("img", { name: "狐仙" });
    advance(10_000);
    expect(sprite.dataset.animation).toBe("waiting");
    expect(sprite.dataset.frame).toBe("0");
    act(() => { reducedMotion = false; motionChanged?.(); });
    advance(150);
    expect(sprite.dataset.frame).toBe("1");
    act(() => { reducedMotion = true; motionChanged?.(); });
    advance(1000);
    expect(sprite.dataset.frame).toBe("0");
    view.unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
});
