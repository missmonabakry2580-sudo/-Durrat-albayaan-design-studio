# Amin's push-to-talk transcriber (macOS helper)

**Status: written, never compiled or run.** This was written in a Linux
sandbox with no macOS, no Xcode, and no microphone — see `main.swift`'s
header comment for the full detail. This README is the checklist for the
first time it's built on a real Mac.

## 1. Build it

```bash
cd macos/transcriber
swiftc -O main.swift -o amin-transcriber
```

Fix whatever the compiler says first — this is genuinely unverified Swift,
and a first-pass compile error or two would not be surprising.

## 2. Test it standalone, before wiring it into Amin

```bash
./amin-transcriber
```

It should prompt for microphone and speech-recognition permission the
first time (macOS's permission dialog). Say something, then type `stop`
and press Enter. Watch for JSON lines like:

```json
{"type":"partial","text":"..."}
{"type":"final","text":"..."}
```

**If no permission prompt appears, or it silently fails:** this is the
known open risk flagged in `main.swift` — a standalone CLI binary may not
get TCC (privacy permission) prompts the way code running inside a signed
`.app` bundle does. Don't spend hours on it here; the likely fix is moving
this logic in-process into the main Rust/Tauri binary via a Swift↔Rust FFI
bridge instead of a separate child process. Report back with what actually
happened (prompt shown? denied? no prompt at all?) so we can decide.

## 3. Wire it into Amin

Once step 2 works:

1. Copy `amin-transcriber` into this repo somewhere Tauri can bundle it
   (or leave it here and reference it by relative path).
2. Add a `resources` entry to `bundle` in `src-tauri/tauri.conf.json`,
   e.g. `"resources": ["../macos/transcriber/amin-transcriber"]`.
3. Run `npm run tauri dev` and hold the mic button (or `alt+A`) in the app.
   `src-tauri/src/voice.rs` already looks for the binary at the Tauri
   resource path and will say plainly if it can't find it.

## Known limitations to revisit, not silently work around

- **Single locale (`ar-EG`).** `SFSpeechRecognizer` doesn't do free
  Arabic/English code-switching the way a human listener does. If mixed
  speech comes out garbled or wrong-language, that's this limitation, not
  a bug — worth a real product conversation about whether that's
  acceptable for now or needs a different approach.
- **On-device vs. server recognition.** The code prefers on-device
  recognition (`requiresOnDeviceRecognition`) per Amin's "voice stays
  local" principle, but falls back silently to Apple's server-based
  recognition if on-device isn't available for `ar-EG` on this OS
  version. Worth confirming which one actually ran (Apple doesn't make
  this obvious at the API level) before calling the privacy story done.
