import { readFileSync } from "node:fs";

/// Reads just the JSON chunk out of a binary .glb file — no rendering, no
/// WebGL, no three.js dependency. A .glb is: a 12-byte header
/// (magic/version/length), then one or more chunks of
/// [chunkLength:u32][chunkType:u32][chunkData]. Chunk type 0x4E4F534A is
/// "JSON" (ASCII "JSON" read little-endian); the first chunk in a
/// conformant file is always the JSON chunk, but this walks all chunks
/// rather than assuming that, since a validator that trusts file-order
/// blindly is exactly the kind of "looks right" shortcut Mona explicitly
/// ruled out.
const GLB_MAGIC = 0x46546c67; // "glTF"
const CHUNK_TYPE_JSON = 0x4e4f534a; // "JSON"

export function readGlbJson(path) {
  const buf = readFileSync(path);
  if (buf.length < 12) {
    throw new Error(`'${path}' is too small to be a valid .glb file (${buf.length} bytes)`);
  }
  const magic = buf.readUInt32LE(0);
  if (magic !== GLB_MAGIC) {
    throw new Error(`'${path}' is not a .glb file (bad magic number)`);
  }
  const totalLength = buf.readUInt32LE(8);
  if (totalLength > buf.length) {
    throw new Error(
      `'${path}' is truncated: header declares ${totalLength} bytes, file has ${buf.length}`,
    );
  }

  let offset = 12;
  while (offset + 8 <= totalLength) {
    const chunkLength = buf.readUInt32LE(offset);
    const chunkType = buf.readUInt32LE(offset + 4);
    const chunkStart = offset + 8;
    const chunkEnd = chunkStart + chunkLength;
    if (chunkEnd > totalLength) {
      throw new Error(`'${path}' has a corrupt chunk (declared length runs past end of file)`);
    }
    if (chunkType === CHUNK_TYPE_JSON) {
      const jsonText = buf.toString("utf8", chunkStart, chunkEnd);
      return JSON.parse(jsonText);
    }
    offset = chunkEnd;
  }
  throw new Error(`'${path}' has no JSON chunk`);
}
