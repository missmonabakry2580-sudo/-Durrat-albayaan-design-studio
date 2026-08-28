#!/usr/bin/env node
// Validates a .glb file against Mona's exact requirement: every mesh's morph
// targets, read back out of the exported file itself (not trusted from
// whatever 3D tool produced it), must together cover all 52 ARKit
// blendshapes and all 15 Oculus/OVRLipSync visemes by exact, case-sensitive
// name. See docs/ARCHITECTURE.md's "Realtime voice" section for why this
// exists before any facial rig work starts: there is currently no 3D model
// of Amin in this repository at all (see the BLOCKER report already given),
// so this tool has nothing real to check yet — it exists so that the moment
// a commissioned model arrives, checking it is a one-command, evidence-based
// answer instead of a visual guess.
//
// Reads the glTF JSON chunk directly (glb-json.mjs) rather than going
// through three.js/GLTFLoader, so it runs in plain Node with no WebGL
// context and no CDN dependency — see this project's own recent
// experience hitting a blocked CDN host from a sandboxed environment.
// Morph target names are read from `mesh.extras.targetNames`, the exact
// field three.js's own GLTFLoader (updateMorphTargets) reads to build
// `morphTargetDictionary` — confirmed against GLTFLoader's source, not
// assumed.

import { readGlbJson } from "./glb-json.mjs";
import { REQUIRED_ARKIT, REQUIRED_VISEMES } from "./required-targets.mjs";

// Duplicates only matter *within* one mesh's own target list — that's what
// would make glTF's per-primitive index-to-name mapping ambiguous. The same
// name reused across different meshes (e.g. "browDownLeft" on both the head
// and the eyelashes, so the lashes track brow movement) is the standard,
// correct pattern for a multi-mesh face rig: three.js builds a separate
// morphTargetDictionary per mesh, so there's no collision. Checking this
// list-wide instead of per-mesh was this validator's own bug, never caught
// because no real multi-mesh model existed to test it against until now.
function findDuplicates(meshes) {
  const duplicates = new Set();
  for (const mesh of meshes) {
    if (!mesh.targetNames) continue;
    const seen = new Set();
    for (const name of mesh.targetNames) {
      if (seen.has(name)) duplicates.add(name);
      seen.add(name);
    }
  }
  return [...duplicates];
}

/// Extracts every mesh's morph target names from the glTF JSON, keyed by
/// mesh name. A mesh contributes to the report only if it declares morph
/// targets at all (mesh.primitives[].targets) — meshes with no targets
/// (a torso, a prop) are irrelevant to this check and skipped.
function extractMeshMorphTargets(gltf) {
  const meshes = gltf.meshes ?? [];
  const byMesh = [];
  meshes.forEach((mesh, index) => {
    const targetCounts = (mesh.primitives ?? [])
      .map((p) => (p.targets ?? []).length)
      .filter((n) => n > 0);
    if (targetCounts.length === 0) return; // this mesh has no morph targets

    const targetNames = mesh.extras?.targetNames;
    const name = mesh.name ?? `mesh[${index}]`;
    byMesh.push({
      name,
      targetCount: Math.max(...targetCounts),
      targetNames: Array.isArray(targetNames) ? targetNames : null,
    });
  });
  return byMesh;
}

