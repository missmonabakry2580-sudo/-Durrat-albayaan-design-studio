import { useEffect, useRef } from "react";
import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { getAudioLevel } from "../../lib/visual/audioLevelBus";
import { currentViseme, hasVisemeTrack } from "../../lib/visual/visemeTrack";
import { attention } from "../../lib/visual/attentionBus";
import type { AminState } from "./types";

interface ThreeDAvatarProps {
  state: AminState;
  /** The tone Claude tagged its own reply with (agent.rs's KNOWN_EMOTIONS —
   * happy/calm/concerned/excited/apologetic/serious/playful/neutral), or
   * null/undefined when the last reply carried none. Drives
   * EMOTION_EXPRESSIONS below; an unrecognized value just falls through to
   * no expression rather than guessing one. */
  emotion?: string | null;
  className?: string;
  /** Fired once if the model fails to load or WebGL isn't available, so the
   * caller can fall back to Portrait mode instead of showing a blank box. */
  onFailure: (reason: string) => void;
}

/** A named subset of the rig's 52 ARKit blendshapes (see
 * scripts/facial-rig/required-targets.mjs for the full list this model was
 * validated against) — deliberately excludes every name the blink/gaze/
 * mouth logic further down already owns every frame (eyeBlinkLeft/Right,
 * eyeLook{Up,Down,In,Out}{Left,Right}, jawOpen, and every viseme_* name),
 * so this expression layer and that one never fight over the same morph
 * target.
 *
 * Real bug this fixes, a real Mac (2026-08-28), Mona: "مفيش اي تعبيرات
 * بتصدر من وجهه إلا فم بيفتح لفوق وينزل لتحت فقط" (no expressions at all
 * come from his face except a mouth that opens and closes) — accurate:
 * before this, ThreeDAvatar never received the `emotion` prop at all
 * (AminPresence tracked it but never passed it down), and nothing in the
 * animate() loop touched a single brow/mouth-shape morph target tied to
 * state or emotion. Both gaps are fixed here, not just one — the emotion
 * plumbing (AminPresence → ThreeDAvatar) and the actual expression logic
 * that uses it. */
type ExpressionTargets = Record<string, number>;

/** Amin's 8 real, disclosed emotions, each a resting facial expression.
 * Claude tags at most one of these per reply — never invented, never
 * guessed from tone-of-text analysis this file has no way to do.
 *
 * Intensities pushed toward the top of the 0-1 range deliberately, found
 * by directly inspecting this exact .glb's own vertex data (not guessed):
 * `mouthSmileLeft`'s sparse morph target moves its ~2500 affected
 * vertices by at most ~8mm, versus `jawOpen`'s ~35mm — these blendshapes
 * are real (present, correctly named, correctly wired — confirmed via
 * morphTargetDictionary and by rendering a diff of two screenshots at
 * 0.55, which did show a real, measured pixel difference), just visually
 * subtle at Meshy's sculpted magnitude. A value that would read as an
 * obvious smile on a rig with stronger deltas was imperceptible at a
 * glance here; these values are calibrated against actual screenshots of
 * this model, not a generic assumption about what "0.6" should look
 * like. */
/** How fast each emotion arrives on the face, in seconds to essentially
 * complete.
 *
 * From Mona's Engineered Arts reference (an 11-minute Ameca compilation,
 * fetched from her Drive): its signature moment is a jump from a neutral
 * face to full shock — eyes wide, brows up, jaw dropped, head back —
 * inside a couple of frames, which then HOLDS and relaxes slowly. Every
 * expression here used to arrive at one uniform, leisurely rate (a flat
 * ~400ms crossfade for all eight), and an emotion that fades in gently is
 * not that emotion. A slow surprise is not surprise; it reads as the face
 * drifting.
 *
 * So the fast, reactive emotions snap and the settled ones ease. Decay is
 * always slower than onset, which is how real expressions behave: they
 * arrive suddenly and leave gradually. */
const EMOTION_ONSET_SECONDS: Record<string, number> = {
  excited: 0.11,
  concerned: 0.15,
  apologetic: 0.3,
  happy: 0.22,
  playful: 0.18,
  serious: 0.25,
  calm: 0.6,
  neutral: 0.5,
};
const EMOTION_DECAY_SECONDS = 0.75;

/** The parts of an emotion that are not blendshapes: how far the jaw
 * drops and what the head does. Ameca's shock is not a brow pose — the
 * jaw falls open and the head pulls back at the same instant, and it is
 * the coordination that sells it. This avatar's emotions previously lived
 * entirely in the brows and lip corners, so even a correct expression had
 * no body behind it.
 *
 * `jaw` applies only when Amin is NOT speaking; while he talks the mouth
 * belongs to the viseme track (see VISEME_JAW_CEILING) and an emotion
 * must not fight it. `pitch` is head rotation in radians — negative pulls
 * the chin up and back, positive dips it. `tilt` is the sideways cock of
 * the head that reads as curiosity or sympathy. */
const EMOTION_POSTURE: Record<string, { jaw?: number; pitch?: number; tilt?: number }> = {
  excited: { jaw: 0.3, pitch: -0.05 },
  concerned: { jaw: 0.05, pitch: 0.03, tilt: 0.05 },
  apologetic: { pitch: 0.06, tilt: 0.07 },
  happy: { jaw: 0.12, pitch: -0.02 },
  playful: { jaw: 0.08, tilt: -0.06 },
  serious: { pitch: 0.02 },
  calm: {},
  neutral: {},
};

