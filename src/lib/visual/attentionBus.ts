/** When Amin last heard Mona say something.
 *
 * The three robots Mona sent as references (Columbia's Emo in two of
 * them, Promobot in the third) are not admired for talking. Emo's whole
 * headline is that it reacts WITH the person — "روبوت مُصمم لمحاكاة
 * المشاعر والتفاعل" — and Promobot's segment says outright that the face,
 * not the voice, is the hard part: "تعبيرات الوجه هي أصعب ما في العملية".
 *
 * Amin's face had nothing at all to do while Mona was the one talking. It
 * came alive only for his own reply, which is exactly backwards from a
 * conversation: the moments a listener's face matters most are the ones
 * where they are listening.
 *
 * `voice://partial` already fires several times a second while she
 * speaks, so this needs no new plumbing in the Swift engine — it is the
 * signal "she just said another word", which is all a listening face
 * needs to stay engaged.
 *
 * Module-level and read imperatively from the render loop, same as
 * audioLevelBus and visemeTrack beside it, and for the same reason: this
 * updates several times a second and only the avatar's own rAF loop reads
 * it. */
let lastHeardAt = 0;

export function markHeardSpeech(): void {
  lastHeardAt = performance.now();
}

export function clearAttention(): void {
  lastHeardAt = 0;
}

/** 1 the instant she says a word, decaying to 0 over about a second and a
 * half of silence. Fractional rather than a boolean so the face settles
 * when she pauses instead of switching between two poses. */
export function attention(): number {
  if (lastHeardAt === 0) return 0;
  const since = performance.now() - lastHeardAt;
  return Math.max(0, 1 - since / 1500);
}
