// Builds two SYNTHETIC .glb files purely to prove validate-facial-rig.mjs
// itself works correctly — neither is a real 3D asset (no real geometry,
// no real Amin model; there isn't one yet, see the BLOCKER report). Each
// is just a minimal, spec-valid glTF JSON chunk (no BIN chunk — optional
// per the glTF spec, and the validator never reads geometry data) with
// meshes[].primitives[].targets of the right length and
// meshes[].extras.targetNames set, which is exactly and only what the
// validator inspects.
//
// fixtures/complete.glb   — declares all 52 ARKit + 15 Oculus names correctly → expect PASS
// fixtures/incomplete.glb — missing 3 ARKit names and has one duplicate name → expect FAIL

import { writeFileSync, mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { REQUIRED_ARKIT, REQUIRED_VISEMES } from "./required-targets.mjs";

const __dirname = dirname(fileURLToPath(import.meta.url));
const fixturesDir = join(__dirname, "fixtures");
mkdirSync(fixturesDir, { recursive: true });

function buildGlb(targetNames) {
  const gltf = {
    asset: { version: "2.0", generator: "amin-facial-rig-test-fixture (synthetic, not a real asset)" },
    meshes: [
      {
        name: "AminFace_TEST_FIXTURE",
        primitives: [
          {
            attributes: { POSITION: 0 },
            targets: targetNames.map(() => ({ POSITION: 0 })),
          },
        ],
        extras: { targetNames },
      },
    ],
  };

  const jsonText = JSON.stringify(gltf);
  // Pad the JSON chunk to a 4-byte boundary with spaces, as the glTF
  // binary spec requires (0x20 is the mandated JSON padding byte).
  const pad = (4 - (jsonText.length % 4)) % 4;
  const jsonBuf = Buffer.concat([Buffer.from(jsonText, "utf8"), Buffer.alloc(pad, 0x20)]);

  const header = Buffer.alloc(12);
  header.writeUInt32LE(0x46546c67, 0); // "glTF"
  header.writeUInt32LE(2, 4); // version 2
  const totalLength = 12 + 8 + jsonBuf.length;
  header.writeUInt32LE(totalLength, 8);

  const chunkHeader = Buffer.alloc(8);
  chunkHeader.writeUInt32LE(jsonBuf.length, 0);
  chunkHeader.writeUInt32LE(0x4e4f534a, 4); // "JSON"

  return Buffer.concat([header, chunkHeader, jsonBuf]);
}

const complete = buildGlb([...REQUIRED_ARKIT, ...REQUIRED_VISEMES]);
writeFileSync(join(fixturesDir, "complete.glb"), complete);

const incompleteNames = [...REQUIRED_ARKIT, ...REQUIRED_VISEMES]
  .filter((n) => !["tongueOut", "noseSneerLeft", "noseSneerRight"].includes(n)) // drop 3 ARKit names
  .concat(["viseme_aa"]); // add a duplicate of an existing viseme name
const incomplete = buildGlb(incompleteNames);
writeFileSync(join(fixturesDir, "incomplete.glb"), incomplete);

console.log("Wrote synthetic test fixtures (not real Amin assets):");
console.log("  scripts/facial-rig/fixtures/complete.glb   — expect PASS");
console.log("  scripts/facial-rig/fixtures/incomplete.glb — expect FAIL (3 missing, 1 duplicate)");