const EMOTION_EXPRESSIONS: Record<string, ExpressionTargets> = {
  happy: { mouthSmileLeft: 0.9, mouthSmileRight: 0.9, cheekSquintLeft: 0.6, cheekSquintRight: 0.6 },
  excited: {
    mouthSmileLeft: 0.8,
    mouthSmileRight: 0.8,
    browOuterUpLeft: 0.75,
    browOuterUpRight: 0.75,
    browInnerUp: 0.6,
    eyeWideLeft: 0.55,
    eyeWideRight: 0.55,
  },
  concerned: { browDownLeft: 0.65, browDownRight: 0.65, browInnerUp: 0.55, mouthFrownLeft: 0.55, mouthFrownRight: 0.55 },
  apologetic: { browInnerUp: 0.75, mouthFrownLeft: 0.4, mouthFrownRight: 0.4, eyeSquintLeft: 0.25, eyeSquintRight: 0.25 },
  serious: { browDownLeft: 0.55, browDownRight: 0.55, mouthPressLeft: 0.4, mouthPressRight: 0.4 },
  // An asymmetric smile (left corner up more than right) reads as a smirk
  // rather than a plain smile — the one deliberately lopsided expression
  // here, matching what "playful" is supposed to feel like.
  playful: { mouthSmileLeft: 0.85, mouthSmileRight: 0.5, browOuterUpRight: 0.6 },
  calm: {},
  neutral: {},
};

/** Amin's own cognitive state (types.ts), layered on top of whichever
 * emotion expression above is currently resting — additive per morph
 * target (summed, then clamped to 1 in the blend loop) rather than one
 * replacing the other, so "thinking" while the last reply was "concerned"
 * reads as both at once instead of either erasing the other. */
const STATE_EXPRESSIONS: Record<AminState, ExpressionTargets> = {
  idle: {},
  // REVISED 2026-08-28 — these were originally 0.2/0.15/0.15 and
  // 0.3/0.25/0.25, kept deliberately mild for calm background
  // attentiveness. Real bug: this model's blendshape vertex
  // displacements are small (see the "small blendshape deltas" note
  // elsewhere in this file/ARCHITECTURE.md) — this session already found
  // ~0.5+ necessary before a change reads as visible at all on-screen, so
  // "mild" here meant "invisible", not "subtle". Mona reported hands-free
  // looking completely unchanged when armed, which this alone could fully
  // explain regardless of whether listening itself was working. Pushed
  // into the same visible range every other calibrated expression uses.
  // RE-REVISED 2026-08-28, same day: the 0.5-0.6 eyeWide values from the
  // morning's "make it visible" pass overshot badly on the OTHER axis —
  // a real screenshot from Mona showed a bug-eyed sideways stare ("ايه
  // شكله ده"), because eyeWide's displacement on this rig is large (like
  // jawOpen, unlike the ~8mm smile shapes), so 0.5+ bares the whites.
  // Not every blendshape needs the small-delta boost: brows carry
  // "attentive" and CAN sit high; eye-widening reads as startled past a
  // low threshold and must stay subtle.
  armed: { browInnerUp: 0.5, eyeWideLeft: 0.15, eyeWideRight: 0.15 },
  listening: { browInnerUp: 0.6, eyeWideLeft: 0.22, eyeWideRight: 0.22 },
  thinking: { browInnerUp: 0.5, browDownLeft: 0.3 },
  planning: { browInnerUp: 0.4, browDownLeft: 0.25 },
  executing: { browDownLeft: 0.35, browDownRight: 0.35 },
  speaking: {},
  success: { mouthSmileLeft: 0.6, mouthSmileRight: 0.6, browOuterUpLeft: 0.35, browOuterUpRight: 0.35 },
  warning: { browDownLeft: 0.65, browDownRight: 0.65, mouthFrownLeft: 0.45, mouthFrownRight: 0.45 },
  waiting: { mouthShrugUpper: 0.25 },
};

/** Every blendshape name either map above ever targets — computed once so
 * the animate() loop can lerp each of them toward 0 the instant neither
 * the current emotion nor the current state asks for it anymore, instead
 * of leaving a stale expression stuck on the face after a state change. */
const ALL_EXPRESSION_NAMES = [
  ...new Set([...Object.values(EMOTION_EXPRESSIONS), ...Object.values(STATE_EXPRESSIONS)].flatMap(Object.keys)),
];

/** Every mouth-shape name either expression map can target — as opposed to
 * the brow/eye/cheek ones, these directly reshape the same lips jawOpen
 * and the viseme_* targets are already animating during speech. Real bug,
 * a real Mac (2026-08-28): Mona's reply was tagged "happy" (mouthSmileLeft/
 * Right at 0.9), and while she was actively speaking the jaw was
 * simultaneously wide open for the audio-reactive viseme animation — the
 * two combined into a distorted, overly wide, teeth-baring mouth that
 * looked broken rather than expressive ("هي دي تعبيرات الفم المتطابقة مع
 * الكلام؟؟؟"). Suppressing exactly these names while speaking (see
 * combineExpressions below) hands the mouth entirely to the jaw/viseme
 * animation for the duration of the utterance; brow/eye/cheek expression
 * keeps running underneath the whole time, so "happy while talking" still
 * reads in the eyes and brows, just not fighting over the mouth shape. */
