/** The mouth-shape timeline for the reply Amin is currently speaking, and
 * the moment playback started, so the 3D avatar can look up which shape
 * belongs on the face right now.
 *
 * Deliberately a module-level value read imperatively from the render
 * loop, exactly like audioLevelBus next to it and for the same reason: the
 * avatar's requestAnimationFrame loop is the only reader, and pushing a
 * per-frame lookup through React state would re-render the tree for
 * nothing.
 *
 * Why a timeline instead of reacting to the audio: a loudness signal
 * cannot tell you what shape a mouth is in. It only knows how loud the
 * room is. A loud "م" — lips pressed shut — and a loud "آ" — jaw wide —
 * are the same number. Mona's brief was that the lips should look like the
 * words are coming out of them, and that needs to know which letter is
 * being said, not how loud it is. ElevenLabs tells us exactly that, per
 * character, in milliseconds (see elevenlabs.rs's visemes_from_alignment).
 */
export interface VisemeCue {
  at_ms: number;
  viseme: string;
}

let cues: VisemeCue[] = [];
let startedAt = 0;

export function setVisemeTrack(next: VisemeCue[]): void {
  cues = next;
}

/** Called when playback actually begins (voice://speaking-started), which
 * is the zero point every `at_ms` is measured from. */
export function startVisemeTrack(): void {
  startedAt = performance.now();
}

export function clearVisemeTrack(): void {
  cues = [];
  startedAt = 0;
}

export function hasVisemeTrack(): boolean {
  return cues.length > 0 && startedAt > 0;
}

/** The shape the mouth should be in right now, and how far through its
 * hold it is (0 at the start of the shape, 1 by the end) — the caller uses
 * the second value to blend one shape into the next instead of snapping,
 * which is what makes it read as a mouth rather than a slideshow. */
export function currentViseme(): { viseme: string; progress: number } | null {
  if (!hasVisemeTrack()) return null;
  const t = performance.now() - startedAt;
  // Linear scan from the end: replies are short and cues are few, and the
  // last cue is nearly always the answer.
  for (let i = cues.length - 1; i >= 0; i--) {
    if (t >= cues[i].at_ms) {
      const next = cues[i + 1];
      const span = next ? Math.max(1, next.at_ms - cues[i].at_ms) : 200;
      return { viseme: cues[i].viseme, progress: Math.min(1, (t - cues[i].at_ms) / span) };
    }
  }
  return { viseme: cues[0].viseme, progress: 0 };
}
