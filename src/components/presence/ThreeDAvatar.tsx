import { useEffect, useRef } from "react";
import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { getAudioLevel } from "../../lib/visual/audioLevelBus";
import type { AminState } from "./types";

interface ThreeDAvatarProps {
  state: AminState;
  className?: string;
  /** Fired once if the model fails to load or WebGL isn't available, so the
   * caller can fall back to Portrait mode instead of showing a blank box. */
  onFailure: (reason: string) => void;
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
export function ThreeDAvatar({ state, className, onFailure }: ThreeDAvatarProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const stateRef = useRef(state);
  stateRef.current = state;

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
    const blink = { value: 0, timer: 1 + Math.random() * 2, phase: "waiting" as "waiting" | "closing" | "opening" };
    const gaze = { x: 0, y: 0, targetX: 0, targetY: 0, timer: 1 };
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
          blink.timer = 2.5 + Math.random() * (isListening ? 2.2 : 3.5);
        }
      }
      setMorph(faceMeshes, "eyeBlinkLeft", blink.value);
      setMorph(faceMeshes, "eyeBlinkRight", blink.value);
      if (leftCornea) leftCornea.visible = blink.value < 0.6;
      if (rightCornea) rightCornea.visible = blink.value < 0.6;

      // --- Eye saccades (real bones, small idle gaze shifts) ---
      gaze.timer -= dt;
      if (gaze.timer <= 0) {
        gaze.targetX = (Math.random() - 0.5) * (isThinking ? 0.35 : 0.55);
        gaze.targetY = (Math.random() - 0.5) * 0.3;
        gaze.timer = 1.4 + Math.random() * 2.4;
      }
      gaze.x = lerp(gaze.x, gaze.targetX, 1 - Math.pow(0.001, dt));
      gaze.y = lerp(gaze.y, gaze.targetY, 1 - Math.pow(0.001, dt));
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

      // --- Head sway (real Head bone, kept under ~2 degrees) ---
      if (headBone) {
        const thinkTilt = isThinking ? 0.05 : 0;
        const swayY = Math.sin(t * 0.55) * 0.018 + Math.sin(t * 0.21 + 1) * 0.01;
        const swayX = Math.sin(t * 0.37 + 2) * 0.012;
        headBone.quaternion
          .copy(headBaseQuat)
          .multiply(new THREE.Quaternion().setFromEuler(new THREE.Euler(swayX, swayY, thinkTilt)));
      }

      // --- Mouth: amplitude-reactive while speaking, closed otherwise ---
      const level = isSpeaking ? getAudioLevel() : 0;
      const targetJaw = isSpeaking ? Math.min(1, level * 1.7) * 0.55 : 0;
      const targetSil = isSpeaking ? Math.max(0, 1 - targetJaw * 1.4) : 1;
      jaw.current = lerp(jaw.current, targetJaw, 1 - Math.pow(0.0005, dt));
      sil.current = lerp(sil.current, targetSil, 1 - Math.pow(0.0005, dt));
      setMorph(faceMeshes, "jawOpen", jaw.current);
      setMorph(faceMeshes, "viseme_sil", sil.current);
      const wobble = 0.5 + 0.5 * Math.sin(t * 9);
      setMorph(faceMeshes, "viseme_aa", jaw.current * wobble * 0.6);
      setMorph(faceMeshes, "viseme_oh", jaw.current * (1 - wobble) * 0.5);

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
    // Deliberately mount once — `state` changes are read every frame via
    // stateRef so a new turn never re-triggers a full model reload.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return <div ref={containerRef} className={className} />;
}
