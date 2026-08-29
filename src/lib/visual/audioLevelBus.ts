// A tiny module-level pub/sub for the real-time speech amplitude value Rust
// emits (see src-tauri/src/audio_level.rs + commands.rs's speak_text). This
// intentionally bypasses React state: the value updates ~25 times a second
// while Amin talks, and running that through useState/useEffect would
// re-render the whole component tree at that rate for no reason — only the
// 3D avatar's own requestAnimationFrame loop needs to read it, once per
// frame, imperatively. See docs/ARCHITECTURE.md's "Visual modes" section
// for why this exists and exactly what the level does and doesn't represent
// (short-window RMS loudness — not per-phoneme viseme timing).
let level = 0;
const subscribers = new Set<(level: number) => void>();

export function setAudioLevel(next: number): void {
  level = Math.max(0, Math.min(1, next));
  for (const cb of subscribers) cb(level);
}

export function getAudioLevel(): number {
  return level;
}

/** Only ThreeDAvatar's render loop subscribes today; exported for that use
 * and so a future portrait lip-sync renderer can read the same bus instead
 * of a second one, keeping "one voice, one signal" for either visual mode. */
export function subscribeAudioLevel(cb: (level: number) => void): () => void {
  subscribers.add(cb);
  return () => subscribers.delete(cb);
}

export function resetAudioLevel(): void {
  setAudioLevel(0);
}