export function validateFacialRig(path) {
  const gltf = readGlbJson(path);
  const meshes = extractMeshMorphTargets(gltf);

  if (meshes.length === 0) {
    return {
      pass: false,
      totalMorphTargets: 0,
      meshes: [],
      arkitFound: [],
      arkitMissing: [...REQUIRED_ARKIT],
      visemesFound: [],
      visemesMissing: [...REQUIRED_VISEMES],
      duplicates: [],
      extraCustomMorphs: [],
      notes: ["No mesh in this file declares any morph targets at all."],
    };
  }

  const notes = [];
  let allNames = [];
  for (const mesh of meshes) {
    if (!mesh.targetNames) {
      notes.push(
        `Mesh "${mesh.name}" has ${mesh.targetCount} morph target(s) but no ` +
          `extras.targetNames — glTF stores morph targets by index only here, so ` +
          `they cannot be matched by name (and three.js's own GLTFLoader would not ` +
          `build a morphTargetDictionary for this mesh either). This is a real export ` +
          `problem to fix at the source, not something this script can work around.`,
      );
      continue;
    }
    if (mesh.targetNames.length !== mesh.targetCount) {
      notes.push(
        `Mesh "${mesh.name}" declares ${mesh.targetCount} morph target(s) but ` +
          `extras.targetNames has ${mesh.targetNames.length} name(s) — mismatched, ` +
          `so name-to-index mapping for this mesh is unreliable.`,
      );
    }
    allNames.push(...mesh.targetNames);
  }

  const duplicates = findDuplicates(meshes);
  const nameSet = new Set(allNames);

  const arkitFound = REQUIRED_ARKIT.filter((n) => nameSet.has(n));
  const arkitMissing = REQUIRED_ARKIT.filter((n) => !nameSet.has(n));
  const visemesFound = REQUIRED_VISEMES.filter((n) => nameSet.has(n));
  const visemesMissing = REQUIRED_VISEMES.filter((n) => !nameSet.has(n));

  const requiredSet = new Set([...REQUIRED_ARKIT, ...REQUIRED_VISEMES]);
  const extraCustomMorphs = [...new Set(allNames)].filter((n) => !requiredSet.has(n));

  const pass =
    arkitMissing.length === 0 &&
    visemesMissing.length === 0 &&
    duplicates.length === 0 &&
    notes.length === 0;

  return {
    pass,
    totalMorphTargets: allNames.length,
    meshes,
    arkitFound,
    arkitMissing,
    visemesFound,
    visemesMissing,
    duplicates,
    extraCustomMorphs,
    notes,
  };
}

function formatReport(path, r) {
  const lines = [];
  lines.push(`FILE:`, path, "");
  lines.push(`MESHES WITH MORPH TARGETS:`);
  if (r.meshes.length === 0) {
    lines.push("  (none)");
  } else {
    for (const m of r.meshes) {
      lines.push(`  - ${m.name}: ${m.targetCount} target(s)`);
    }
  }
  lines.push("");
  lines.push(`TOTAL MORPH TARGETS:`, String(r.totalMorphTargets), "");
  lines.push(
    `ARKIT:`,
    `${r.arkitFound.length} / ${REQUIRED_ARKIT.length} ${r.arkitMissing.length === 0 ? "PASS" : "FAIL"}`,
    "",
  );
  lines.push(
    `OCULUS VISEMES:`,
    `${r.visemesFound.length} / ${REQUIRED_VISEMES.length} ${r.visemesMissing.length === 0 ? "PASS" : "FAIL"}`,
    "",
  );
  lines.push(`MISSING:`, r.arkitMissing.length + r.visemesMissing.length === 0
    ? "NONE"
    : [...r.arkitMissing, ...r.visemesMissing].join(", "), "");
  lines.push(`DUPLICATES:`, r.duplicates.length === 0 ? "NONE" : r.duplicates.join(", "), "");
  lines.push(
    `UNEXPECTED TARGETS (kept, not required):`,
    r.extraCustomMorphs.length === 0 ? "NONE" : r.extraCustomMorphs.join(", "),
    "",
  );
  if (r.notes.length > 0) {
    lines.push("NOTES:");
    for (const n of r.notes) lines.push(`  - ${n}`);
    lines.push("");
  }
  lines.push(`RESULT:`, r.pass ? "PASS" : "FAIL");
  return lines.join("\n");
}

// Only run as a CLI when invoked directly (`node validate-facial-rig.mjs
// file.glb`) — importable as a module (validateFacialRig) for the test
// fixture script and any future automated check.
if (import.meta.url === `file://${process.argv[1]}`) {
  const path = process.argv[2];
  if (!path) {
    console.error("Usage: node validate-facial-rig.mjs <path-to.glb>");
    process.exit(2);
  }
  try {
    const result = validateFacialRig(path);
    console.log(formatReport(path, result));
    process.exit(result.pass ? 0 : 1);
  } catch (e) {
    console.error(`ERROR: ${e.message}`);
    process.exit(2);
  }
}
