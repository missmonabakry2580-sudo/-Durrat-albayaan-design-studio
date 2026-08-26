# Amin's push-to-talk voice engine (macOS helper)

**Status: written, compiled successfully in CI, never run against a real
microphone.** This was written in a Linux sandbox with no macOS, no Xcode,
and no microphone — see `AminVoice.swift`'s header comment for the full
detail.

## Why this is a library, not a standalone helper

An earlier version was a standalone executable, spawned as a child
process from `src-tauri/src/voice.rs` and talked to over stdin/stdout JSON
lines. On a real Mac, Mona hit "couldn't start the audio engine" — the
exact failure this file's header had flagged as a known open risk before
it was tried: a spawned CLI binary may not cleanly inherit the signed
`.app`'s microphone/speech TCC (privacy permission) grant.

The fix was moving this logic in-process: `AminVoice.swift` now compiles
to a small dylib, loaded and called directly by Amin's own Rust binary
(via `dlopen`/`dlsym`, the `libloading` crate — see `voice.rs`) instead of
being spawned as a separate process. Every AVFoundation/Speech call now
executes as the same process macOS prompts for microphone/speech access,
closing the TCC-identity gap a subprocess had.

## How it's built

CI (`.github/workflows/build-macos.yml`) compiles this for both
architectures and lipo-combines them into one universal dylib:

```bash
cd macos/transcriber
swiftc -O -target arm64-apple-macosx13.0 -emit-library -parse-as-library \
  -module-name AminVoice AminVoice.swift -o libaminvoice-arm64.dylib
swiftc -O -target x86_64-apple-macosx13.0 -emit-library -parse-as-library \
  -module-name AminVoice AminVoice.swift -o libaminvoice-x86_64.dylib
lipo -create libaminvoice-arm64.dylib libaminvoice-x86_64.dylib -output libaminvoice.dylib
```

The result is placed at `src-tauri/libaminvoice.dylib`, which
`tauri.conf.json`'s `bundle.resources` picks up and bundles into the
`.app`. If the compile fails, CI bundles a placeholder that reports a
clear in-app error instead of failing the whole release — check that
workflow step's log first if voice doesn't work.

There is deliberately no manual "run it standalone from a terminal" step
anymore: `-parse-as-library` means this file has no executable entry
point of its own. The only way to exercise it is through the running
Amin app (`npm run tauri dev`, hold the mic button or `alt+A`) on a real
Mac — `src-tauri/src/voice.rs` looks for the dylib at the Tauri resource
path and says plainly if it can't find or load it.

## What to check the first time this runs on a real Mac

1. Holding the mic button (or `alt+A`) should trigger macOS's microphone
   and speech-recognition permission prompts the first time.
2. If a prompt appears and is granted, watch for `voice://partial` /
   `voice://final` events reaching the chat input as you speak, and
   `voice://error` if something goes wrong (see `AminVoice.swift`'s error
   strings for what each one means).
3. If **no prompt appears at all**, or it fails silently even after
   granting permission — report back exactly what happened (prompt
   shown? denied? console/log output?) rather than assuming this fix
   didn't work; there may be a second, different problem to diagnose.

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
