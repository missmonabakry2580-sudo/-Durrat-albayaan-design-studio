// The exact 52 ARKit blendshape names and 15 Oculus/OVRLipSync viseme names
// Mona specified, transcribed verbatim (including her required case —
// "viseme_PP" not "Viseme_PP" or "PP"). This file is the single source of
// truth both the validator and any future FacialRigController/VisemeMapper
// must import from, so the required list is never re-typed and drifting
// copies can't disagree with each other.

export const REQUIRED_ARKIT = [
  "browDownLeft", "browDownRight", "browInnerUp", "browOuterUpLeft", "browOuterUpRight",
  "cheekPuff", "cheekSquintLeft", "cheekSquintRight",
  "eyeBlinkLeft", "eyeBlinkRight",
  "eyeLookDownLeft", "eyeLookDownRight",
  "eyeLookInLeft", "eyeLookInRight",
  "eyeLookOutLeft", "eyeLookOutRight",
  "eyeLookUpLeft", "eyeLookUpRight",
  "eyeSquintLeft", "eyeSquintRight",
  "eyeWideLeft", "eyeWideRight",
  "jawForward", "jawLeft", "jawOpen", "jawRight",
  "mouthClose",
  "mouthDimpleLeft", "mouthDimpleRight",
  "mouthFrownLeft", "mouthFrownRight",
  "mouthFunnel",
  "mouthLeft",
  "mouthLowerDownLeft", "mouthLowerDownRight",
  "mouthPressLeft", "mouthPressRight",
  "mouthPucker",
  "mouthRight",
  "mouthRollLower", "mouthRollUpper",
  "mouthShrugLower", "mouthShrugUpper",
  "mouthSmileLeft", "mouthSmileRight",
  "mouthStretchLeft", "mouthStretchRight",
  "mouthUpperUpLeft", "mouthUpperUpRight",
  "noseSneerLeft", "noseSneerRight",
  "tongueOut",
];

export const REQUIRED_VISEMES = [
  "viseme_sil", "viseme_PP", "viseme_FF", "viseme_TH", "viseme_DD",
  "viseme_kk", "viseme_CH", "viseme_SS", "viseme_nn", "viseme_RR",
  "viseme_aa", "viseme_E", "viseme_ih", "viseme_oh", "viseme_ou",
];

if (REQUIRED_ARKIT.length !== 52) {
  throw new Error(`REQUIRED_ARKIT must have exactly 52 entries, has ${REQUIRED_ARKIT.length}`);
}
if (REQUIRED_VISEMES.length !== 15) {
  throw new Error(`REQUIRED_VISEMES must have exactly 15 entries, has ${REQUIRED_VISEMES.length}`);
}