/** Every mouth shape this rig can make, confirmed against
 * public/models/amin_facial_rig.glb's own morphTargetDictionary rather
 * than assumed from the Oculus spec — the full set is present on both the
 * face mesh and the lower teeth, which is what lets the teeth follow the
 * lips instead of sitting still behind them. `viseme_sil` is deliberately
 * NOT in this list: it is the resting shape, driven separately. */
const VISEME_NAMES = [
  "viseme_PP",
  "viseme_FF",
  "viseme_TH",
  "viseme_DD",
  "viseme_kk",
  "viseme_CH",
  "viseme_SS",
  "viseme_nn",
  "viseme_RR",
  "viseme_aa",
  "viseme_E",
  "viseme_ih",
  "viseme_oh",
  "viseme_ou",
] as const;

/** How far the jaw is allowed to drop while each shape is on the face.
 *
 * REAL DEFECT this fixes, caught by photographing every shape rather than
 * trusting the design: with the jaw still driven purely by loudness, a
 * loud "م" — lips pressed shut — rendered with the mouth hanging open. The
 * viseme was correct and the jaw was contradicting it, which is the very
 * bug the viseme track was built to end. Loudness may modulate the jaw
 * WITHIN what a shape allows; it may not overrule the shape. A bilabial
 * closes the lips no matter how loudly it is said.
 *
 * Values are articulatory, not tuned by taste: bilabials and labiodentals
 * close, sibilants and alveolars sit nearly closed, back consonants and
 * close vowels open a little, rounded vowels a little more, and only the
 * open vowel "aa" gets the full range. */
const VISEME_JAW_CEILING: Record<string, number> = {
  viseme_sil: 0,
  viseme_PP: 0,
  viseme_FF: 0.08,
  viseme_nn: 0.12,
  viseme_SS: 0.14,
  viseme_TH: 0.16,
  viseme_DD: 0.18,
  viseme_CH: 0.2,
  viseme_RR: 0.24,
  viseme_ih: 0.24,
  viseme_E: 0.32,
  viseme_kk: 0.34,
  viseme_ou: 0.36,
  viseme_oh: 0.55,
  viseme_aa: 1,
};

const MOUTH_SHAPE_NAMES = new Set([
  "mouthSmileLeft",
  "mouthSmileRight",
  "mouthFrownLeft",
  "mouthFrownRight",
  "mouthPressLeft",
  "mouthPressRight",
  "mouthShrugUpper",
]);

/** Sums the emotion and state expression maps per blendshape name,
 * clamping each to 1 — two mild expressions stacking shouldn't be able to
 * exceed what a single strong one would look like. `suppressMouthShapes`
 * zeroes every MOUTH_SHAPE_NAMES target instead of summing it — see that
 * set's own comment for why (active speech already owns the mouth). */
function combineExpressions(
  emotion: ExpressionTargets,
  state: ExpressionTargets,
  suppressMouthShapes: boolean,
): Map<string, number> {
  const combined = new Map<string, number>();
  for (const name of ALL_EXPRESSION_NAMES) {
    if (suppressMouthShapes && MOUTH_SHAPE_NAMES.has(name)) {
      combined.set(name, 0);
      continue;
    }
    combined.set(name, Math.min(1, (emotion[name] ?? 0) + (state[name] ?? 0)));
  }
  return combined;
}

const MODEL_URL = "/models/amin_facial_rig.glb";

// Face meshes carrying the 51 ARKit + 15 Oculus morph targets — see
// docs/ARCHITECTURE.md's facial-rig section for how this file was produced
// and validated (scripts/facial-rig/validate-facial-rig.mjs). AvatarBody,
// the corneas, and outfit carry no morph targets and are irrelevant here.
const FACE_MESH_NAMES = ["AvatarHead", "AvatarEyelashes", "AvatarTeethLower"];

/** Sets a morph target influence by name on every mesh that declares it —
 * meshes that don't (e.g. "cheekPuff" only exists on AvatarHead) are
 * silently skipped rather than erroring, since which mesh carries which
 * ARKit name is Meshy's export choice, not something this code should
 * assume or hard-code per-mesh. */
function setMorph(meshes: THREE.SkinnedMesh[], name: string, value: number): void {
  for (const mesh of meshes) {
    const dict = mesh.morphTargetDictionary;
    const influences = mesh.morphTargetInfluences;
    if (!dict || !influences || !(name in dict)) continue;
    influences[dict[name]] = value;
  }
}

function lerp(current: number, target: number, damping: number): number {
  return current + (target - current) * damping;
}

/**
 * Renders Amin's real 3D facial rig (public/models/amin_facial_rig.glb —
 * the exact file produced and validated this session; see
 * docs/ARCHITECTURE.md) instead of the flat identity portrait.
 *
 * Every motion here is driven by a real, disclosed signal, never invented:
 *  - Blink: a randomized timer (2.5-6s between blinks), the standard
 *    technique for idle avatar blinking — not audio- or state-driven.
 *  - Eye saccades: small randomized gaze targets on the real LeftEye/
 *    RightEye bones the rig ships with (this is a full Mixamo-style
 *    skeleton, not a face-only mesh).
 *  - Head sway: a low-amplitude sine composite on the real Head bone,
 *    kept under ~2° so it reads as breathing, not nodding.
 *  - Mouth while speaking: driven by the real-time RMS loudness Rust
 *    computes from the actual audio Mona hears (see
 *    src-tauri/src/audio_level.rs + the voice://audio-level event) via
 *    audioLevelBus — jaw opens proportionally to loudness, blended across
 *    a couple of open-mouth visemes for a little shape variety. This is
 *    honestly amplitude-reactive lip movement, not phoneme-accurate
 *    viseme lip-sync (that needs real-time phoneme alignment, which
 *    doesn't exist in this pipeline) — disclosed as such to Mona rather
 *    than oversold.
 * There is no tongue in this rig (see the facial-rig validator's
 * documented tongueOut gap) and no facial-expression animation clips —
 * only what's listed above is real; nothing else is faked.
 */
export function ThreeDAvatar({ state, emotion, className, onFailure }: ThreeDAvatarProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const stateRef = useRef(state);
  stateRef.current = state;
  const emotionRef = useRef(emotion);
  emotionRef.current = emotion;

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    let renderer: THREE.WebGLRenderer;
    try {
      renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
    } catch (e) {
      onFailure(`WebGL غير متاح: ${String(e)}`);
      return;
    }

    const scene = new THREE.Scene();
    const camera = new THREE.PerspectiveCamera(24, 1, 0.05, 50);
    renderer.setClearColor(0x000000, 0);
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    container.appendChild(renderer.domElement);
    renderer.domElement.style.width = "100%";
    renderer.domElement.style.height = "100%";

    const key = new THREE.DirectionalLight(0xfff2d9, 2.4);
    key.position.set(0.6, 1.2, 1.6);
    scene.add(key);
    const fill = new THREE.DirectionalLight(0x9fd0ff, 1.1);
    fill.position.set(-1.2, 0.4, 1.0);
    scene.add(fill);
    scene.add(new THREE.AmbientLight(0x404050, 1.1));

    let disposed = false;
    let rafId = 0;
    let faceMeshes: THREE.SkinnedMesh[] = [];
    let leftCornea: THREE.Object3D | null = null;
    let rightCornea: THREE.Object3D | null = null;
    let headBone: THREE.Object3D | null = null;
    let leftEyeBone: THREE.Object3D | null = null;
    let rightEyeBone: THREE.Object3D | null = null;
    let headBaseQuat = new THREE.Quaternion();
    let leftEyeBaseQuat = new THREE.Quaternion();
    let rightEyeBaseQuat = new THREE.Quaternion();

    const jaw = { current: 0 };
    const sil = { current: 1 };
    const blink = {
      value: 0,
      timer: 1 + Math.random() * 2,
      phase: "waiting" as "waiting" | "closing" | "opening",
      // A real blink is often a quick double. Tracking one pending repeat
      // is the whole trick: a single, perfectly regular blink is one of
      // the strongest "this is a computer" tells there is.
      double: false,
    };
    const gaze = { x: 0, y: 0, targetX: 0, targetY: 0, timer: 1 };
    // Last frame's attention value. The gaze block runs before this
    // frame's is computed, and one frame of lag on "is she talking" is
    // imperceptible — far better than reordering the whole loop.
    const attentionRef = { current: 0 };
    // Speech emphasis: the audio level with a fast attack and a slow
    // release, so it spikes on a stressed syllable and eases off rather
    // than tracking every wobble. This is what Mona's reference footage
    // (Engineered Arts' Ameca) actually does that this avatar did not:
    // almost none of its aliveness while talking is the mouth. It is the
    // brows lifting on emphasis, the head punctuating, the eyes moving —
    // all of it keyed off the same stress the voice is putting on the
    // words. We already receive that signal 25x/second (audioLevelBus);
    // it was only ever wired to the jaw.
    const emphasis = { current: 0 };
    // A listener's nod — the small "go on, I'm with you" dip of the head
    // people make while someone else is mid-sentence. Runs only while Mona
    // is actually talking (see attentionBus) and never while Amin is.
    const backchannel = { value: 0, timer: 1.5 + Math.random() };
    // Smoothed emotional posture (jaw drop, head pitch, head tilt), eased
    // on the same per-emotion clock as the blendshapes so the whole face
    // and head arrive together rather than the brows leading the body.
    const posture = { jaw: 0, pitch: 0, tilt: 0 };
    // This frame's smoothed value per expression blendshape (brows, mouth
    // shape — never blink/gaze/jaw/viseme, which stay owned by the logic
    // below). Persists across frames within this one mount so each morph
    // eases toward its target instead of snapping.
    const expressionCurrent = new Map<string, number>(ALL_EXPRESSION_NAMES.map((name) => [name, 0]));
    // This frame's smoothed value per viseme, so each mouth shape eases in
    // and out instead of snapping on the frame its letter starts.
    const visemeCurrent = new Map<string, number>(VISEME_NAMES.map((name) => [name, 0]));
    const clock = new THREE.Clock();

    function resize() {
      const w = container!.clientWidth || 1;
      const h = container!.clientHeight || 1;
      renderer.setSize(w, h, false);
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
    }
    const resizeObserver = new ResizeObserver(resize);
    resizeObserver.observe(container);

    const loader = new GLTFLoader();
    loader.load(
      MODEL_URL,
      (gltf) => {
        if (disposed) return;
        const root = gltf.scene;
        scene.add(root);

        root.traverse((obj) => {
          if ((obj as THREE.SkinnedMesh).isSkinnedMesh && FACE_MESH_NAMES.includes(obj.name)) {
            faceMeshes.push(obj as THREE.SkinnedMesh);
          }
        });
        // The cornea meshes are static (no morph targets) and sit in front
        // of the eyelid's closed position — Meshy's export never gave the
        // eyeball a matching "retreat" shape, so a real, correctly-applied
        // eyeBlink morph (verified against the source .glb's own vertex
        // data — this isn't a guess) is otherwise invisible, fully
        // occluded by the static eyeball. Hiding the cornea exactly when
        // the lid should be covering it fixes what Meshy's rig left
        // broken, without touching or re-exporting the model.
        leftCornea = root.getObjectByName("AvatarLeftCornea") ?? null;
        rightCornea = root.getObjectByName("AvatarRightCornea") ?? null;
        headBone = root.getObjectByName("Head") ?? null;
        leftEyeBone = root.getObjectByName("LeftEye") ?? null;
        rightEyeBone = root.getObjectByName("RightEye") ?? null;
        if (headBone) headBaseQuat = headBone.quaternion.clone();
        if (leftEyeBone) leftEyeBaseQuat = leftEyeBone.quaternion.clone();
        if (rightEyeBone) rightEyeBaseQuat = rightEyeBone.quaternion.clone();

        if (faceMeshes.length === 0) {
          onFailure("الملف اتحمّل لكن مفيش meshes بها morph targets — فحصي public/models/amin_facial_rig.glb");
        }

        // Real bug from a real Mac screenshot (2026-08-28, Mona: "أنا بنيت
        // ليك جسم كامل ليه انت دمرت الشكل كده" — I built you a full body,
        // why did you destroy the shape): this rig's rest pose is a T-pose
        // (arms out to the sides), and it ships with no idle-standing
        // animation to fix that. Cropping around it was never going to
        // hold at every window size; posing it properly does.
        //
        // Axis/angle found by direct inspection, not guessed twice: a first
        // attempt rotated around each bone's LOCAL Z axis, which (Mixamo
        // bone-local axes don't line up with world axes) actually swung
        // both forearms to cross in front at the waist — confirmed by
        // temporarily pulling the camera back to a full-body view and
        // screenshotting the actual result rather than assuming the first
        // guess was right. Local X by 90°, same sign for both arms, is
        // what actually brings them down to a natural at-the-sides stance
        // (verified the same way — full-body screenshot showing both hands
        // resting near the hips). Re-confirmed at three bust-crop aspect
        // ratios afterward (including a deliberately extreme 1800×650)
        // with no artifacts at any of them.
        const leftArm = root.getObjectByName("LeftArm");
        const rightArm = root.getObjectByName("RightArm");
        const armDropAngle = Math.PI / 2;
        if (leftArm) {
          leftArm.quaternion.multiply(new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(1, 0, 0), armDropAngle));
        }
        if (rightArm) {
          rightArm.quaternion.multiply(
            new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(1, 0, 0), armDropAngle),
          );
        }

        // Frame a bust shot (head + shoulders), not the full body this
        // skeleton also carries — matches the tight crop the static
        // portrait already used in this same slot.
        if (headBone) {
          // Distance/lookAt tuned empirically (Playwright screenshots
          // against the real model, not guessed): the previous 0.62/24°
          // combination put barely 0.26m of vertical extent in frame —
          // less than head height alone — so it cropped off the top of
          // the head and, per Mona's real Mac screenshot, the shoulders
          // too. 0.85 frames hairline-to-collar with a little headroom,
          // confirmed against the model's actual bounding box (head bone
          // at y≈1.524, hair top at y≈1.698) rather than the full
          // arms-out rest pose this rig has no idle animation to fix —
          // framing tight enough to crop that out is also what keeps it
          // from reading as a scarecrow pose.
          const headWorldPos = new THREE.Vector3();
          headBone.getWorldPosition(headWorldPos);
          camera.position.set(headWorldPos.x, headWorldPos.y, headWorldPos.z + 0.85);
          camera.lookAt(headWorldPos.x, headWorldPos.y, headWorldPos.z);
        } else {
          const box = new THREE.Box3().setFromObject(root);
          const center = box.getCenter(new THREE.Vector3());
          const size = box.getSize(new THREE.Vector3()).length();
          camera.position.set(center.x, center.y, center.z + size * 0.6);
          camera.lookAt(center);
        }
        resize();
      },
      undefined,
      (err) => onFailure(`تعذّر تحميل الموديل: ${String(err)}`),
    );

    function animate() {
      rafId = requestAnimationFrame(animate);
      const dt = Math.min(clock.getDelta(), 0.1);
      const t = clock.elapsedTime;
      const currentState = stateRef.current;
      const isSpeaking = currentState === "speaking";
      const isThinking = currentState === "thinking" || currentState === "planning";
      const isListening = currentState === "listening" || currentState === "armed";

      // --- Blink (idle timer, not tied to state or audio) ---
      blink.timer -= dt;
      if (blink.phase === "waiting" && blink.timer <= 0) {
        blink.phase = "closing";
        blink.timer = 0.09;
      } else if (blink.phase === "closing") {
        blink.value = Math.min(1, blink.value + dt / 0.09);
        if (blink.timer <= 0) {
          blink.phase = "opening";
          blink.timer = 0.11;
        }
      } else if (blink.phase === "opening") {
        blink.value = Math.max(0, blink.value - dt / 0.11);
        if (blink.timer <= 0) {
          blink.phase = "waiting";
          if (blink.double) {
            // Second half of a double blink: back almost immediately.
            blink.double = false;
            blink.timer = 0.12;
          } else {
            // People blink markedly more while they talk and while they
            // are being talked to; a flat idle rate reads as a mannequin
            // staring. Roughly 2x the rate when speaking.
            const base = isSpeaking ? 1.3 : isListening ? 2.5 : 2.9;
            const spread = isSpeaking ? 1.6 : isListening ? 2.2 : 3.5;
            blink.timer = base + Math.random() * spread;
            blink.double = Math.random() < 0.28;
          }
        }
      }
      setMorph(faceMeshes, "eyeBlinkLeft", blink.value);
      setMorph(faceMeshes, "eyeBlinkRight", blink.value);
      if (leftCornea) leftCornea.visible = blink.value < 0.6;
      if (rightCornea) rightCornea.visible = blink.value < 0.6;

      // --- Eye saccades (real bones) ---
      // These were smooth glides over roughly a third of a second, which
      // no eye has ever done: a real saccade is ballistic, ~40ms, and the
      // time is spent FIXATED between them. That slow drift is a large
      // part of why this face read as animation rather than as someone
      // present — the reference footage's eyes snap and hold.
      //
      // While thinking, gaze also breaks away and wanders wider (looking
      // "up and away" is what people do while composing an answer); while
      // speaking or listening it stays near the viewer with small,
      // frequent shifts, which is where eye contact actually lives.
      gaze.timer -= dt;
      if (gaze.timer <= 0) {
        if (isThinking) {
          gaze.targetX = (Math.random() - 0.5) * 0.7;
          gaze.targetY = 0.18 + Math.random() * 0.3;
          gaze.timer = 0.8 + Math.random() * 1.4;
        } else {
          // Mostly small shifts around the viewer, occasionally a bigger
          // glance away — an unbroken stare is as unsettling as a drift.
          // While Mona is mid-sentence the glances away are suppressed and
          // the shifts shrink: looking at someone is how a face says it is
          // listening, and looking around is how it says it is not.
          const listeningNow = attentionRef.current > 0.35;
          const big = !listeningNow && Math.random() < 0.22;
          const spread = listeningNow ? 0.1 : big ? 0.6 : 0.22;
          gaze.targetX = (Math.random() - 0.5) * spread;
          gaze.targetY = (Math.random() - 0.5) * (listeningNow ? 0.07 : big ? 0.32 : 0.14);
          gaze.timer = (isSpeaking ? 0.45 : 0.7) + Math.random() * (big ? 1.6 : 1.1);
        }
      }
      // ~40ms to reach the new fixation instead of ~300ms of gliding.
      const saccade = 1 - Math.pow(1e-22, dt);
      gaze.x = lerp(gaze.x, gaze.targetX, saccade);
      gaze.y = lerp(gaze.y, gaze.targetY, saccade);
      const eyeYaw = gaze.x * 0.3;
      const eyePitch = gaze.y * 0.2;
      if (leftEyeBone) {
        leftEyeBone.quaternion
          .copy(leftEyeBaseQuat)
          .multiply(new THREE.Quaternion().setFromEuler(new THREE.Euler(eyePitch, eyeYaw, 0)));
      }
      if (rightEyeBone) {
        rightEyeBone.quaternion
          .copy(rightEyeBaseQuat)
          .multiply(new THREE.Quaternion().setFromEuler(new THREE.Euler(eyePitch, eyeYaw, 0)));
      }

      // --- Speech emphasis envelope ---
      // Fast attack (a stressed syllable should register immediately),
      // slow release (so the face settles rather than flickering at
      // syllable rate). Everything expressive below keys off this.
      const rawLevel = isSpeaking ? getAudioLevel() : 0;
      const emphasisTarget = Math.min(1, Math.sqrt(rawLevel) * 1.15);
      emphasis.current =
        emphasisTarget > emphasis.current
          ? lerp(emphasis.current, emphasisTarget, 1 - Math.pow(1e-9, dt))
          : lerp(emphasis.current, emphasisTarget, 1 - Math.pow(0.02, dt));

      // --- Emotional posture (jaw + head), eased with the expression ---
      const postureTarget = EMOTION_POSTURE[emotionRef.current ?? "neutral"] ?? {};
      const postureOnset = EMOTION_ONSET_SECONDS[emotionRef.current ?? "neutral"] ?? 0.3;
      const pDamp = 1 - Math.pow(0.01, dt / postureOnset);
      const pDecay = 1 - Math.pow(0.01, dt / EMOTION_DECAY_SECONDS);
      const easePosture = (cur: number, tgt: number) =>
        lerp(cur, tgt, Math.abs(tgt) > Math.abs(cur) ? pDamp : pDecay);
      // The jaw half only applies when Amin is silent: while he talks the
      // mouth belongs to the viseme track and an emotion must not fight
      // it (that contradiction is exactly the bug VISEME_JAW_CEILING
      // exists to prevent).
      posture.jaw = easePosture(posture.jaw, isSpeaking ? 0 : (postureTarget.jaw ?? 0));
      posture.pitch = easePosture(posture.pitch, postureTarget.pitch ?? 0);
      posture.tilt = easePosture(posture.tilt, postureTarget.tilt ?? 0);

      // --- Attention: Mona is mid-sentence ---
      // Decays over ~1.5s of silence, so the face settles when she pauses
      // instead of flipping between two poses. Only meaningful while Amin
      // is listening; his own speech clears it.
      const attn = isListening ? attention() : 0;
      attentionRef.current = attn;

      // Backchannel nod: a small dip of the head every couple of seconds
      // while she is actually saying something. This is the single most
      // recognisable thing a listening face does, and its absence is a
      // large part of why Amin read as a screen rather than someone
      // paying attention.
      if (attn > 0.35) {
        backchannel.timer -= dt;
        if (backchannel.timer <= 0) {
          backchannel.value = 1;
          backchannel.timer = 1.8 + Math.random() * 1.6;
        }
      } else {
        backchannel.timer = Math.min(backchannel.timer, 0.9);
      }
      // One dip, ~0.4s, then back.
      backchannel.value = Math.max(0, backchannel.value - dt / 0.4);
      const nodDip = Math.sin(backchannel.value * Math.PI) * 0.03 * attn;

      // --- Head (real Head bone) ---
      // The idle sway stays as it was — two slow sines, under ~2 degrees.
      // What is new is that the head now PUNCTUATES: a small chin-down
      // nod on each stressed syllable, and a slight turn that follows the
      // eyes so gaze and head don't disagree. Both are what a person does
      // without noticing, and their absence is most of the "electronic"
      // look Mona is pointing at in the reference footage.
      if (headBone) {
        const thinkTilt = (isThinking ? 0.05 : 0) + posture.tilt;
        const swayY = Math.sin(t * 0.55) * 0.018 + Math.sin(t * 0.21 + 1) * 0.01;
        const swayX = Math.sin(t * 0.37 + 2) * 0.012;
        // Chin down on emphasis (positive X pitches the head forward on
        // this rig, same axis the arm-drop fix used).
        const nod = emphasis.current * 0.035 + nodDip + posture.pitch;
        // Head follows gaze at about a fifth of the eyes' amplitude —
        // enough to read as one movement, not enough to swing the face.
        const follow = gaze.x * 0.12;
        headBone.quaternion
          .copy(headBaseQuat)
          .multiply(new THREE.Quaternion().setFromEuler(new THREE.Euler(swayX + nod, swayY + follow, thinkTilt)));
      }

      // --- Mouth: amplitude-reactive while speaking, closed otherwise ---
      // RECALIBRATED 2026-08-28 from a real screenshot: at the previous
      // 0.55 ceiling (jawOpen moves this rig's jaw ~35mm at 1.0, the
      // single largest blendshape it has) any sustained loudness pinned
      // the mouth at a full gape — tongue visible, more scream than
      // speech. Unlike the brow/cheek expressions, which needed BOOSTING
      // past 0.5 to read at all (their deltas are ~8mm), the jaw needs
      // the opposite treatment for the same reason: real conversational
      // mouths barely reach a third of a full jaw drop. sqrt() keeps
      // quiet syllables visible without letting loud ones slam the cap.
      // WIDENED 0.30 -> 0.42, but only together with the noise gate below,
      // which is what makes that safe. The 0.55 that once looked like a
      // scream was not too high a PEAK — it was pinned near the top the
      // whole time, because a flat sqrt() of a continuous loudness signal
      // never comes back down between words. Gating the quiet third to
      // zero restores the closures that separate one word from the next,
      // and only real stressed syllables now reach the ceiling. Range
      // where speech actually lives, instead of a permanent half-open
      // mouth.
      const gated = rawLevel <= 0.12 ? 0 : (rawLevel - 0.12) / 0.88;
      // Loudness still drives the JAW — how far the mouth opens really is
      // a function of how much sound is coming out — but it no longer
      // decides the SHAPE. That comes from the viseme track below.
      const loudnessJaw = isSpeaking ? Math.min(1, Math.sqrt(gated) * 1.15) * 0.42 : 0;
      // The shape currently on the face caps how far the jaw may drop —
      // see VISEME_JAW_CEILING. Without a track (the REST fallback path)
      // there is no shape to respect, so loudness has the jaw to itself.
      const activeCue = currentViseme();
      const jawCeiling =
        hasVisemeTrack() && activeCue ? (VISEME_JAW_CEILING[activeCue.viseme] ?? 0.42) : 1;
      const targetJaw = loudnessJaw * jawCeiling;
      const targetSil = isSpeaking && !hasVisemeTrack() ? Math.max(0, 1 - targetJaw * 2.6) : 0;
      // Asymmetric, like a jaw: opens fast, closes a little slower, but
      // both quick enough that a syllable is a distinct movement rather
      // than a smear. The old symmetric damping blurred adjacent
      // syllables into one continuous hover.
      const jawDamp = targetJaw > jaw.current ? 1 - Math.pow(1e-7, dt) : 1 - Math.pow(1e-5, dt);
      jaw.current = lerp(jaw.current, targetJaw, jawDamp);
      sil.current = lerp(sil.current, targetSil, 1 - Math.pow(1e-5, dt));
      // An emotion can open the mouth too — Ameca's shock drops the jaw at
      // the same instant the brows go up, and it is the coordination that
      // sells it. Additive, and zero while speaking (see posture.jaw).
      setMorph(faceMeshes, "jawOpen", Math.min(1, jaw.current + posture.jaw));

      // --- Mouth SHAPE: the actual letters being spoken ---
      // What was here before was a sine wobble crossfading viseme_aa and
      // viseme_oh in time with the volume. It had no relationship to the
      // words at all: a loud "م", which is lips pressed shut, produced a
      // wide-open "aa" — the mouth was flapping near some audio rather
      // than forming speech. Mona's brief was that the lips should look
      // like the words are coming out of them, and that is not reachable
      // from a loudness signal, however it is tuned.
      //
      // This rig carries the full Oculus viseme set (verified against the
      // .glb's own morphTargetDictionary: sil/PP/FF/TH/DD/kk/CH/SS/nn/RR/
      // aa/E/ih/oh/ou), and ElevenLabs hands us the exact millisecond
      // every character is voiced at. So each shape now goes on the face
      // when its letter is actually said.
      const active = activeCue;
      for (const name of VISEME_NAMES) {
        // Ease in over the first third of the hold and back out over the
        // last third, so consecutive shapes flow into each other. Snapping
        // between them reads as a puppet.
        let target = 0;
        if (active && name === active.viseme && isSpeaking) {
          const p = active.progress;
          target = p < 0.33 ? p / 0.33 : p > 0.72 ? Math.max(0, (1 - p) / 0.28) : 1;
        }
        const cur = visemeCurrent.get(name) ?? 0;
        // Fast, but not instant — roughly a 45ms move, which is about how
        // quickly a real articulator gets to its target.
        visemeCurrent.set(name, lerp(cur, target, 1 - Math.pow(1e-14, dt)));
        setMorph(faceMeshes, name, visemeCurrent.get(name) ?? 0);
      }
      // Silence shape only when there is no real track to follow (the REST
      // fallback path carries no timings — see commands::speak_text).
      if (!hasVisemeTrack()) {
        setMorph(faceMeshes, "viseme_sil", sil.current);
        const wobble = 0.5 + 0.5 * Math.sin(t * 9);
        setMorph(faceMeshes, "viseme_aa", jaw.current * wobble * 0.6);
        setMorph(faceMeshes, "viseme_oh", jaw.current * (1 - wobble) * 0.5);
      }

      // --- Facial expression: emotion + cognitive state, on real brow/
      // mouth-shape blendshapes (see EMOTION_EXPRESSIONS/STATE_EXPRESSIONS
      // above) — every other ARKit target this rig carries besides the
      // blink/gaze/jaw/viseme ones already driven above. Eased toward its
      // target rather than snapped, same damping style as the mouth.
      const expressionTargets = combineExpressions(
        EMOTION_EXPRESSIONS[emotionRef.current ?? "neutral"] ?? {},
        STATE_EXPRESSIONS[currentState],
        isSpeaking,
      );
      // Brows ride ON TOP of whatever the emotion/state expression is
      // doing, added after its slow easing rather than mixed into it —
      // an emphasis lift has to be as fast as the syllable that caused
      // it, and the expression damping is deliberately slow. This is the
      // single largest contributor to a talking face looking alive: in
      // the reference footage the brows are never still while the mouth
      // is moving, and here they were completely motionless.
      const browLift = emphasis.current;
      const speechBrows: Record<string, number> = isSpeaking
        ? {
            browInnerUp: browLift * 0.34,
            browOuterUpLeft: browLift * 0.42,
            // Very slightly less on the right: perfectly symmetric brows
            // are another thing real faces never do.
            browOuterUpRight: browLift * 0.36,
          }
        : attn > 0
          ? {
              // Listening: an open, attentive brow that lifts while she is
              // actually speaking and settles when she stops. Much smaller
              // than the speech lift — a listener's face is interested,
              // not startled, and this rig bares the whites of the eyes
              // past a low threshold (see STATE_EXPRESSIONS' armed note).
              browInnerUp: attn * 0.22,
              browOuterUpLeft: attn * 0.16,
              browOuterUpRight: attn * 0.13,
            }
          : {};
      // Onset is per-emotion and always faster than decay — see
      // EMOTION_ONSET_SECONDS. `1 - 0.01^(dt/seconds)` reaches ~99% of the
      // target in `seconds`, so these numbers mean what they say.
      const onsetSeconds = EMOTION_ONSET_SECONDS[emotionRef.current ?? "neutral"] ?? 0.3;
      const onsetDamp = 1 - Math.pow(0.01, dt / onsetSeconds);
      const decayDamp = 1 - Math.pow(0.01, dt / EMOTION_DECAY_SECONDS);
      for (const name of ALL_EXPRESSION_NAMES) {
        const target = expressionTargets.get(name) ?? 0;
        const previous = expressionCurrent.get(name) ?? 0;
        const current = lerp(previous, target, target > previous ? onsetDamp : decayDamp);
        expressionCurrent.set(name, current);
        setMorph(faceMeshes, name, Math.min(1, current + (speechBrows[name] ?? 0)));
      }
      // browInnerUp/browOuterUp* may not appear in ALL_EXPRESSION_NAMES if
      // no emotion or state map happens to use them, in which case the
      // loop above never writes them — apply those directly so the lift
      // isn't silently dropped.
      for (const [name, value] of Object.entries(speechBrows)) {
        if (!ALL_EXPRESSION_NAMES.includes(name)) setMorph(faceMeshes, name, value);
      }

      renderer.render(scene, camera);
    }
    animate();

    return () => {
      disposed = true;
      cancelAnimationFrame(rafId);
      resizeObserver.disconnect();
      scene.traverse((obj) => {
        const mesh = obj as THREE.Mesh;
        if (mesh.geometry) mesh.geometry.dispose();
        const material = (obj as THREE.Mesh).material;
        if (Array.isArray(material)) material.forEach((m) => m.dispose());
        else if (material) material.dispose();
      });
      renderer.dispose();
      renderer.forceContextLoss();
      if (renderer.domElement.parentElement === container) {
        container.removeChild(renderer.domElement);
      }
    };
    // Deliberately mount once — `state` and `emotion` changes are read
    // every frame via stateRef/emotionRef so a new turn never re-triggers
    // a full model reload.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return <div ref={containerRef} className={className} />;
}
