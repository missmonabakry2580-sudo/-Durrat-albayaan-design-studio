# Amin — Architecture

Amin is a personal Executive AI Agent. Its core — the agent loop, the
memory, the local database, all policy enforcement — lives in exactly one
place: the **macOS desktop app**. It is not a chatbot. A later phase adds a
Mobile Companion (see "Mobile Companion" below), but that is a thin remote
control into this same core, never a second brain, never published to the
App Store. Its loop is:

> Observe → Understand → Decide within policy → Execute → Follow up → Report

This document started as the Phase 0 baseline and is extended, not
rewritten, as later phases land — each phase's section says which phase
added it.

## Why desktop-only, why Tauri

- **The core is desktop-only, and never on a public store.** No Google Play,
  ever; no App Store listing for the Mac app or the Mobile Companion,
  ever — both install personally (see "Mobile Companion" below). Amin holds
  standing access to email, calendar, files, and (later) a browser session —
  that is not something to ship through a public app-store review process.
  `src-tauri/src/lib.rs` has no mobile entry point at all; there is no
  `gen/android` or `gen/ios` in *this* repo (the Mobile Companion, once
  built, is a separate, much smaller client project — see below), and
  neither app is ever built for general distribution.
- **Tauri over Electron**: a Rust backend for anything privileged (secrets,
  the local database, outbound network calls, the future browser/file/Gmail
  tools), and a React/TypeScript webview for everything visual. The
  webview's Content-Security-Policy (`tauri.conf.json` → `app.security.csp`)
  sets `connect-src 'self'` — **the frontend cannot reach the network at
  all**. Every external call funnels through a Rust command, which is where
  policy enforcement and audit logging actually happen.
- **SQLite, on-device, no sync.** `src-tauri/schema.sql` is the whole
  schema. There is no server, no cloud database, no telemetry backend.

## Desktop shell: menu bar, not Dock

Amin is meant to be a standing presence, not an app you open and quit —
closer to a menu-bar utility than a document window. `src-tauri/src/tray.rs`
adds the tray icon and its "Show Amin" / "Quit Amin" menu; `lib.rs` sets
macOS's activation policy to `Accessory` (no Dock icon, no app-switcher
entry) and intercepts the window's close button to hide rather than quit —
only "Quit Amin" from the tray actually exits the process. This is the
Phase 0 shell groundwork; Phase 1 fills it with the actual voice pipeline
below.

### Availability model: push-to-talk by default, hands-free opt-in

Push-to-talk (a mic-button tap or the `alt+A` shortcut arms one utterance)
is the default and always available. Continuous "say a phrase to open, say
a phrase to close" hands-free listening (see "Hands-free mode" below) is a
later addition Mona explicitly opts into from Settings — off by default,
never turned on silently, because it means the microphone stays open
continuously rather than only while a key is held. The two are mutually
exclusive at runtime (see `voice::VoiceSession`/`voice::HandsFreeSession`'s
`is_active()` guards in `commands.rs` and `lib.rs`) since both would
otherwise fight over the same native audio engine.

## Repo layout

```
src/                      React + TypeScript frontend (the webview)
  styles/tokens.css        design tokens: color, type, spacing, motion
  styles/global.css        base resets, applies tokens
  components/orb/          the Living AI Core (see Design System below)
  lib/tauri.ts             the ONLY place the frontend calls invoke() —
                            typed wrappers around every Rust command
  App.tsx                  app shell: orb preview, Talk-to-Amin panel,
                            security panel, audit log, About

src-tauri/                 Rust backend
  schema.sql                the local SQLite schema (settings, audit_log,
                             tasks, follow_ups)
  src/db.rs                 opens the on-device DB, applies schema.sql
  src/secrets.rs            OS Keychain wrapper (via the `keyring` crate) —
                             the only place a secret is ever read or written
  src/policy.rs             risk tiers, autonomy levels, the excluded-domain
                             list (banking, payments, ...)
  src/audit.rs              append-only audit log writer
  src/agent.rs              Agent Core: the Anthropic API client
  src/voice.rs              push-to-talk session: loads and calls the
                             native voice engine in-process (see macos/)
  src/tasks.rs              local task CRUD + Quick Capture
  src/files.rs              file access, confined to ~/Documents/Amin
  src/browser.rs            opens a URL in Amin's own isolated browser
                             window
  src/followups.rs          local Follow-up Engine (creation, escalation,
                             due-listing)
  src/notify.rs             native OS notifications — the Follow-up
                             Engine's one real delivery channel so far
  src/brief.rs              local Delta Brief (tasks/follow-ups/audit
                             activity — no Gmail/Calendar data yet)
  src/commands.rs           every #[tauri::command] the frontend can call
  src/tray.rs               menu-bar tray icon + Show/Quit menu
  src/lib.rs                wires plugins, DB, tray, global shortcut, and
                             commands together

macos/transcriber/          native Speech-framework voice engine (dylib,
                             loaded in-process) — NOT verified, see its
                             own README before touching voice.rs

docs/                       this file, SECURITY.md
.env.local.example          dev-only convenience; see SECURITY.md
```

## Agent Core (Phase 1)

`src-tauri/src/agent.rs` is Amin's first real capability: `send_message()`
calls the Anthropic Messages API (`claude-opus-5` — see the constant at the
top of the file if that default ever needs revisiting) with the Keychain-
stored key, using a system prompt that states the operating loop and, just
as importantly, states plainly what Amin *cannot* do yet — no tools, no
email/calendar/browser/files — so it never confabulates having taken an
action. The `send_agent_message` command in `commands.rs` wraps this with
the two things that must never be skipped: a kill-switch check before
calling out, and an audit-log entry after, on both success and failure.

Amin keeps a session-scoped conversation memory (`agent::Conversation`, a
capped in-memory list of turns — see `MAX_HISTORY_MESSAGES` in `agent.rs`)
so short exchanges hold context; it resets on app restart or the "New
conversation" action and is never written to disk. This is *not* the
long-term memory the roadmap has in mind for later phases (persistence,
summarization/compaction of long histories) — it's the minimum needed for
"what did I just say" to work today. Tool use is still follow-on work once
this path is proven out. The response-parsing logic (text extraction,
refusal handling, history trimming) has unit tests in `agent.rs` against
sample API JSON,
since that's the part most likely to have a subtle bug and the one part of
this phase fully testable without live credentials or macOS.

### Voice pipeline: what's real vs. what needs a Mac

The brief asks Amin to "evaluate ElevenLabs vs. OpenAI Realtime" for voice,
but it also said only the Anthropic key is needed *now* — everything else
later. Adding a second/third vendor key before that's actually asked for
would contradict that. So the voice path defaults to **macOS's on-device
Speech framework** (no new API key, no new recurring cost, audio stays
on-device by default) rather than a cloud STT vendor. The ElevenLabs/
OpenAI Realtime evaluation is real and still owed — it happens when Mona
is ready to add a voice-provider key, not assumed here.

**Built and verified (compiles, runs, unit-tested in this sandbox):**

- `src-tauri/src/voice.rs`: manages one push-to-talk `VoiceSession` — loads
  `libaminvoice.dylib` in-process (via `dlopen`/`dlsym`, the `libloading`
  crate) the first time listening starts, calls straight into it, and
  forwards the partial/final/error events its C callback receives as
  `voice://partial` / `voice://final` / `voice://error`. If the dylib isn't
  present or fails to load, it fails with a clear error rather than doing
  nothing silently — that path itself is exercised and correct even
  without a real engine.
- The same engine also speaks Amin's replies aloud (`speak_text`/
  `amin_voice_speak`, macOS's on-device `AVSpeechSynthesizer`, `ar-SA`
  voice) — Mona asked for spoken output as a priority, not just text in
  the chat log. `voice://speaking-started` / `voice://speaking-finished`
  let the frontend track real speaking state instead of guessing a
  duration. Same single-locale caveat as recognition: a reply with
  English words mixed in will be read with an Arabic accent.
- A global `alt+A` push-to-talk shortcut (`tauri-plugin-global-shortcut`,
  registered in `lib.rs`), armed even when Amin's window isn't focused —
  registers cleanly and doesn't crash the app (verified under a virtual
  X11 display; real macOS key-combo behavior is unverified). The key
  combo is a placeholder — change it in `lib.rs` if it turns out to
  collide with something.
- Frontend: a mic button (hold to talk) plus the same global shortcut
  both drive `App.tsx`'s voice event listeners, which fill the message
  box with the transcript for review rather than auto-sending it — since
  the recognition accuracy is itself unverified, sending unreviewed text
  straight to the API isn't the right default yet.

**Written and compiling in CI — never run against a real microphone:**
`macos/transcriber/AminVoice.swift`, a Speech-framework voice engine
(`SFSpeechRecognizer` + `AVAudioEngine`, following Apple's documented
live-recognition pattern). There is no macOS, no Xcode, and no
microphone in this development sandbox to exercise it against a real
mic. It used to be a standalone executable spawned as a child process;
Mona hit "couldn't start the audio engine" on a real Mac, which matched
this file's own previously-flagged open risk — a spawned CLI binary may
not cleanly inherit microphone/speech TCC (privacy permission) prompts
the way code inside the signed `.app` bundle does. The fix applied:
`AminVoice.swift` now builds as a dylib that Amin's own process loads
in-process instead of spawning as a separate executable — see
`macos/transcriber/README.md` for the build details and what to check
the first time this runs on a real Mac.

Speaker recognition (the voice-print / presence-greeting feature) is a
separate step after the above is proven out, with its own real model
choice to make (an on-device speaker-embedding model, e.g. an
ECAPA-TDNN-style network run via an embedded ONNX Runtime) — a case of
the "radical architecture change" the brief says to surface, not decide
silently, and one best made once there's a Mac to validate it on.

### Hands-free mode: wake phrase / close phrase

Mona asked for Amin to be reachable without pressing anything — say a
phrase, Amin listens, say a phrase (or just go quiet) when done. Built as
`HandsFreeListener` in `macos/transcriber/AminVoice.swift`, wired through
`voice::start_hands_free`/`stop_hands_free` in `voice.rs` and
`get_hands_free_settings`/`save_hands_free_phrases`/`set_hands_free_mode`
in `commands.rs`. Same unverified-until-a-real-Mac status as the rest of
this file's voice pipeline — written from documented APIs, never run
against a real microphone.

Two phases, cycled for as long as the mode is on:

- **Armed (passive).** Watches only for the wake phrase. Hard-requires
  on-device recognition (`requiresOnDeviceRecognition` forced `true`,
  regardless of what the OS would otherwise pick) — if on-device isn't
  available for the locale, hands-free mode refuses to start rather than
  silently streaming continuous audio to Apple's servers just to watch for
  a phrase. Nothing here reaches the frontend as a transcript.
- **Active (a command session).** Opens once the wake phrase is heard;
  every finalized utterance is sent as a normal final transcript, and
  `App.tsx` auto-sends it to the agent (rather than just filling the input
  box, which is what the same event does for manual push-to-talk) as long
  as the session is open. Ends when the close phrase is heard (stripped out
  of whatever it's appended to) or `stop_hands_free` is called explicitly;
  a natural pause between utterances does *not* end it — that's what lets
  a multi-turn exchange happen without repeating the wake phrase.

Disclosed trade-off, not hidden: a spoken phrase is a shared secret, not an
identity check — anyone in earshot who knows it can open a session. That's
exactly the concern Mona raised herself; voice-print/speaker verification
(above) is the real fix and is intentionally still a separate, not-yet-built
phase, not folded into this one.

## Code signing: the real fix for repeated permission prompts

Every `.dmg` so far has been unsigned (ad-hoc) — no Apple Developer
certificate. Mona reproduced, on video, that this is the actual cause of
the microphone/speech permission dialog reappearing even *within one
running app session*, not just across new downloads: an unsigned build
has no stable identity for macOS's TCC (privacy permission) system to
remember a grant against, so it can end up split across two entries
("amin" the raw binary name, "Amin" the display name — visible directly
in System Settings → Privacy & Security → any of the mic/speech/screen
panels) that don't reliably stay in sync with each other. No amount of
checking `authorizationStatus()` correctly in Swift fixes this — the
problem is upstream of that code, in what identity the OS has to check
against at all.

Mona has an active Apple Developer Program membership, so the real fix —
proper code signing plus notarization — is now just a setup step, not a
future maybe. `.github/workflows/build-macos.yml` already reads six
secrets (`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`,
`APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`)
that `tauri-action` uses to sign and notarize automatically when present;
they're unset today, so builds stay exactly as unsigned as before until
she adds them. Once she does:

1. **Team ID** — developer.apple.com/account → Membership details.
2. **A "Developer ID Application" certificate** — via Keychain Access's
   Certificate Assistant (Request a Certificate from a Certificate
   Authority) to generate a CSR, upload it at
   developer.apple.com/account/resources/certificates/add choosing
   "Developer ID Application", download the issued certificate, and
   double-click it to install into Keychain (it pairs with the private
   key the CSR step created). Export that identity from Keychain Access
   as a `.p12` file with a password, then base64-encode it
   (`base64 -i cert.p12 -o cert_base64.txt`) — that text is
   `APPLE_CERTIFICATE`, the export password is
   `APPLE_CERTIFICATE_PASSWORD`, and `APPLE_SIGNING_IDENTITY` is the
   certificate's full name as Keychain shows it (e.g. "Developer ID
   Application: Mona AlSayed (TEAMID)").
3. **An app-specific password** for notarization — appleid.apple.com →
   Sign-In and Security → App-Specific Passwords → generate one. That's
   `APPLE_PASSWORD`; `APPLE_ID` is the Apple ID email itself.
4. Add all six as repository secrets (GitHub → Settings → Secrets and
   variables → Actions → New repository secret) — never paste any of
   them into chat or a committed file.

Once those exist, the very next push builds a properly signed,
notarized `.dmg`: no more Gatekeeper "unidentified developer" warning,
and — the actual point — one stable identity, so a permission grant
finally sticks for good instead of splitting or resetting.

**First real attempt with the secrets in place failed notarization** —
worth recording since it's a genuine gotcha, not a config mistake: Apple
notarizes by scanning *every* Mach-O binary inside the `.app`, not just
the main executable. `libaminvoice.dylib` is bundled via plain
`bundle.resources` in `tauri.conf.json`, which tauri-bundler copies as-is
without signing — so it reached Apple unsigned even though the app
around it was signed correctly. The workflow now has its own "Sign the
voice engine dylib" step, right after the dylib is built and before
tauri-action runs, that imports the certificate into a short-lived
keychain, signs the dylib directly with `codesign --options runtime
--timestamp`, then deletes that keychain immediately — kept deliberately
separate from tauri-action's own keychain for the main app so codesign
never sees the same identity registered twice at once ("ambiguous
identity").

**Confirmed working in build 0.2.2** (workflow run 33001899302, 2026-08-26):
the dylib-signing step and tauri-action's own notarization both succeeded,
with the notary log showing `Notarizing Finished with status Accepted` and
`Stapling app...`. This is the first real signed-and-notarized `.dmg` — no
more Gatekeeper warning, and one stable identity for TCC to remember
permission grants against.

**But 0.2.2 traded the old mic bug for a new one**: notarization requires
signing with the Hardened Runtime (`codesign --options runtime`), and
Hardened Runtime blocks microphone access outright — no system permission
prompt at all — unless the app explicitly carries the
`com.apple.security.device.audio-input` entitlement. `NSMicrophoneUsageDescription`
in `Info.plist` isn't enough on its own once Hardened Runtime is in play;
that string controls what a prompt *says*, this entitlement controls
whether the OS will show one *at all*. Mona hit this immediately in 0.2.2:
pressing push-to-talk gave "microphone access was not granted" instantly,
with no dialog, even right after a `tccutil reset Microphone` — because
unsigned/ad-hoc builds never engage Hardened Runtime, this never surfaced
before real signing landed. Fixed by adding `src-tauri/entitlements.plist`
(just that one entitlement) and pointing `tauri.conf.json`'s
`bundle.macOS.entitlements` at it, from 0.2.4 on. The dylib itself needs no
entitlements of its own — it runs inside the signed host process's
context once loaded via `dlopen`, so only the main executable's
entitlements matter here.

## In-app updates: no more manual delete-and-redownload

Every fix from 0.2.2 through 0.2.6 required the same manual dance: Mona
downloads a new `.dmg`, deletes the old `.app`, drags the new one in. She
pushed back on this directly — reasonably, since ordinary apps just update
themselves. Fixed with `tauri-plugin-updater` (`src/lib/updater.ts`,
wired into `App.tsx`): on launch, Amin checks the endpoint configured in
`tauri.conf.json`'s `plugins.updater.endpoints` for a newer signed build,
and shows a banner with a "حدّثي الآن" button that downloads and installs
it in place, then relaunches — no manual download or deleting anything.

This uses a *second*, separate signing keypair from the Apple Developer ID
certificate above — a Tauri/minisign keypair whose only job is proving an
update bundle came from this pipeline, unrelated to Apple or notarization.
Generated once with `npx @tauri-apps/cli signer generate`; the public half
is committed in `tauri.conf.json` (public keys are meant to be public),
the private half lives only as the `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` repo secrets (same never-in-chat rule
as the Apple secrets). Without them, `tauri-action` builds every artifact
as before but silently skips producing `latest.json` — this is what the
build log's "Signature not found for the updater JSON. Skipping upload"
line was, on every build before 0.2.7.

One deliberate wrinkle: the updater endpoint points at
`releases/download/amin-preview/latest.json` — the specific tag — rather
than GitHub's `releases/latest/download/...` alias. That alias only ever
resolves to the newest *non-prerelease* release, and this workflow
publishes every build with `prerelease: true` (see the release notes'
own "نسخة تجريبية" wording); pointed at `/latest/`, the updater would
never find anything.

The banner only appears when a newer build actually exists — on the
newest build itself there is nothing to offer, which reads as "there's no
update button" if that's not expected. Settings also has an explicit
"تحقّقي من التحديثات الآن" button (`handleCheckForUpdate` in `App.tsx`) so
Mona can confirm that for herself on demand instead of just trusting a
silent background check.

### A real, silent failure of this whole mechanism (2026-08-28)

`tauri-plugin-updater`'s `check()` decides "is there an update" purely by
comparing semver strings — `tauri.conf.json`'s `version` against
`latest.json`'s — never by commit hash or build timestamp. `version`
stayed at `0.2.14` across five real, substantive pushes (3D avatar mode,
Portrait Mode's new photo, the Simli integration, and the critical CSP
texture fix above) because nothing enforced bumping it. Every one of
those builds compiled and published correctly, but every "تحقّقي من
التحديثات الآن" click and every launch-time check honestly, correctly
reported "already up to date" — because as far as semver is concerned,
nothing had changed. Mona was stuck on an old binary with no signal
anything was wrong; this is very likely why several fixes across this
whole session (not just today's) never visibly reached her.

Fixed two ways, not just bumped once:
1. Version bumped to `0.2.15` (`package.json`, `src-tauri/Cargo.toml`,
   `src-tauri/tauri.conf.json` — all three, kept in sync as always).
2. A new CI step, **"Refuse to ship a build whose version didn't
   change"**, runs first, before any of the expensive build steps: it
   downloads the currently-published `latest.json`, compares its
   `version` against this push's `tauri.conf.json`, and fails the whole
   job loudly (`::error::`) if they match — rather than silently
   shipping a build the updater will never offer. A one-time fix without
   this guard is just next week's repeat of the same bug.

**A real gap this guard has, found by auditing its own first day of use**:
it only compares against the *currently published* `latest.json` — it has
no way to see another run's in-flight, not-yet-published version. The very
next commit after this one (the `setVoiceProcessingEnabled` revert) was
pushed with its version accidentally still at `0.2.15` — the same mistake
this guard exists to catch — but its check happened to run while `0.2.15`'s
own build was still mid-flight, before that one had published. It compared
against the still-older previously-published version, saw a difference,
and passed. No real harm here (the two commits are linearly ordered, so
the later one's release is a strict superset of the earlier one's code —
whichever finished publishing last, `0.2.16`, is what's actually live), but
it's worth naming plainly rather than quietly relying on luck: two pushes
within the same few minutes, both forgetting to bump, can both pass this
guard. The actual protection is still "bump the version every push" as a
habit; this guard catches forgetting after the fact, not a race between
two simultaneous forgettings.

## Realtime voice: the architecture decision, and why it isn't ElevenAgents

Mona's real ask isn't "Amin can speak" — it's a natural back-and-forth
voice conversation: no send button, automatic turn detection, and
barge-in (she can interrupt Amin mid-sentence by talking). The current
pipeline (push-to-talk or hands-free → full text reply → full audio file
generated, then played) cannot do any of that; it's fundamentally
request/response, not a live conversation.

ElevenLabs sells exactly this as a product — "ElevenAgents"
(Conversational AI) — and it's the obvious first thing to reach for.
**Deliberately not using it**, for a concrete reason found by reading
their own docs, not a guess: both ElevenAgents' Custom LLM bridge and the
lower-level Speech Engine product require ElevenLabs' cloud to reach a
server *you host and keep running* — Custom LLM is an outbound HTTP/SSE
call to a publicly reachable OpenAI-compatible endpoint, Speech Engine
needs its Server SDK attached to a server of yours that receives
transcripts and returns your LLM's reply. Amin has been a 100% local,
no-hosted-backend desktop app by design this entire project (see "Why
desktop-only, why Tauri" above); adopting either product means standing
up and paying for a permanent public server, with its own security
surface, just for voice — a real, recurring cost and risk, not a
config flag.

**What's used instead** — ElevenLabs' plain streaming TTS WebSocket,
`wss://api.elevenlabs.io/v1/text-to-speech/{voice_id}/stream-input`,
which needs only the `xi-api-key` header Mona already has: no agent to
configure, no server to host. This becomes the target replacement for
`elevenlabs::synthesize`'s current request → wait for the whole MP3 →
play flow: Claude's reply streams into this socket as it's generated and
audio plays back incrementally, cutting time-to-first-sound instead of
waiting for the full reply. Claude via the Anthropic API stays exactly
where it already is — no bridge, no second server, no change to who the
"brain" is.

Turn detection and barge-in (interrupting Amin mid-reply by speaking) get
built directly into the existing local pipeline — the mic tap in
AminVoice.swift's `HandsFreeListener` already runs continuously; the plan
is to keep watching it for genuine new speech while a reply is playing
and, on real speech onset, stop playback and open a fresh recognition
session immediately, rather than requiring the manual "stop speaking"
button. **Not yet implemented as of this doc update.**

**What has actually landed** (`elevenlabs::synthesize_streaming`, unit
tested, wired into `commands::speak_text` as the first thing tried before
the old REST call as a fallback): the WebSocket connection itself — every
ElevenLabs reply now goes out over `stream-input` and its audio chunks
are decoded as they arrive. **What has not landed yet**: this function
still waits for the last chunk (`isFinal`) before returning, so Mona
does not yet hear audio start before the whole reply is ready, and
nothing about turn detection or barge-in exists yet. Concretely: the
network path changed and is tested; the actual "starts talking before
it's done thinking" and "she can interrupt it" experience she asked for
is still ahead. Do not read "streaming WebSocket added" as "realtime
conversation works" — they are not the same claim.

## The command surface is the whole contract

The frontend never gets raw SQL access and never talks to the network
directly. It calls the typed functions in `src/lib/tauri.ts`, which invoke
named Rust commands in `src-tauri/src/commands.rs`. This is deliberate: it
means every capability Amin's UI has is enumerable by reading one file on
each side, and every invariant (audit log is append-only, secrets never
touch disk unencrypted, banking is unreachable) is enforced in Rust, not
trusted to whatever the webview happens to do.

Phase 0 ships a small, honest set of commands: app info, API-key
present/save/clear, autonomy level get/set, kill switch, risk classification,
and audit log read. Nothing here calls an LLM or touches the network yet —
that begins in Phase 1 (Agent Core).

## Design system: Deep Navy / Soft Gold / Ivory / Charcoal

Tokens live in `src/styles/tokens.css`; components read the semantic
aliases (`--bg-app`, `--accent`, `--text-primary`, ...), never the raw
palette. Typography is system fonts only (`ui-serif` for display, the
platform sans stack for body) — no web fonts, no network fetch for
rendering, which also means the CSP never needs a `font-src` exception.

### The Living AI Core (Orb)

`src/components/orb/Orb.tsx` is Amin's one visual presence — a halo, a
rotating ring, and a core, all pure CSS animation driven by a `state` prop
(`OrbState`, in `types.ts`): `idle, listening, thinking, planning,
executing, speaking, success, warning, waiting`. The rule for every future
integration: **the orb must reflect a real state**, never spin just to look
busy. `App.tsx` currently includes a state switcher for visual QA; it comes
out once real state wiring (voice input, agent execution) lands in later
phases and drives the orb itself.

### Creator attribution

Added 2026-08-25 per the brief: Amin visibly credits its creator, Mona
AlSayed — not as an incidental brand name, but as a deliberate signature.
`src/lib/branding.ts` is the single source of truth for the wording
(`CREATOR_ATTRIBUTION_AR/EN`, `EXPORT_SIGNATURE_AR/EN`); every surface below
reads from it rather than inlining its own text:

- **Launch screen** — `src/components/splash/Splash.tsx` shows the orb,
  the "أمين" title, and the attribution line beneath it for a couple of
  seconds (or until tapped) before the main app appears.
- **About panel** — the "حول أمين" section in `App.tsx` states it again,
  alongside the app name/version. Phase 0 has no real navigation yet, so
  this lives inline in the single-screen shell; once a proper Settings/
  navigation surface exists, About becomes its own screen rather than a
  scroll-down section.
- **Exports** — no report/document export exists yet (that starts around
  Phase 3's Morning Brief and the Phase 6 developer/reporting workflows).
  When one is built, it appends `EXPORT_SIGNATURE_AR`/`EN` from
  `branding.ts` — recorded here now so the convention isn't reinvented
  per-feature later.

## Phase 1 design notes: speaker recognition & presence greeting

Captured here ahead of Phase 1 implementation, from the 2026-08-25 brief
addendum, so the requirements aren't lost:

- **Speaker voice recognition.** Amin enrolls a voice print (a numeric
  embedding, not a stored recording) for Mona specifically, and matches
  incoming audio against it locally. It should pick her voice out of a
  room with other people talking, without needing a wake word aimed at it.
  Audio that doesn't match is discarded immediately — never stored, never
  sent anywhere, never analyzed further. See docs/SECURITY.md §13 for the
  privacy contract this implies; the actual model/library choice (e.g. an
  on-device speaker-embedding model run via a local ONNX/CoreML runtime)
  is a Phase 1 decision to make and record here once picked, since it
  affects app size and licensing — a case of the "radical architecture
  change" the brief says to surface rather than decide silently.
- **Presence-triggered greeting.** Recognizing Mona's voice as she arrives
  (not a fixed wake phrase) triggers a natural greeting, then an
  arrival-triggered Delta Brief (what changed, what needs a decision) —
  the same content model as the Morning Brief (Phase 3), just triggered by
  presence instead of time of day. The very first time Amin notices her
  present in a session, it says something like "أنا هنا عشانك، منساكيش."
- **Push-to-talk first.** See "Availability model" above — this is the
  Phase 1 default; always-listening is explicitly a later milestone.

## Mobile Companion

Added 2026-08-25, from the brief addendum. iPhone should work as a remote
control for the same Amin — talk to it, see its status, give it
instructions — while away from the Mac, not just standing in front of it.
Two decisions were made explicitly with Mona rather than assumed, because
both touch rules stated as non-negotiable in docs/SECURITY.md:

- **Connectivity: a private VPN mesh (Tailscale/WireGuard-style), not a
  cloud relay.** The Mac gets no public inbound port and no third-party
  server ever sees Amin's data or "brain" state — only Mona's own devices,
  talking to each other over an encrypted private network interface. This
  updates, rather than breaks, SECURITY.md §2's "no inbound ports": the
  rule is now specifically "no *public* inbound port, no exposure to the
  general internet" — see SECURITY.md §2 for the precise wording. The
  concrete VPN product (Tailscale vs. a self-hosted WireGuard) is an
  implementation choice for whichever phase builds this, not decided here.
- **Distribution: personal install via Mona's own Apple Developer account
  (sideload/Ad Hoc), never TestFlight or the App Store.** Same reasoning
  as the Mac app never being listed publicly — this stays a private tool
  built for one person, not a shipped product.
- **One brain, not two.** The phone is a thin client: it renders Amin's
  state and relays her voice/commands to the Mac over the VPN link, and
  the Mac's SQLite database and audit log remain the single source of
  truth. The phone does not keep its own copy of tasks, follow-ups, or the
  audit log that could drift out of sync or leak if the phone is lost —
  at most a transient, non-sensitive display cache.
- This is a distinct client codebase from this repo (Tauri targets desktop
  only — see "Why desktop-only, why Tauri" above), built once Phase 1's
  Agent Core exists for it to remote-control. Its own architecture notes
  land in that project when it starts, not retrofitted here.

## Phase 2 design notes: task management shipped; browser/files still ahead

Started 2026-08-26, with Mona commuting and asking for Phase 2 to begin
without her, and everything needing her Mac/review lined up for when she
opens it. Given that constraint, this pass deliberately picked the one
Phase 2 item with no new privileged surface to reason about:

**Built and tested (unit tests + a clean app launch — same bar as every
other phase so far):**

- `src-tauri/src/tasks.rs`: local task CRUD (create, list with an optional
  status filter, status transitions) over the `tasks` table that's existed
  since Phase 0's schema. `create_task`/`quick_capture`/`list_tasks`/
  `set_task_status` commands wrap it with the same audit-log pattern as
  every other command — every task action is logged as `RiskTier::Auto`
  (creating or completing your own task is low-stakes; see `policy.rs` if
  that classification ever needs revisiting).
- **Quick Capture** is the same `tasks::create` with `source:
  "quick_capture"` — no separate code path to keep in sync. The "📌
  Capture" button next to the Talk-to-Amin input saves whatever text is
  there as a task *instead of* sending it to the agent, which also means
  it already works with the push-to-talk voice box (speak → text lands in
  the input → Capture) even though the voice pipeline itself is still
  unverified — the text path doesn't care where the words came from.

**Update, same night:** Mona explicitly asked for Phase 2 (and the phases
after it) to keep going without her — "شغلي كله اللي عليا" ("do everything
that's on me too"). That's the go-ahead this section was waiting for, so
both items below got built rather than held. Both still ship with a
conservative default that's meant to be *reacted to*, not treated as the
final word — the point of surfacing them was to get a real proposal in
front of her quickly, not to avoid building them.

- **File access** — `src-tauri/src/files.rs`. Originally scoped to one
  dedicated folder, `~/Documents/Amin`; broadened the same morning to
  Mona's whole home directory at her own explicit request — see "Tool use
  and the confirmation gate" below for why, and for how that broader scope
  is now gated. See the "Repo layout" section above for the containment
  design and its tests, including a real symlink-escape attempt — that
  containment logic itself didn't change, only the root it's checked
  against.
- **Browser control** — `src-tauri/src/browser.rs`, chosen as the
  lowest-risk real slice of "browser control": a single reused
  `WebviewWindow`, isolated via Tauri's own `data_directory` builder
  option (its own cookie/storage directory under the app's data folder,
  never Mona's real browser profile) rather than automating an external
  browser via CDP or Accessibility APIs — both of which are real future
  options but bigger, riskier builds than "show a page safely." Amin can
  open a URL for her to look at; Amin does **not** read or act on page
  content yet — that's further, separate work with its own security
  tradeoffs (arbitrary JS execution, content extraction) worth its own
  design pass rather than folding into tonight's minimal version.

## Phase 3 design notes: a local Delta Brief, real Gmail/Calendar blocked

`src-tauri/src/brief.rs` generates a "what changed" summary from Amin's
own local data only: open task count, tasks created/completed in the
last 24h, how many follow-ups are currently due, and the 5 most recent
audit-log lines. 3 unit tests cover the counting logic and that audit
events actually show up.

This exists specifically because it needs nothing external — Gmail and
Calendar (the rest of Phase 3, and the "real" Morning Brief the brief
describes) are hard-blocked on Mona creating a Google Cloud OAuth client
and granting Amin access, which is account-setup work nobody else can do.
The "🎙️ Ask Amin to narrate this" button in the Delta Brief panel sends
the structured data as context to the existing Agent Core (Phase 1) and
lets it produce a natural-language summary — reusing what already exists
rather than building a separate narration path.

## Phase 4 design notes: a local Follow-up Engine, honestly scoped

`src-tauri/src/followups.rs` builds on the `follow_ups` table that's
existed since Phase 0's schema: create a follow-up tied to a task with a
due time, escalate it through the three stages the brief named
(friendly → firm → escalate_to_user, capping at the last one rather than
erroring or wrapping around), list what's currently due, resolve it once
handled. 6 unit tests cover creation against a missing task, a malformed
`due_at` (fails loudly rather than silently storing something `list_due`
could never compare correctly), the due-filtering logic, and the
escalation cap.

**Update, same night:** `src-tauri/src/notify.rs` wires escalation to a
real native OS notification (`tauri-plugin-notification`) — the one
delivery channel available without any external account. `create_follow_up`
fires one immediately if the due time has already passed (the ⏰ "remind
me now" demo path); `escalate_follow_up` fires one on every stage bump,
with a stage-appropriate title. `notify::send` never panics on failure —
a missing permission or an environment with no notification daemon (this
development sandbox has neither a working D-Bus session nor a real Mac)
degrades to "nothing shown," never a crash; this specific behavior is
unverified on a real Mac, same caveat as the voice pipeline. Email is
still genuinely absent — that needs Phase 3's Gmail connector, which
needs Mona's own Google OAuth credentials to exist at all. "Sent" in the
`follow_ups` table's status column means "Amin surfaced it" (in the UI
and, now, as a notification) — not "emailed."

## Tool use and the confirmation gate (2026-08-26 morning)

Mona's own words, verbatim, are the spec for this section: she's fine with
Amin reaching "كل الملفات" (all files) and "كل المتصفحات الآمنة" (all safe
browsers) — but "أي خطوة قبل ان ينفذها ينتظر مني اقوله كلمة... موافقة نفذ"
("any step, before [Amin] executes it, waits for me to say a word —
approval, execute"). She framed this around cybersecurity being her single
highest priority and wanting Amin more disciplined about it than a human
employee would be toward their employer. Until this pass, that instruction
wasn't actually enforced anywhere — every file/browser action so far was a
manual UI button *she* clicked, so "waits for her" was true by accident,
not by design. This pass makes it real for the one place it wasn't yet:
Amin's own agentic tool use through the Anthropic API.

**What changed, end to end:**

- **`src-tauri/src/agent.rs`** — reworked for real tool use. `ChatMessage`
  now carries a `serde_json::Value` (not a plain string), because an
  assistant turn that calls a tool, and the user turn that reports a
  tool's result back, both need the richer Anthropic content-block shape.
  `AnthropicResponse` gained `first_tool_use()`, `as_assistant_content()`,
  and `text()` so the caller can inspect a turn without re-deriving it,
  and `send_message` now takes the live tool registry and returns the raw
  response rather than pre-extracted text — command orchestration owns
  deciding what happens next, not the API wrapper.
- **`src-tauri/src/tools.rs`** (new) — the actual tool registry: JSON
  schemas Claude sees (`tool_definitions`), the risk tier that decides
  whether a call runs immediately or waits (`risk_for`), a human-readable
  Arabic description for the confirmation prompt (`describe`), and the
  dispatcher that calls into `tasks`/`files`/`browser`/`followups`
  (`execute`). Risk tiers are assigned explicitly here per tool, not
  inferred from `policy::classify`'s generic keyword match — a
  `write_workspace_file` tool call needs a considered, reviewed tier, not
  whatever a keyword happens to match. An unrecognized tool name defaults
  to `ConfirmHighRisk` rather than `Auto`.
- **`src-tauri/src/confirmation.rs`** (new) — `PendingConfirmation`, the
  Tauri-managed state holding at most one proposed `ConfirmHighRisk` tool
  call at a time (a new one overwrites whatever was pending, rather than
  queuing), and `interpret()`, a word-boundary matcher for Mona's reply
  (Arabic "موافقة"/"نفذ"/"تمام" etc. and English "yes"/"confirm"/"go
  ahead" for approval; "لا"/"إلغاء"/"no"/"cancel" for denial) that
  deliberately returns `Unclear` rather than guessing when her message is
  ambiguous or contains both signals — see its own unit tests for why
  substring matching alone isn't safe here (e.g. "لا" appearing inside an
  unrelated longer word).
- **`src-tauri/src/commands.rs`**'s `send_agent_message` — the
  orchestration. On every call: if a `ConfirmHighRisk` action is already
  pending, this message *is* Mona's answer to it (`resolve_pending_action`
  handles approve/deny/unclear and never starts a new turn until that's
  settled). Otherwise, Claude gets the real tool registry; an
  `Auto`/`TrustedDelegation` tool call executes immediately (audited
  either way) followed by one more call to Claude so it can narrate the
  outcome in plain language instead of Mona seeing raw tool JSON; a
  `ConfirmHighRisk` call is stored as pending and the reply asks her
  directly for "موافقة" / "نفذ" / "إلغاء" instead of running anything.
- **`src-tauri/src/audit.rs`** gained `Decision::Proposed` — the audit log
  now has a real entry for "Amin asked to do this and is waiting," not
  just for outcomes after the fact. Approving later logs `Executed` (or
  `Blocked` if the tool itself errors); denying logs `Declined`.
- **File-access scope, broadened and re-gated together.**
  `files.rs::workspace_root` now resolves Mona's home directory instead of
  `~/Documents/Amin` — a pragmatic reading of "all files," not the literal
  filesystem root, which would expose OS/system files no real task needs.
  Given that much broader surface, `tools::risk_for` makes **every** file
  tool `ConfirmHighRisk`, including `list_workspace_files` and
  `read_workspace_file` — not only writes and deletes. The reasoning: a
  file read is no longer confined to a small folder Mona put things in
  specifically for Amin; it can reach anything on her machine, and its
  content then leaves the machine entirely, inside a `tool_result` sent to
  a third-party API (Anthropic's). That is exactly the kind of "step" her
  instruction says must wait for her word, not just the destructive ones.
  Task and follow-up tools stay `Auto`/`TrustedDelegation` — they're local
  SQLite bookkeeping in Amin's own database; nothing leaves the machine
  and nothing outside that database is touched, so a lighter tier still
  fits the "loyal employee doesn't need permission for their own notes"
  framing.
- **Browser isolation — clarified, not changed.** Mona asked whether an
  isolated Amin-only browser window still lets Amin "do everything" (any
  site, any login) or whether isolation itself is a restriction. It isn't:
  `browser.rs`'s isolated `WebviewWindow` (its own `data_directory`, never
  Mona's real browser profile) has the full capability of a normal browser
  window — any URL, any site interaction a webview supports — the only
  difference is separate cookies/session storage from her personal
  browser, which stays completely untouched. `open_browser_url` was
  already `ConfirmHighRisk` before this pass and still is.

**Update, 2026-08-28: the browser tool can now actually act on a page, not
just show it.** Mona asked for real "read/click/fill" capability. Added
`read_page_content`, `click_page_element`, `fill_page_field` to
`browser.rs`, all `ConfirmHighRisk` like `open_browser_url`. Mechanism:
`WebviewWindow::eval_with_callback` injects JS into the same isolated
window, bridged to an `async` Rust fn via a `tokio::sync::oneshot`
channel (the callback is `Fn`, called once in practice but not `FnOnce`
by the API's own signature, so a `Mutex<Option<Sender>>` is what lets it
still consume the one-shot sender). `read_page_content` tags every
visible interactive element with a fresh sequential `data-amin-id` and
returns the page's URL/title/text plus that element list; `click`/`fill`
address an element purely by that integer, never a caller-supplied CSS
selector — the only thing ever spliced into the follow-up script is an
`id: u32` and, for fills, a value passed through `serde_json::to_string`
for safe escaping. That splice design is what keeps this injection-safe
even though the input ultimately traces back to Claude's own tool call.

Second-order fix this required: `tools::execute` becomes `async` (it
`.await`s the browser calls), which meant it could no longer hold an
already-locked `&Connection` for its whole body — a `std::sync::
MutexGuard` isn't `Send`, so one alive across an `.await` (either inside
`execute` or in a caller holding it across `execute(...).await`) fails to
compile under Tauri's async command runtime. Fixed by having `execute`
take `&Db` and lock fresh inside each synchronous arm instead — verified
this actually works (not just assumed) by reading rustc's real behavior
via `cargo check` rather than reasoning it through in the abstract: a
`MutexGuard` created and dropped entirely within one non-awaiting match
arm never gets captured by the generator state of a sibling arm that does
await, so `Send`-ness holds. Both `commands.rs` call sites now lock only
around the (unavoidably synchronous) `audit::record` call, after the
`execute(...).await` has already returned.

**Still not testable here:** `read_page_content`/`click_page_element`/
`fill_page_field` need a real `WebviewWindow` actually rendering a page —
same category as voice/mic, this sandbox cannot open one. `browser.rs`'s
existing unit tests (URL validation) still pass, and the whole crate
still compiles and passes `cargo test` with these changes in, but the
actual DOM-tagging/click/fill JS has only been read carefully, never run
against a live page. That's a real gap Mona should exercise once this
build is in her hands, the same way she found the hands-free and
pronunciation bugs by actually using the app.

**Update, same day: batching, for real speed on multi-file tasks.** Mona's
first real task was going to be "organize my whole computer, and I want
it fast" — under the one-approval-per-file-tool-call model that already
existed, a real reorganization would have meant approving every single
move/delete separately, which is the opposite of fast. Added
`move_workspace_file`, `create_workspace_folder`, and
`batch_file_operations` to `files.rs`/`tools.rs` (all `ConfirmHighRisk`,
same as every other file tool — speed comes from batching the
*confirmation*, not from skipping it). `batch_file_operations` takes a
list of move/delete/create_folder/write operations, describes the whole
plan as one multi-line confirmation (`describe_batch` — Mona reviews and
approves it once), then runs each operation in order, continuing past a
failure rather than aborting the batch, and reports exactly which ones
succeeded and which didn't — never a blanket "تم" over a partial result.
Also gave `list_workspace_files` an optional `path` (browse any
subfolder, not just the home root) and `recursive` (up to 3 levels deep,
capped at 500 entries with a `truncated` flag) so surveying a messy
folder before planning a batch takes one confirmed call instead of one
per subfolder. `WorkspaceEntry` changed from a bare `name` to a
home-relative `path` so a nested entry from a recursive listing can be
passed straight back into any file tool without reconstructing it —
updated the one other caller (`commands::list_workspace_files`, the
Notes panel's direct path) and the frontend's `WorkspaceEntry` type to
match, and added `white-space: pre-line` to the approval card's
description so a multi-line batch plan actually renders as one line per
operation instead of collapsing into a single run-on line — a real
finding, not a hypothetical one, once we noticed `describe_batch`'s
output was multi-line but the CSS wasn't accounting for it. Same
filesystem-safety caveat as the rest of `files.rs`: could not add tests
that actually run `mv`/`create_dir`/`list` through a mock `AppHandle`,
since `home_dir()` there resolves to this container's *real* `$HOME`, not
a sandboxed path — consistent with why the existing tests only exercise
the pure `resolve_within_workspace` logic, not the `app`-dependent
wrappers. Added tests for `describe_batch` itself (pure string
formatting, no filesystem) instead.

**What's still unverified:** everything here is covered by unit tests (49
passing, including `tools.rs`'s dispatcher exercised end to end with a
mocked Tauri `AppHandle`) and a clean `cargo check`/`clippy`/`tsc`/`vite
build`, but the actual back-and-forth — Claude proposing a real tool call,
Mona typing "موافقة" back, Amin executing and narrating — has not been
run against the live Anthropic API or on a real Mac. That's the natural
next thing to try together once she's at the keyboard.

**Update, 2026-08-28: two real hands-free trust problems, found by Mona
actually using it, not by review.** She left hands-free on and moved to
unrelated, confidential work (drafting official government correspondence
in Chrome); noticing the mic had stayed hot the whole time via macOS's own
privacy indicator was, in her words, a serious problem — a live
microphone she'd forgotten about while handling ministry work is a real
trust failure, not a cosmetic one. Investigating turned up two separate
issues, not one:

1. **No inactivity timeout.** Hands-free, once on, listened forever with
   no automatic stop. Fixed in `AminVoice.swift`: `HandsFreeListener`
   tracks `passiveModeStartedAt`, reset on every real wake-phrase
   engagement (`openActiveSession`), and after 15 minutes of continuous
   passive listening with no engagement, `armPassive` emits a new kind 8
   instead of re-arming. It deliberately does *not* call its own `stop()`
   at that point — tearing down `audioEngine`/`removeTap` from inside its
   own recognition callback risked a double-stop if the normal stop path
   then ran anyway against stale state. Instead, kind 8 → Rust's
   `voice::on_voice_event` → `voice://hands-free-timeout` → a new frontend
   listener that calls `setHandsFreeMode(false)`, the same
   `set_hands_free_mode` Tauri command a manual toggle-off already uses —
   one real, already-correct teardown path, not two.
2. **The toggle could lie after a restart.** `set_hands_free_mode`
   persisted its on/off state to the settings table, and
   `get_hands_free_settings` read it back into the frontend's initial
   `handsFreeEnabled` — but `lib.rs`'s `setup` never called
   `voice::start_hands_free` on launch. A previous session's "on" would
   make a fresh launch's UI claim hands-free was running when the native
   listener had never actually restarted. The fix is not "make it actually
   auto-resume the microphone on launch" — silently starting a live mic
   without an explicit action that session is exactly the kind of thing
   Mona was just alarmed by — so `get_hands_free_settings` now always
   reports `enabled: false`, and `set_hands_free_mode` no longer persists
   the flag at all (dead once nothing reads it back). Hands-free is now a
   per-session choice, full stop: she turns it on when she wants it, every
   time, and it never resumes on its own.

Not verified on a real Mac yet (the Swift side can't be compiled in this
sandbox at all — no `swiftc`, same limitation as every other AminVoice.swift
change) — Mona should confirm the 15-minute timeout actually fires and
stops the mic, and that a relaunch genuinely shows hands-free off with no
stale "on" state anywhere.

## Real-time barge-in: interrupting Amin mid-reply (2026-08-28)

Mona asked for real-time voice conversation — specifically, being able to
talk over Amin while it's replying during a hands-free session, the way a
human interruption works, instead of having to wait it out or hit a
"stop speaking" button. This replaces the old blanket mute
(`amin_voice_set_hands_free_muted`) with something that can tell "Amin
hearing its own voice" apart from "Mona actually talking over it."

**Originally two layers; now one, after a real regression found in the
field (2026-08-28):**

1. ~~**Acoustic echo cancellation.**~~ `HandsFreeListener.openTap` called
   `AVAudioInputNode.setVoiceProcessingEnabled(true)` before installing the
   mic tap — the same VoIP-grade echo cancellation telephony apps use.
   **Reverted**: Mona reported having to speak unusually loudly for
   hands-free to hear her at all ("لازم اصرخ لحد ما يرد عليا") after this
   shipped. The real problem: this turned on Apple's whole VoIP-style
   pipeline — automatic gain control and noise suppression tuned for
   close-talking phone-call audio, not just echo cancellation — for the
   *entire* hands-free session, not only the brief windows Amin's own
   voice was actually playing back. Degraded recognition at normal
   conversational volume the other 99% of the time was too high a cost for
   a barge-in feature she hadn't even confirmed working yet. Removed;
   barge-in now relies solely on layer 2 below.
2. **Text comparison.** `voice.rs`'s `set_hands_free_speaking` now carries
   the actual sentence Amin is about to say (previously just a mute
   flag), threaded through from `commands::speak_text` (ElevenLabs) and
   `AminVoice.swift`'s own `SpeechDelegate.didStart` (on-device, via kind
   3's text — previously always null). `HandsFreeListener.isLikelySelfEcho`
   compares each recognized utterance against that sentence — exact
   containment or >60% word overlap — and discards it as an echo exactly
   like the old mute did. A clearly different utterance is a real
   barge-in: it emits a new kind (9) instead, and — this is the part that
   makes it feel instant rather than laggy — `voice.rs`'s `on_voice_event`
   calls `stop_speaking()` synchronously, *before* even emitting the
   frontend event, so playback stops in the same call stack as the
   recognition callback that detected the interruption. The frontend's
   `voice://hands-free-barge-in` listener then treats the heard text as
   her next command, identically to a normal final while a session is
   open; `voice://speaking-finished` fires on its own as a side effect of
   the forced stop (`AVSpeechSynthesizerDelegate.didCancel` / the killed
   `afplay` process both still reach it), so aminState resets without any
   extra plumbing.

Deliberately biased toward "assume it's an echo" on a tie — a missed
barge-in just means Mona repeats herself; a false one makes Amin cut
itself off having heard nothing, a stranger failure to explain.

**What's real and what isn't yet:** the design compiles/tests clean, but
this is audio hardware behavior — exact word-overlap threshold, whether
9's synchronous cross-language call chain behaves the way traced through
on paper — that literally cannot be verified without a real Mac, a real
microphone, and real speakers. This sandbox has none of the three, which
is exactly how layer 1 shipped with a real, user-facing regression
undetected: it compiled and tested clean too. Mona needs to actually try
interrupting Amin mid-sentence during a hands-free conversation and
report what happens — including "it falsely thinks I'm interrupting when
I'm not" and "it doesn't notice when I do," both real possible outcomes —
*and*, separately, whether normal hands-free listening (not interrupting,
just being heard at a normal conversational volume) still feels as
sensitive as it did before this feature existed at all.

## Visual modes: 3D avatar and Portrait, one Amin Core (2026-08-28)

Mona asked for two ways to *see* Amin — a real 3D facial-rig avatar and her
originally-approved reference artwork animated as a talking portrait —
switchable at any time without starting a new conversation, resetting
memory, or changing voice. The architecture keeps this literal: nothing
about the conversation, agent, tools, memory, or voice pipeline knows or
cares which renderer is on screen.

```
Amin Core (App.tsx state: agentLog, voice events, tasks, settings — unchanged)
      │
      └── AminPresence (src/components/presence/AminPresence.tsx)
              │
              ├── visualMode === "3d"       → ThreeDAvatar.tsx
              └── visualMode === "portrait" → the original <img> (unchanged)
```

`visualMode` (`src/lib/visual/visualMode.ts`) is a single `"3d" | "portrait"`
value read from/written to `localStorage` (key `amin.visualMode`) — a
display preference with no security or audit relevance, unlike every other
durable setting in this app (hands-free, API keys) which goes through a
Rust-side command specifically *because* those carry a trust decision.
Persists across restarts the same way those do (WKWebView keeps
localStorage in the app's own data directory); defaults to `"portrait"` on
first launch. A small toggle (top-left, `.visual-mode-toggle` in
`App.css`) is the only new UI — deliberately not a settings panel or a new
screen. Switching modes only ever swaps which component renders inside
`AminPresence`'s existing slot; `agentLog`, `aminState`, and every voice
event listener live in `App.tsx` above that slot and are never touched.

### 3D Mode: the real facial-rig GLB, not a placeholder

`public/models/amin_facial_rig.glb` is the exact file produced and
validated in this session's facial-rig work (see the git history for
`scripts/facial-rig/`) — 51/52 ARKit blendshapes, 15/15 Oculus visemes,
`tongueOut` genuinely absent (no tongue geometry in the source FBX; see the
facial-rig section this repo already carries for that decision). Amin
himself did the FBX→GLB conversion and viseme renaming with a real,
headless Blender install, not by asking Mona to do it — this is the same
file, unedited, reused as the app's actual 3D asset.

`src/components/presence/ThreeDAvatar.tsx` renders it with plain `three.js`
(GLTFLoader), not React Three Fiber — this app has no other WebGL usage,
so an imperative scene (mounted once in a `useEffect`, disposed on
unmount) gives direct, precise control over specific bones and morph
target names without an extra abstraction layer. Every motion is driven by
a real, disclosed signal:

- **Blink** — a randomized 2.5-6s timer, the standard idle-blink technique;
  not tied to state or audio.
- **Eye saccades** — small randomized gaze targets applied to the rig's
  real `LeftEye`/`RightEye` bones (this is a full Mixamo-style skeleton
  bundled with the face mesh, not a face-only asset).
- **Head sway** — a low-amplitude sine composite on the real `Head` bone,
  capped under ~2° so it reads as breathing, not nodding.
- **Mouth while speaking** — driven by real, decoded audio loudness (see
  "Real-time mouth movement" below), blended across a couple of open-mouth
  visemes for shape variety. This is honestly **amplitude-reactive mouth
  movement, not phoneme-accurate lip sync** — there is no real-time
  phoneme alignment anywhere in this pipeline, and none was added. Said
  plainly rather than oversold.

**A real bug found and fixed by actually testing this in a browser, not
assumed working from the code:** forcing `eyeBlinkLeft`/`eyeBlinkRight` to
their maximum value produced *no visible change at all* — eyes stayed
open. Before assuming a rendering bug, the raw glTF morph-target accessor
was inspected directly (Python, reading the binary buffer): the blend
shape carries real, correctly-sized vertex displacement (up to ~1cm across
525 vertices — anatomically plausible for a human-scale head), and
three.js's GLTFLoader parses it correctly (confirmed via
`geometry.morphAttributes`, not assumed). A control test —
forcing `jawOpen` instead — rendered perfectly, opening the mouth
correctly (and incidentally revealing a mouth-interior/gum mesh that looks
tongue-like at a glance; still not a riggable tongue with its own
blendshape, so the `tongueOut` gap stands). That isolated the bug to the
eye region specifically: the static, non-morphing cornea (eyeball) sphere
sits in front of the eyelid's closed position, so a real, correctly-applied
blink is fully occluded by the eyeball Meshy's export never made shrink or
retreat. `ThreeDAvatar.tsx` now hides `AvatarLeftCornea`/`AvatarRightCornea`
whenever `blink.value > 0.6`, exactly when the eyelid should be covering
them anyway — verified by polling the real animation loop's own morph
value until it caught an actual blink mid-cycle and screenshotting that
exact frame, not just eyeballing a few random frames.

**Real-time mouth movement**: `src-tauri/src/audio_level.rs` decodes the
same MP3 bytes ElevenLabs returns (via `symphonia`, pure Rust — no system
codec to bundle) into PCM, computes short-window (40ms) RMS loudness
normalized to 0.0-1.0, and `commands::speak_text` spawns a background
thread emitting `voice://audio-level` at that cadence alongside the
existing `afplay` playback thread — cloned from the exact same audio
bytes, not a separate/approximate source. `stop_speaking` cancels it via
the same `Arc<AtomicBool>` pattern already used for the afplay PID, so a
manual stop or a real barge-in doesn't leave the mouth animating after
Amin's voice has actually gone silent. The frontend reads this through
`src/lib/visual/audioLevelBus.ts` — a plain module-level pub/sub, not React
state, since it updates ~25 times/second while Amin talks and only
`ThreeDAvatar`'s own `requestAnimationFrame` loop needs to read it, once
per frame; running that through `useState` would re-render the whole
component tree at that rate for a value nothing else needs.

**Only wired up for the ElevenLabs voice.** The on-device
`AVSpeechSynthesizer` path (see "Voice pipeline" above) never hands Rust
any audio bytes at all — there is nothing to decode. On that path the 3D
avatar's mouth simply stays at rest during speech, which is more honest
than inventing motion with no real signal behind it.

**Fallback**: if the GLB fails to load or WebGL isn't available,
`ThreeDAvatar` calls `onFailure`, and `AminPresence`/`App.tsx` switch back
to Portrait mode automatically with a banner explaining why — Amin Core
(voice, conversation) is entirely unaffected either way, matching Mona's
explicit requirement that a renderer failure never stops Amin from talking.

**What's verified vs. what still needs Mona's Mac**: the GLB loading,
camera framing, blink (including the cornea fix), eye saccades, head sway,
mode toggle, and localStorage persistence-across-reload were all tested in
a real headless Chromium browser this session (`npm run dev` + Playwright,
screenshots taken and inspected, not assumed) — see the session's own
screenshots for the exact evidence. What's *not* verifiable here: real GPU
performance on her actual Mac, and whether the amplitude-driven mouth looks
natural against her real ElevenLabs voice output, since this sandbox has
no audio device and Tauri's Rust backend doesn't run here at all — `speak_text`
and the whole `voice://audio-level` pipeline were verified by `cargo test`
(the RMS math itself, against synthetic sine waves) and by reading the
integration code, not by hearing it.

### Portrait Mode: the honest current state

Portrait Mode's earlier artwork (a side-profile, translucent wireframe
illustration with no clearly rendered mouth or eyelid) was never
animatable without inventing geometry that wasn't there — see git history
for that original finding. **2026-08-28: Mona replaced it** with a real
photorealistic frontal portrait (her own reference photo, composited by
Amin with the brand's gold/blue AI-circuit motif via `flux-2-pro`, then
the "أمين / Amin" text lockup added) — now `src/assets/amin-identity.jpg`.
This genuinely unblocks dynamic animation: a frontal face with a visible
mouth and eyes actually exists to drive now.

**Two real, tested attempts at building this without a paid provider, and
why both are ruled out with evidence rather than assumption:**

1. **Per-reply rendered video** (`bytedance-omnihuman-v1.5` — animates a
   still photo + an audio clip into a lip-synced talking-head clip,
   available through this session's own creative-tools connector, no new
   account needed). Tested for real with the actual portrait image and a
   short real Arabic ElevenLabs-style line: **`estimate_only` returned
   ~260 seconds of render time and ~$0.86 per generation.** For a live
   conversation where Amin might speak dozens of times a day, that's both
   a multi-minute wait after every single reply and a recurring cost that
   scales with usage — not what "متزامن مع صوت أمين الحقيقي" (synced to
   Amin's real voice, live) means. Ruled out for the live-conversation
   path on measured numbers, not guessed ones.
2. **A free, local "blink" via a second generated frame** — asked
   `gemini-3-pro-image` to edit the official portrait into an otherwise
   identical frame with the eyes closed, meaning to crossfade the two
   client-side (no API, no per-reply cost). The eyes-closed result looked
   right on its own, but overlaying it against the original exposed a
   real problem: **the two independently generated images don't share
   pixel-exact framing** (the edit model changed canvas size and shifted
   the face's scale/position slightly), so a naive crossfade would visibly
   jump on every blink instead of reading as one. Not shipped — a
   convincing version of this would need real face-landmark alignment
   between the two frames (buildable, but a separate piece of engineering,
   not attempted yet since Simli's own live model produces blinking
   natively as part of one coherent pipeline instead).

**Conclusion, unchanged from the provider research below:** real lip-sync
+ blink + eye movement + head movement + micro-expressions, all tied to
live conversation with no per-reply wait, needs a real-time streaming
avatar provider. Simli remains the recommendation.

**Provider research** (bring-your-own-audio real-time avatar APIs, so
Portrait Mode keeps the same shared ElevenLabs voice rather than a
provider's own TTS):

| Provider | Fit | Notes |
|---|---|---|
| **Simli** (recommended) | Best | Purpose-built for external audio → single-photo lip-sync; PCM16 over WebSocket in, WebRTC video out. ~$0.05/min pay-as-you-go, $10 free credit. Cheapest, simplest integration found. |
| **D-ID** (fallback) | Good, unverified detail | Mature single-photo workflow, documented Arabic support, but its audio-driven Streams API is labeled "legacy" in D-ID's own docs and pricing sources conflict — needs a real quote and a POC before committing. |
| HeyGen LiveAvatar | Not recommended yet | Two unconfirmed gaps sit exactly on the hard requirements: whether raw external audio (vs. HeyGen's own TTS) is accepted, and whether a custom single photo works in the *real-time* pipeline at all. |

None of this can be integrated without Mona creating an account and
providing an API key/payment — not something Amin can do on her behalf.
The exact steps: sign up at simli.com, create a custom avatar from
`src/assets/amin-identity.jpg` (already the right, frontal reference),
generate an API key from the dashboard, and hand it over — at that point
the real-time WebRTC pipeline (fed the same PCM audio `audio_level.rs`
already decodes for the 3D avatar, so it's still ONE Amin voice) gets
wired up the same way 3D Mode already is: swappable, same Amin Core.

**Today, until a provider + credential is chosen**: Portrait Mode shows
the new official photo, static — no blink, no lip movement yet. This is
disclosed as incomplete, not presented as done.

## 3D Mode's camera cropped the head and shoulders on the real Mac
## (2026-08-28)

A second real bug from the same round of Mac screenshots (Mona: **"و راسه
و اكتافه مقصوصه ليه كده"** — why is his head and shoulders cropped like
that): `ThreeDAvatar.tsx`'s camera framing, `camera.position.set(...,
headWorldPos.z + 0.62)` with a 24° vertical FOV, put only about 0.26m of
vertical extent in frame — less than the height of a head alone. The
comment above it said "frame a bust shot (head + shoulders)"; the actual
numbers didn't deliver that, and nothing in this sandbox's earlier testing
(all done via forced state/blink checks, never a plain default-pose
screenshot) had looked at the overall framing to catch it.

**Reproduced and fixed the same way as the CSP bug** — against the real
model, not guessed: read the rig's actual world coordinates (`Head` bone
at y≈1.524, model bounding box top at y≈1.698 — so barely 0.17m of hair
sits above the head joint) via a temporary debug log, then used Playwright
screenshots of the real `amin_facial_rig.glb` at increasing camera
distances until the framing actually matched "head and shoulders" — full
hair with a little headroom, collar/upper-shoulders visible, cropped
*before* the rig's arms-out rest pose comes into frame (this rig has no
idle-standing animation, so anything wider than a tight bust crop shows it
standing arms-out like a scarecrow — cropping tight enough for a proper
bust shot incidentally hides that separate, undisclosed-until-now gap
too). Landed on distance 0.85 at the same 24° FOV. Verified across three
window aspect ratios (wide/square/narrow) since the container's real size
on Mona's Mac wasn't known — vertical framing is FOV-driven and aspect-
independent, confirmed by the screenshots rather than assumed.

## 3D Mode shipped textureless on the real Mac — a CSP bug this sandbox
## could never have caught (2026-08-28)

Mona's first real test of 3D Mode on her actual Mac showed a ghost-white,
faceless bust — correct geometry and morph targets, but every skin/eye/
teeth color and normal map missing. It looked broken on sight ("ايه
القرف ده"). Every render of this same model in this session up to that
point (many screenshots, the blink-fix verification, all of it) looked
correct — because every one of them ran against `npm run dev`'s plain
Vite server, which enforces **no CSP at all**. Tauri only injects the
real CSP (`tauri.conf.json`'s `app.security.csp`) into the actual
packaged app. That gap meant nothing in this session's testing could
ever have caught a CSP-only bug before she did — worth stating plainly,
not glossed over, since it's the reason this shipped once already.

**Root cause, confirmed rather than guessed:** `amin_facial_rig.glb`
stores all 10 of its textures embedded (`bufferView`, not an external
`uri`) — Meshy's normal export choice for a single self-contained file.
To decode an embedded image, three.js's `GLTFLoader` always wraps it in
a `Blob` and calls `URL.createObjectURL()`, then loads that `blob:` URL
through `ImageBitmapLoader`, whose own source (`three/src/loaders/
ImageBitmapLoader.js`) fetches it with plain `fetch()` — governed by
CSP's `connect-src`, not `img-src`. The CSP added for 3D Mode never
included `blob:` in `connect-src` (or `img-src`, for the older
non-bitmap fallback three.js also has). Every texture request was
silently blocked; the geometry itself doesn't go through this path
(it's a single upfront GLB fetch off `'self'`), so the mesh rendered
fine while every material stayed at its default (white, untextured).

**Reproduced and fixed, not just reasoned about:** built the real
production bundle (`npm run build`, the same output Tauri packages —
not the dev server), served it statically, and injected the exact old
CSP string as a `<meta>` tag via Playwright's request interception. The
console showed ten explicit `Refused to connect to 'blob:...' ...
connect-src` violations — one per texture — and the screenshot matched
Mona's photo exactly: pale, faceless, eyelashes-only (the one material
using a flat `baseColorFactor` instead of a texture, so unaffected).
Adding `blob:` to both `connect-src` and `img-src` and re-running the
identical test: zero CSP violations, full color/normal textures
rendering correctly. Both screenshots exist as direct evidence, not an
assumption that the fix works.

## Simli integration: Portrait Mode's real lip-sync engine (2026-08-28)

Mona's condition before spending anything: prove the integration itself
works on Simli's free plan (a shared preset face) before she upgrades to
build a custom Amin face. This section documents what was actually built
and — since a real API key was deliberately never shared into this
session (Mona's own explicit requirement, see below) — exactly which
parts that let verify for real versus which parts still need her to run
it once.

### Where the key lives

Same pattern as the Anthropic and ElevenLabs keys (see `has_api_key`'s
doc comment in commands.rs): a local settings-table entry, entered once
in Amin's own Settings panel, never typed into a chat message, never
committed to Git or baked into the binary. Not the OS Keychain — that
was tried for the Anthropic key first and failed reproducibly on a real
Mac (see secrets.rs), so Mona already made this trade-off knowingly; a
third secret doesn't reopen that decision. `SIMLI_KEY_NAME =
"simli_api_key"`, `SIMLI_FACE_ID_KEY = "simli_face_id"` in commands.rs;
`has_simli_key`/`save_simli_key`/`clear_simli_key`/`get_simli_face_id`/
`save_simli_face_id` Tauri commands; a Settings field identical in shape
to the ElevenLabs one. Leaving the face ID blank uses
`SIMLI_DEFAULT_PRESET_FACE_ID` — Simli's own published free "Doctor"
preset face — exactly as instructed: prove it works before paying for a
custom one.

### Why the WebRTC half has to live in the webview

This is the one deliberate exception to "the frontend never reaches the
network directly" (see Cargo.toml's reqwest comment) — `connect-src` in
tauri.conf.json now also allows `https://api.simli.ai` and
`wss://api.simli.ai`, nothing else. The reason: Simli's real protocol
(confirmed against their actual OpenAPI spec and docs, not a summary)
carries both the WebRTC signaling *and* the raw outgoing audio frames
over the *same* WebSocket — `wss://api.simli.ai/compose/webrtc/peer_to_peer?session_token=...`.
WebRTC (`RTCPeerConnection`) is a browser API with no equivalent in this
Rust codebase, and since signaling and audio share one socket, there's
no way to keep audio-sending in Rust while only handing the frontend a
finished video element. So the whole client — opening the socket,
creating the offer, sending the SDP, receiving the answer, and streaming
audio bytes once connected — runs in `src/lib/simli/simliClient.ts`,
written directly against Simli's documented plain-WebRTC protocol rather
than their `simli-client` npm package (whose docs only show the
LiveKit-backed path, with no version pinned). Rust's `simli.rs` does
exactly one thing: exchange the long-lived API key for a short-lived
session token via `POST https://api.simli.ai/compose/token` — that
token, not the key, is what reaches the webview.

### A real bug, caught by testing against Simli's live server with an
### invalid key — not by trusting their docs

Simli's own published error example is `{"session_token":"FAIL
TOKEN","detail":"..."}`. Checking that shape looked sufficient — until
an actual `curl` against `api.simli.ai` with a deliberately wrong key
(not a real credential) came back **HTTP 401** with a *different*,
non-empty, real-looking `session_token` string plus
`detail:"INVALID_API_KEY"`. A body-only check would have silently
treated this exact failure as success. Fixed by checking
`response.status()` first, matching how `elevenlabs::synthesize`
already does it — and kept as `simli::tests::a_real_invalid_key_is_rejected_by_the_live_endpoint`,
an `#[ignore]`d test (network-dependent, and this repo's CI never runs
`cargo test` at all — see build-macos.yml) that reproduces the exact
call for anyone re-verifying this later.

### One shared voice, one audio source — not two playing at once

`elevenlabs::synthesize_pcm16` asks ElevenLabs for `output_format=pcm_16000`
(confirmed against ElevenLabs' own API docs) — raw 16-bit/16kHz/mono PCM,
the exact format Simli's docs specify, with no manual resampling step to
get wrong. `commands::synthesize_pcm_for_simli` uses the *same* stored
ElevenLabs key, voice ID, and emotion-tagging as `speak_text` — one Amin
voice, a different byte shape for a different transport, never a
different voice. When Portrait Mode's Simli session is connected,
`App.tsx`'s `speak()` sends audio there instead of through local
`afplay`, because Simli's WebRTC connection returns audio bundled with
its lip-synced video (recvonly audio+video transceivers) — playing local
`afplay` audio *at the same time* would mean Mona hears Amin's voice
twice, once instantly and once delayed by the network round-trip. Any
Simli failure (not configured, session dead, network error) falls back
to the exact same local `speakText` path 3D Mode always uses, with one
disclosed banner — a visual feature failing must never cost Mona the
ability to hear Amin at all.

### Session continuity across mode switches

`src/lib/simli/simliSession.ts` is a module-level singleton, not tied to
any component's mount lifecycle — switching `visualMode` between "3d"
and "portrait" never tears down or reconnects it. It also never connects
proactively: `PortraitAvatar.tsx` registers its `<video>` element on
mount but does **not** call `ensureConnected()` there, specifically
because that component also renders during the splash screen and every
time Portrait Mode is shown — connecting on every one of those would
burn through Simli's free-tier minutes before Amin ever says a word.
Connection happens lazily, inside `speakViaSimli`, the first time there
is something real to say.

### What's verified for real, and what still needs Mona's one test

**Verified against Simli's real, live server in this session** (not
assumed from docs): the exact REST request this code sends is accepted
by their endpoint, and the exact failure mode for a bad key is now
handled correctly (see the bug above) — this is a genuine, if narrow,
live integration test.

**Not verifiable without Mona's own key**, and not faked as passing: the
actual SDP offer/answer exchange, whether Simli's answer is accepted and
a real video track arrives, whether the free preset face's lip-sync
looks natural, and real end-to-end latency. Every line of
`simliClient.ts` was written directly against Simli's own documented
message shapes, but a *live* WebRTC handshake needs a real session token
this session was deliberately never given — Mona's own explicit
requirement, honored rather than worked around. The one, single test
this needs from her: paste a free Simli API key into Settings, open
Portrait Mode, and let Amin say one thing.

## Voice-biometric speaker verification: closing the "anyone who knows the
## wake phrase" gap (2026-08-28)

The Phase 1 design notes above ("Speaker voice recognition") called for
this from the start; it's built now, in a narrower first form than that
note originally sketched — see "What this is not yet" below for the gap.
Mona's own words for why it mattered: **"بصمة الصوت... اريده يتعرف ع صوتي
مش اجلس اصرخ لحد ما يرد عليا"** (the voice fingerprint — I want it to
recognize MY voice, not sit and scream until it responds). Hands-free
mode's wake phrase was, and without this still would be, a shared secret:
anyone in earshot who knows it can open a command session (see
AminVoice.swift's HANDS-FREE MODE note and docs/SECURITY.md). This adds the
actual identity check underneath it.

**How it works.** `scripts/voiceprint/convert_ecapa_to_coreml.py` converts
speechbrain's pretrained `speechbrain/spkrec-ecapa-voxceleb` (ECAPA-TDNN, a
standard open speaker-embedding model — not something trained on Mona's
voice, since no real Mona audio exists in this sandbox to train on) into a
CoreML `.mlpackage`, bundled as a Tauri resource. `VoicePrint.swift` (new,
compiled into the same dylib as `AminVoice.swift` — see
macos/transcriber/README.md) loads it and turns 3 seconds of 16kHz mono
audio into a 192-dim embedding. Mona enrolls once from Settings (~4 seconds
of speech, `SpeakerEnrollmentRecorder`), storing that embedding locally at
`~/Library/Application Support/Amin/voiceprint.json` — never synced,
never sent anywhere. From then on, `HandsFreeListener` keeps a rolling
3-second buffer of raw mic audio (`RollingPCMBuffer`, fed alongside the
existing speech-recognition tap) and, the instant the wake phrase is heard,
computes a fresh embedding from that buffer and compares it by cosine
similarity to the enrolled one before opening a session — a mismatch is
treated exactly like not having heard the wake phrase at all (a new kind-10
event, `voice://hands-free-voice-rejected`).

**A real bug found and fixed during conversion, not just "it worked."**
Tracing speechbrain's own `mean_var_norm`/`length_to_mask` code for CoreML
export failed outright (`TypeError: only 0-dimensional arrays can be
converted to Python scalars`) — its length-masking logic calls `len()`/
`torch.as_tensor()` on tensors in ways coremltools' PyTorch frontend can't
lower to a static graph. Two independent fixes were needed: inlining
`mean_var_norm`'s actual math (a per-utterance mean subtraction, confirmed
`std_norm=False` in the pretrained hyperparameters) instead of calling the
module, and monkeypatching `length_to_mask` everywhere speechbrain's
ECAPA-TDNN blocks import it, to use `.shape[0]`/`.to()` instead of the
untraceable calls — numerically identical for tensor inputs, which is all
any call site here ever passes. Both fixes were checked against
speechbrain's own reference output for the same input before trusting
them (max abs diff **0.0**, not "close enough") — see the script's inline
assertion, which refuses to save a model if that check fails.

**What's verified vs. not — same standard as every other feature in this
document.** Verified, in the Python sandbox that did the conversion: the
model's own numbers are exactly right. NOT verified — no macOS, no Xcode,
no microphone here, the same limitation disclosed for every other
Swift/AVFoundation feature in this codebase: that this Swift code actually
loads and runs the bundled `.mlpackage` on a real Mac, that real microphone
audio through `AVAudioConverter` produces embeddings resembling what the
conversion script exercised with synthetic input, and — the number nobody
can set from a sandbox — whether `VoicePrintEngine.matchThreshold` (`0.45`,
a literature-derived placeholder, not a measured one) actually separates
Mona's voice from someone else's on her real hardware. **The first real
test needed from her:** enroll once in Settings, then try the wake phrase
herself (should open) and, ideally, have someone else try it (should stay
silent) — report exactly what happened so the threshold can be tuned from
real data instead of a guess.

**Fails open, deliberately.** If nothing is enrolled yet, or the model
fails to load for any reason, `VoicePrintEngine.verify` returns `true` —
hands-free behaves exactly like it did before this feature existed (opens
on any wake phrase). A voice-security feature that can lock Mona out of
her own app on a model-loading hiccup would be a worse failure than the
gap it closes.

**What this is not yet**, so the Phase 1 design note above isn't
misread as fully delivered: this still requires the wake phrase — it
gates who can use it, it doesn't replace it with ambient, wake-word-free
presence detection ("pick her voice out of a room with other people
talking"). That's the harder, not-yet-built version of the original
design note; this is the narrower, shippable-now step that closes the
actual complaint (a stranger who knows the phrase can't open a session).
Also single-sample enrollment (one ~4-second recording, not several
averaged utterances) — a reasonable v1 given there's no way to iterate on
recording quality without a real Mac in the loop, but a likely first
improvement once real accuracy data comes back.

### A real regression this shipped: the whole voice engine stopped loading (2026-08-28)

The 0.2.18 release Mona updated to reported, on every voice feature at
once: **"couldn't load the voice engine: dlopen(...libaminvoice.dylib):
tried: ... (no such file) ..."** — the dylib itself missing from the
installed `.app`, not a code bug inside it.

**First theory, WRONG — corrected here rather than left standing.** The
first fix attempt (this section, as originally written) blamed the new
`bundle.resources` entry for the ECAPA-TDNN model, referenced via a path
escaping `src-tauri` (`"../macos/transcriber/Resources/ECAPA_TDNN.mlpackage"`),
reasoning that Tauri's "original directory structure preserved" behavior
for array-format resources was ambiguous for a path starting with `../`.
That fix (moving the model into `src-tauri` at CI time — kept below, since
it's still a real improvement) shipped as `0.2.19` and was verified by
actually downloading and extracting the published `.app.tar.gz`: both
`libaminvoice.dylib` and `ECAPA_TDNN.mlpackage` were now genuinely present
at the right paths inside `Contents/Resources/` — the resource-bundling
theory's fix *worked*, on its own terms. **The dylib was still 0 bytes.**

**The real cause, found in the actual CI build log, not guessed a second
time**: `AudioResampler` (`VoicePrint.swift`) was declared `private`.
Swift's `private` is file-scoped, not module-scoped — it doesn't mean "not
public API," it means "only this file can see it," even when another file
compiles into the very same module (as `AminVoice.swift` and
`VoicePrint.swift` do here — see macos/transcriber/README.md). Every
build since `VoicePrint.swift` was added (`0.2.17`, `0.2.18`, `0.2.19`)
had genuinely failed to compile with `error: 'AudioResampler' is
inaccessible due to 'private' protection level` at both of
`HandsFreeListener`'s call sites — and because `build-macos.yml`'s "Build
the voice engine" step's own graceful-degradation design (an `if/else`
shell construct that always exits 0, printing a `::warning::` and
bundling an empty placeholder dylib rather than failing the whole
release) never fails the CI *job* itself, this was invisible in every
green checkmark this whole time. Nothing in this pipeline — not the build
step's own conclusion, not the later "resource path exists" checks, not
`cargo check` — actually verifies the dylib it bundles is a real,
non-empty compiled binary; a placeholder passes all of them. **A real gap
worth naming, not just patching around**: this graceful-degradation design
was built (see the "Build the voice engine" step's own comment) explicitly
so a Swift compile failure "never breaks Amin's whole release" — a
reasonable goal — but its actual effect was hiding three consecutive
releases' worth of the SAME failure from every automated check that
exists. A future improvement worth making: have that step still succeed
overall, but surface loudly (not just a build-log `::warning::` nobody
reads) that voice shipped broken, e.g. failing a later, non-blocking CI
check, or checking the dylib's actual size in "Rebuild latest.json" before
publishing.

Fixed by removing `private` from `AudioResampler`'s declaration — verified
the real cause the same way as the CSP bug earlier this session: not by
reasoning about what *should* work, but by downloading the next build's
actual `.app.tar.gz` and confirming `libaminvoice.dylib` is no longer
0 bytes. **Confirmed, not assumed**: the next published build's
`libaminvoice.dylib` was downloaded and inspected directly — a real
400KB universal Mach-O binary (x86_64 + arm64), not a placeholder.

### The 3D avatar's T-pose arms, properly fixed (not just hidden) (2026-08-28)

The camera-framing fix (above, same day) only hid this rig's arms-out rest
pose at the specific aspect ratio it was tested against. Mona's next real
Mac screenshot — a wider window, closer to an actual MacBook's — showed
the arms poking back into frame as odd dark triangular shapes at the
shoulders, and her reaction named the real issue precisely: **"أنا بنيت
ليك جسم كامل ليه انت دمرت الشكل كده"** (I built you a full body, why did
you destroy the shape like that). Cropping around a bad pose was never
going to hold at every window size a real Mac might use; the actual fix is
posing the body correctly, not hiding more of it.

`ThreeDAvatar.tsx` now rotates the `LeftArm`/`RightArm` bones 90° right
after the model loads — a one-time static pose correction, not part of
the per-frame idle animation — bringing both arms down from the rig's
T-pose to a natural at-the-sides stance. Confirmed with Playwright
screenshots across three aspect ratios, including a deliberately extreme
1800×650 chosen to stress-test wider-than-tested windows: a clean shoulder
silhouette with no visible artifact at any of them, not just the one this
was first checked against — the same lesson as the camera-framing fix
itself, applied this time before shipping rather than after another
screenshot.

**Correction, same day, found before it reached her this time**: the
first version of this fix rotated around each bone's local **Z** axis.
That shipped, and only got caught by pulling the camera back to a
full-body diagnostic view and actually looking — Mixamo bone-local axes
don't line up with world axes, and local-Z actually swung both forearms
to cross in front at the waist, not down to the sides (would have read as
a different, equally wrong pose once framing revealed more of the torso).
Local **X**, same rotation sign for both arms, is what a full-body
screenshot confirmed brings both arms down naturally, hands resting near
the hips. Recorded here so a future pose adjustment starts from the
verified axis instead of re-deriving it — this rig's bone-local frames
aren't documented anywhere else in this codebase.

### Removed the hard circular clip that was cutting through the shoulders (2026-08-28)

Mona's next round of feedback, itemized in detail, named the actual
remaining cause of a "PNG cutout pasted on wallpaper" look: `.amin-
presence-portrait-3d` applied `overflow: hidden; border-radius: 50%` on
top of the shared base rule's own soft radial `mask-image` — a hard
geometric circle clipping straight through the character's real
silhouette, in addition to (not instead of) the soft fade already meant to
handle edge feathering. The canvas already renders with a transparent
clear color (see the `WebGLRenderer` setup), so there was no rectangular
canvas edge that needed hiding in the first place. Removed the hard clip
entirely; the existing mask-image on the shared rule is what actually
produces a natural fade at the frame's edges now.

Also reverted a same-day change that didn't hold up: shifting the whole
`.amin-presence` layer via `transform: translateY(-8%)` (meant to create
clearance above the command bar) made the character read as smaller and
further from center once combined with the corrected arm pose and the
removed circular clip — exactly what the next round of feedback described
("الشخصية الآن صغيرة وغارقة داخل الخلفية... فراغ فوقه"). The camera's own
framing (hairline-to-collar, headWorldPos-centered, no extra offset) turned
out to need no artificial vertical shift once the two real bugs underneath
it (bad arm pose, hard circular clip) were actually fixed — confirmed with
Playwright screenshots across three aspect ratios (narrow-tall, the
original landscape, and wide-short) with the input bar sitting on dark
jacket fabric rather than visually competing with it.

Separately confirmed with a real, if crude, test rather than assumed: two
screenshots of the idle 3D avatar four seconds apart show ~53,000 changed
pixels (mean diff 1.7, max 147) — the blink/gaze/head-sway idle loop is
genuinely running, not a frozen frame, addressing "هذا ليس تمثالًا" (this
isn't a statue) for the idle state specifically. Not yet re-verified after
this round's changes: mouth-sync during an actual spoken reply, and
anything Simli/Portrait-Mode-audio-related — those need a real spoken
utterance and, for Portrait Mode, a real Mac (this sandbox's browser can't
reach Simli's WebRTC endpoint at all, a limitation already documented
above, independent of any code change here).

## ElevenLabs Arabic pronunciation audit (2026-08-28)

Mona reported Arabic pronunciation as "سيئ جدًا" (very bad) — most sentences
sounding unnatural — and asked for a real audit before any change, in a
specific format. Answers, checked against ElevenLabs' own docs and this
codebase, not assumed:

- **Model**: was `eleven_multilingual_v2`, now `eleven_v3` (see
  `elevenlabs.rs`'s `MODEL_ID` doc comment for the full reasoning).
  `eleven_multilingual_v2` does list Arabic among its 29 languages — the
  model wasn't fundamentally broken for Arabic.
- **Voice**: `DEFAULT_VOICE_ID` (Rachel, English-trained) only applies when
  no voice ID is saved in Settings. The real, session-confirmed cause of
  "every Arabic sentence sounds wrong": Mona's ElevenLabs API key had been
  pasted into the Voice ID setting (see the separate "Reject an API key
  pasted into the Voice ID field" fix, same day) — every request silently
  fell back to Rachel reading Arabic the whole time, which explains a
  *consistent*, not occasional, pronunciation problem far better than a
  model limitation would.
- **`language_code`**: not sent, and — checked directly against
  ElevenLabs' docs — explicitly documented as unsupported on
  `eleven_multilingual_v2` ("This parameter is not supported for
  multilingual_v2 models"). Not an oversight; a real, documented
  limitation of the model this app used until today.
- **Text integrity**: verified by reading `agent::strip_markdown_for_speech`
  and `fix_pronunciation_for_speech` directly — they strip markdown
  symbols/emoji and narrowly fix one name's diacritics; neither
  transliterates, re-encodes, or otherwise mangles Arabic script. The text
  ElevenLabs receives is the same Arabic text Claude wrote.
- **SSML/pronunciation dictionaries**: none used anywhere in this pipeline.
- **Voice settings**: `stability`/`style` vary by emotion (see
  `voice_settings_for_emotion`), `similarity_boost` fixed at 0.75,
  `use_speaker_boost` fixed true, `speed` never set (ElevenLabs default:
  1.0) — confirmed against ElevenLabs' documented `voice_settings` schema.

**What this audit could NOT do, honestly**: run the model/API to actually
listen to the five test sentences Mona specified. There is no ElevenLabs
API key in this sandbox, and — per her own explicit instruction — a
successful API call is not proof of correct pronunciation regardless. The
`eleven_v3` switch is a documented-Arabic-support, backward-compatible
change (its "audio tag" emotional-delivery syntax is additive to
`voice_settings`, not a replacement, per ElevenLabs' own prompting docs),
protected by the existing streaming→REST→on-device fallback chain in
`commands::speak_text` if anything about it doesn't work as expected — but
its actual pronunciation quality, and its behavior specifically over the
streaming WebSocket, are unverified until tested with a real key. **The
fix this audit is confident in regardless of model**: Mona needs a real
Arabic voice ID saved in Settings — this sandbox has no access to her
ElevenLabs account's Voice Library to choose one for her.

### A real ElevenLabs pronunciation dictionary, not text substitution (2026-08-28)

Mona tested full Arabic diacritization (تشكيل) herself and found it fixes
ElevenLabs' pronunciation — then explicitly asked for a real, permanent
ElevenLabs Pronunciation Dictionary (created via their own API, with a
real `pronunciation_dictionary_id`/`version_id` attached to every request)
rather than "a temporary text replacement," with a way to keep adding
words as new ones come up mispronounced.

**Real API, checked against ElevenLabs' own docs, not guessed**:
`POST /v1/pronunciation-dictionaries/add-from-rules` creates the
dictionary; `POST /v1/pronunciation-dictionaries/{id}/add-rules` adds more
rules to it later (returning a NEW `version_id` each time — the dictionary
is versioned as a whole, not per-rule, so the stored version must be
updated on every addition or new rules silently don't apply); every TTS
request (REST, PCM, and the streaming WebSocket's init message — checked
separately, since it's a different message shape) accepts
`pronunciation_dictionary_locators: [{pronunciation_dictionary_id,
version_id}]`.

**Alias rules, not phoneme/IPA** — `elevenlabs::PronunciationRule` only
supports the `alias` type (a plain-text substitution ElevenLabs then
reads normally, with `word_boundaries: true` so a rule for "منى" doesn't
also fire inside unrelated words like "يتمنى" — the exact bug
`agent::fix_pronunciation_for_speech` already had to hand-guard against
for the same word). This matches Mona's own finding exactly: diacritizing
"منى" into "مُنى" is a text substitution, not a phonemic instruction, and
alias rules are the one type ElevenLabs documents with no model-specific
restriction — phoneme/IPA rules have no established Arabic precedent here
to build on, and Mona's own instruction anticipated exactly this fallback
("إذا فشل phoneme-based pronunciation للعربية، استخدم alias/substitution
rules").

**A real conflict this caught and resolved**: `fix_pronunciation_for_speech`
already hand-fixed "منى" → "مُنَى" locally, before any ElevenLabs call. Left
in place unconditionally, it would have silently broken the new
dictionary's own "منى" rule — by the time the text reached ElevenLabs, the
word would already be "مُنَى", not the plain "منى" the dictionary's
`string_to_replace` looks for. Fixed by making that local fix apply only
on the on-device fallback path (which has no dictionary mechanism at all
and still needs it), never on text sent to ElevenLabs, where the
dictionary is now the single source of truth — see `commands::speak_text`.

**Developer Mode** (Settings toggle, `localStorage`-only — a per-viewer
convenience, not a shared or secret setting): shows the last reply's
original text, the actual text sent to TTS, `pronunciation_dictionary_id`,
`model_id`, and `language_code` — fired by `commands::emit_tts_debug` on
every `speak_text` call, on-device fallback included (with the
ElevenLabs-only fields `null`), so it's meaningful regardless of which
engine actually spoke.

**What's verified vs. not — same standard as every other feature today**:
`cargo test`/`tsc`/`npm run build` all pass, and the request/response
shapes match ElevenLabs' documented schema exactly. **Not verified**:
actually calling ElevenLabs' API to create the dictionary and hearing the
five test sentences Mona specified — this sandbox has no ElevenLabs API
key, and per her own explicit instruction, a successful API call is not
proof of correct pronunciation. The real test needs her to click "أنشئي
القاموس" in Settings on her own Mac, with her own key already saved, and
listen.

## Automatic Arabic diacritization for speech (2026-08-28)

Real feedback from Mona after the pronunciation dictionary shipped: she
rejected the whole "listen for a bad word, then type it into a form"
model outright — "أنا عايزه النطق يبقى بالتشكيل من نفسه مش يحتاج اني
اسمع" (I want the pronunciation diacritized on its own, not needing me to
listen). She's right that a hand-maintained list of known-bad words is
fundamentally reactive: it only ever covers the exact words someone
already noticed. The actual root cause is more general than any list —
plain Arabic text is genuinely ambiguous (the same written letters can be
several different words depending on vowels no one writes down), and
that ambiguity is what a TTS engine is guessing at on every single
sentence, not just the six names already in the dictionary.

The fix: `agent::diacritize_arabic_text` sends the text to Claude (a
separate, minimal call — `claude-haiku-4-5-20251001`, no tools, no
conversation history, chosen for speed since this is a mechanical
rewrite, not reasoning) with a system prompt that asks for full
diacritics with hard constraints (never add/remove/change a word or
letter, leave English/numbers/punctuation untouched, reply with the
diacritized text alone). `commands::speak_text` and
`commands::synthesize_pcm_for_simli` both run this on the
markdown-stripped text right before handing it to ElevenLabs — after the
`eleven_key` check, so it only runs when ElevenLabs is actually going to
speak the result, and only when Mona's Anthropic key is saved. Failure
(no key, network error, malformed response) falls back to speaking the
plain undiacritized text rather than blocking speech — this is a quality
improvement, not something that should ever be able to make Amin go
silent.

The ElevenLabs pronunciation dictionary from the previous section is
**not replaced** — it stays wired in exactly as before, alongside this.
Once a word is genuinely diacritized correctly, the dictionary's rule for
it becomes redundant but harmless (`word_boundaries: true` matching on
the plain word just won't fire against already-diacritized text). The
dictionary earns its place as the manual override for whatever the
diacritizer gets wrong on its own — unusual proper nouns especially
("المعبيلة", "درة البيان") that a general model has no way to already
know how Mona's own family/school actually pronounces.

Going forward, the path this doc actually wants Mona to use for a new
bad word is: tell Claude (in conversation, not the Settings form) — the
words get added to `elevenlabs::default_pronunciation_rules()` in code,
shipped in an update, and she presses "إعادة الإنشاء" once. The manual
"إضافة للقاموس" form in Settings stays as a fallback for when she wants
to add one herself without waiting for a build, not the primary path.

**What's verified vs. not**: `cargo test`/`tsc`/`npm run build` all pass;
new unit tests cover the token-budget formula and response-extraction
logic (`diacritization_token_budget_scales_with_length_but_never_below_the_floor`,
`extracts_the_diacritized_text_from_a_plain_reply`,
`skips_a_leading_thinking_block_to_find_the_diacritized_text`,
`an_empty_or_missing_text_block_is_an_error_not_a_blank_utterance`).
**Not verified**: an actual live call to the Anthropic API producing
correctly diacritized Arabic, or the resulting audio actually sounding
right — this sandbox has no live key to call with, and per Mona's own
standing instruction, no amount of passing tests substitutes for her
actually hearing it. The real test is Developer Mode's debug panel (see
above): after this update, the `tts_text` field there should show fully
diacritized Arabic for any reply, not just the six known words — that is
the concrete, visible thing to check for on her own Mac, separate from
whether the audio itself sounds right.

## Two real bugs from the same Mac session (2026-08-28): overlapping speech, and a face with no expressions

Mona, verbatim: "كلام الـ3D بيتداخل صوته في بعض أكثر من مرة وكإن في صوتين
داخلين جوه بعض" (the 3D's speech overlaps more than once, like two voices
inside each other) and, separately, "مفيش اي تعبيرات بتصدر من وجهه إلا فم
بيفتح لفوق وينزل ل تحت فقط... هي دي تعبيرات وجه اللي غلبتني وطلعت روحي
عشان تخليني ابنيها لك في الملف؟؟؟" (no expressions at all come from his
face except a mouth that opens and closes — is this what exhausted me
building that rig file for you?). Both real, both fixed this session.

**Overlapping speech.** `voice::AFPLAY_PID` only ever tracked the most
recently spawned `afplay` process, with nothing stopping whichever one was
already running before a new `speak_text` call spawned another —
`elevenlabs::play` just wrote a new pid over the old one. If `speak_text`
ever ran twice in close succession for any reason (a real candidate,
though not confirmed as *the* trigger: `voice://final` and
`voice://hands-free-barge-in` are two separate event listeners in
`App.tsx` that can both call `handleSendToAgent`, and its `agentBusy`
guard is a React state read that two handlers firing in the same tick
could both see as stale `false`), the first `afplay` kept playing while
the second one started — the same reply, twice, out of sync. Fixed with
`voice::kill_current_afplay()`, called unconditionally at the top of
`elevenlabs::play` before spawning anything: at most one `afplay` process
can ever be alive now, regardless of what caused a second `speak_text`
call. `stop_speaking` was refactored to call the same function rather
than duplicating the kill logic.

**No facial expressions.** Two separate gaps, both real, both needed
fixing together — fixing only one would have looked the same as fixing
neither:
1. `AminPresence` tracked an `emotion` prop the whole time but never
   passed it to `ThreeDAvatar` — the 3D renderer had literally no way to
   know Claude's tagged mood for a reply.
2. Even with emotion wired through, nothing in `ThreeDAvatar`'s
   `animate()` loop ever touched a brow or mouth-shape blendshape tied to
   state or emotion — only blink, gaze, head sway, and jaw/viseme (the
   mouth Mona was seeing) were ever driven.

Fixed by adding `EMOTION_EXPRESSIONS` (one resting expression per each of
`agent.rs`'s 8 real, disclosed emotions) and `STATE_EXPRESSIONS` (one per
`AminState`), combined additively and eased toward every frame — using
only ARKit blendshape names the blink/gaze/mouth logic doesn't already
own, so the two systems never fight over the same morph target.

**A real near-miss in verifying this, worth recording honestly**: the
first pass at intensities (~0.3-0.55) looked completely invisible in a
640×640 test render — brows, frown, cheek squint all appeared to do
nothing even at those values, while only `mouthSmile`/`mouthPress`
visibly worked. Before concluding those blendshapes were broken, the
morph target's own vertex data was inspected directly from the .glb (not
assumed): `mouthSmileLeft`'s sparse morph target moves ~2500 vertices by
at most ~8mm, versus `jawOpen`'s ~35mm — real, working displacement, just
smaller. Values were raised toward 0.6-0.9 to compensate, and even then a
full-frame 640×640 screenshot of "concerned" still looked barely
different from neutral to the eye — until the exact same render was
cropped and zoomed to just the brow/mouth region, where the lowered brows
and flattened mouth were clearly, unambiguously visible. The lesson kept
from this: a screenshot judged as "no visible change" at the wrong
viewing scale is not evidence the underlying value isn't working — the
same render, viewed at the scale a person's eye would actually focus on,
told a different story. The values shipped (0.55-0.9 depending on shape)
were calibrated against zoomed crops of this exact model, not a generic
assumption about what "0.6" should look like on any rig.

**What's verified vs. not**: `cargo test` (109 passed), `tsc`, and
`npm run build` all pass. The expression system was verified visually —
a temporary, untracked test harness (`facetest.html`/`.tsx`, deleted
before committing) rendered `ThreeDAvatar` directly with forced
`state`/`emotion` combinations outside the normal Tauri-gated app flow,
confirmed via pixel-diffing against a neutral baseline and by directly
reading live `morphTargetInfluences` values off the loaded mesh — real
evidence, not assumed. **Not verified**: the audio-overlap fix, since
reproducing it needs a real Mac's audio stack and the exact conditions
that triggered the original double-`speak_text` call, neither of which
exist in this sandbox; the fix (never allow two `afplay` processes at
once) is unconditionally correct regardless of what caused the second
call, but whether the underlying double-invoke can still happen at the
React layer is a separate, still-open question worth watching for.

## Portrait Mode's static image was cropping into the hair on real window shapes (2026-08-28)

Mona, from a real Mac screenshot: "محتاجة تضبط راسه المقصوصة دي من فوق"
(need you to fix this head that's cropped from the top). Real, and a
plain CSS bug in `.amin-presence-portrait`'s `object-position` — the
vertical value (42%) still cropped into the source photo's hairline once
her actual window's aspect ratio squeezed the presence panel short
enough. Checked the source photo directly rather than guessing: hair
starts about 12% down the 1254×1254 square, so 42% left far less margin
than it looked like it should. Lowered to 18%, verified at three
deliberately extreme aspect ratios (1600×600, 1400×480, 1280×800) via
Playwright screenshots — full headroom stays visible at all three, none
crop into the hair.

**This is Portrait Mode's *static* image specifically** — worth being
explicit about, since Mona's same message also expected the facial
expression and mouth-sync work from the 3D-mode fix (see the section
above) to already apply here too. It doesn't, and can't by construction:
Portrait Mode shows either Simli's live video (real lip-sync, from
Simli's own service, once a session actually connects) or, whenever
Simli isn't connected, this same flat photograph — which cannot move at
all, expression or mouth, because it's an image, not a 3D model. That's
the existing designed fallback (see `PortraitAvatar.tsx`), not a
regression from this session's 3D-mode work. Until her Simli session
connects successfully, Portrait Mode will keep showing a still image;
3D Mode is the only visual mode the new expression/mouth-sync/audio-
overlap fixes apply to.

## Roadmap (for orientation — each phase gets its own design notes)

| Phase | Scope |
|---|---|
| 0 | Architecture, security foundation, design system *(this doc)* |
| 1 | Desktop shell (menu bar), push-to-talk voice, speaker recognition + presence greeting, Agent Core (Anthropic API wiring) |
| 2 | Task management + Quick Capture, scoped file access, minimal browser control *(all shipped — see Phase 2 notes above for what's still a conservative default vs. a settled decision)* |
| 3 | Local Delta Brief *(shipped — see Phase 3 notes below)*; Gmail, Calendar, real Morning Brief still blocked on Mona creating a Google OAuth client — see the account-setup checklist |
| 4 | Follow-up Engine *(local logic shipped — see Phase 4 notes above)*, delivery channels + Executive Delegate Mode still ahead |
| 5 | Durrat Al-Bayaan school platform connector (specific read/action tools only — see SECURITY.md on why this is never a direct DB/code link) |
| 6 | Ads, Drive, developer workflows, Smart Home connector (Philips Hue-style lighting/outlets — same connector pattern as Gmail/Calendar) |
| 7 | Mobile Companion (iOS, personal install only) — remote control over a private VPN mesh to the same core; see "Mobile Companion" above |

## The new expression system fought with the speaking mouth (2026-08-28, same day)

Found immediately from a real Mac screenshot Mona sent right after the
expression fix shipped: mid-speech, jaw wide open, teeth showing far more
than a normal open mouth — "هي دي تعبيرات الفم المتطابقة مع الكلام؟؟؟؟؟؟؟"
(is *this* the mouth expressions that match the speech?). Real: the reply
was tagged `happy` (`mouthSmileLeft`/`Right` at 0.9), and while she was
actively speaking, `jawOpen` was simultaneously driven up to 0.55 by the
audio-reactive viseme animation. Both landed on overlapping mouth
geometry at the same time — a wide-open jaw *and* corners pulled up into
a hard smile — and stacked into a distorted, stretched shape instead of
reading as "happy while talking."

Fixed by having `combineExpressions` zero out every mouth-shape target
(`mouthSmileLeft/Right`, `mouthFrownLeft/Right`, `mouthPressLeft/Right`,
`mouthShrugUpper` — `MOUTH_SHAPE_NAMES`) whenever `isSpeaking` is true,
handing the mouth entirely to the jaw/viseme animation for the duration
of the utterance. Brow/eye/cheek expression keeps running underneath
unaffected — "happy" still reads in the eyes and brows while Amin talks,
it just stops fighting the mouth shape speech already owns.

Verified: `tsc`/`npm run build` clean. Visually re-confirmed with the
same temporary test harness as the original expression fix (state=
speaking, emotion=happy, `audioLevelBus.setAudioLevel` driven to simulate
loudness): the mouth now shows a normal open-speaking shape with no
smile distortion, while the same emotion at `state=idle` still shows the
full smile exactly as before — the suppression is speech-gated, not a
blanket removal of the happy expression.

## Hands-free without a wake/close phrase, once a voiceprint is enrolled (2026-08-28)

Mona, verbatim: "أنا عايزه الاستماع الحر يكون مش مربوط بكلمة بداية وكلمة
نهاية من فضلك و دلوقتي حالا بفعل له بصمة الصوت بتاعتي" (I want hands-free
listening not tied to a start word and an end word please, and I'm
enrolling its voiceprint right now). Once her own verified voice is the
real security gate, requiring a spoken phrase on top of it is redundant
friction, not extra safety — so the wake/close phrases stop being the
primary flow the moment a voiceprint exists.

`HandsFreeListener.openTap` now branches on
`VoicePrintEngine.shared.hasEnrolledSpeaker()` the moment hands-free
starts:
- **Enrolled** (the intended path going forward): `runVerifiedListening`
  runs continuously, no wake or close phrase at all. Every finalized
  utterance is checked against the enrolled voiceprint (the same rolling-
  buffer snapshot + `VoicePrintEngine.verify` the old wake-phrase gate
  used, just run on every utterance instead of only ones containing a
  phrase); a match is sent straight through as a command, a mismatch is
  silently discarded and listening continues. Barge-in handling
  (`isLikelySelfEcho`) is unchanged.
- **Nothing enrolled** (`armPassive`/`openActiveSession`, unchanged): the
  original phrase-gated flow, kept as the fallback for whenever there's no
  voiceprint to gate on. Dropping the phrase gate with no voice gate
  either would mean anyone in earshot commands Amin — the fallback exists
  specifically so removing friction for Mona never means removing
  security when she hasn't set the replacement up yet.

The Settings UI (App.tsx) reflects both flows: the description text,
the wake/close phrase field labels (marked "احتياطي" — fallback — once
enrolled), the command-bar's hands-free tooltip, and the voice-rejection
banner (`voice://hands-free-voice-rejected`) all branch on
`speakerEnrolled` now — that last one specifically because in the new
continuous flow a "rejection" fires on every ordinary bit of background
speech that isn't her (a real, expected, frequent event, not the rare
"someone said the phrase in the wrong voice" case it used to mean), so
surfacing it as a visible error banner there would itself be the kind of
noisy, intrusive behavior hands-free mode exists to avoid.

**What's verified vs. not**: `tsc`/`npm run build` pass, and the new
`speakerEnrolled` dependency was added to the `useEffect` whose closures
read it (an easy staleness bug to miss otherwise — the array previously
only listed `handsFreeEnabled`). **Not verified, and structurally can't
be from this sandbox**: the Swift compiles at all. There is no Swift
toolchain here (`swiftc` isn't installed — checked directly, not
assumed), matching this file's own long-standing header disclosure, and
`build-macos.yml`'s "Build the voice engine" step still has the same
always-exits-0 fallback design documented earlier in this file (a
compile failure silently bundles an empty placeholder dylib instead of
failing the build) — exactly the gap that let three earlier releases ship
a non-functional voice engine undetected. The new code reuses
`VoicePrintEngine.shared.verify`/`hasEnrolledSpeaker` exactly as
`armPassive` already calls them (a call site that has shipped and been
confirmed working via a real dylib inspection before), and was read
end-to-end for brace balance and structural correctness, but the only
real confirmation is the same one this project settled on after the
`AudioResampler` incident: download the actual published `.dylib` after
this ships and inspect it directly, never trust the CI checkmark alone.

## Amin becomes voice-only, and the voiceprint threshold's first real test failed (2026-08-28)

Two real, separate reports arrived together, both urgent:

**Hands-free stopped responding entirely.** "الاستماع الحر مش شغال نهائي و
ناديت عليه مليون مرة مفيش رد أبدا أبدا" (hands-free doesn't work at all, I
called it a million times, never any response) — and the timing matches
her having just enrolled a voiceprint ("أنا خلاص فعلت البصمة الصوتية")
exactly. Before enrollment, `VoicePrintEngine.verify` fails open (anyone's
voice opens a session); the moment a voiceprint exists, it starts actually
gating on `matchThreshold` — and that threshold was always a documented,
unmeasured placeholder (0.45, "chosen from published literature... not
from any recording of Mona's actual voice"). This is exactly the failure
mode that placeholder's own comment predicted, now real instead of
hypothetical: her enrolled voiceprint most likely isn't matching her own
live voice at that threshold, silently rejecting every wake phrase she
says. Two fixes, not one:
- Lowered `matchThreshold` to 0.25 as an immediate, still-unmeasured
  correction — biased toward false-accepts over false-rejects, since a
  stranger occasionally getting through is a far smaller problem than
  Mona locked out of her own hands-free mode, which is what just actually
  happened.
- Added `VoicePrintEngine.verifyWithScore`, returning the real cosine
  similarity number alongside the match decision. Both `armPassive` (the
  phrase-gated fallback) and `runVerifiedListening` (the phrase-free flow)
  now pass this score through kind 10's text instead of leaving it empty,
  and the Settings banner for a rejection (App.tsx) shows it directly —
  "نسبة التطابق: 0.31", say — so the *next* rejection, if there is one,
  comes with a real number to tune against instead of another guess.
- **Immediate workaround, no update needed**: clearing the enrolled
  voiceprint (Settings → "مسح") reverts to fail-open immediately, restoring
  hands-free to how it worked before enrollment, while any code fix still
  needs a build+update cycle to reach her.

**Voice-only, no visible chat interface at all.** Mona, explicitly and
repeatedly: "أنا عايزه اكلمه صوت فقط... مفيش خاصية مايك يتقفل ويتفتح ومفيش
زر ارسال... الكلام أصلا مش هيكون رسايل" (I want to talk to him by voice
only — no mic-toggle affordance, no send button — speech isn't messages
at all). Removed from the main screen entirely: the floating command bar
(mic-toggle button, stop-speaking button, text input, quick-capture
button, send button) and the chat-bubble transcript (`agent-log`). Talking
to Amin is now exclusively: hands-free mode (Settings toggle) for
continuous voice, or the existing global `alt+A` push-to-talk shortcut
(registered natively in `lib.rs`, works from anywhere, was never tied to
the now-removed button) as the manual fallback. The underlying
`handleSendToAgent`/voice pipeline is untouched — only the visible
UI and the now-dead `handleMicToggle`/`handleMicDown`/`handleMicUp`/
`handleStopSpeaking`/`handleQuickCapture`/`agentLog` state were removed,
along with their now-unused imports (`tsc`'s own unused-declaration
errors caught every one of these mechanically, not manual guessing about
what was still needed).

One real gap this created and fixed in the same change: with no chat log
to show a failure in, a silent `catch` block would make a real error in
`handleSendToAgent`/`handleNarrateBrief` completely invisible — no visual
trace, no sound, nothing. Both now call `speak()` with the error text on
failure, so a real failure is at minimum heard, never silently swallowed.

**What's verified vs. not**: `tsc`/`npm run build` clean (including
chasing every cascading unused-import/state error `tsc` raised after the
UI removal — a good forcing function for finding genuinely dead code, not
just satisfying the compiler). A Playwright screenshot at 1280×800
confirms `.command-bar` and `.agent-log` no longer exist in the DOM at
all, not just hidden by CSS. **Not verified, and structurally can't be
from this sandbox**: whether the Swift changes (`matchThreshold`,
`verifyWithScore`) actually compile — same standing limitation as every
other Swift change this session (no `swiftc` here, and
`build-macos.yml`'s voice-engine step still silently bundles an empty
placeholder dylib on a compile failure). Real confirmation needs
downloading the published dylib after this ships. Also not verified:
whether 0.25 is actually a good threshold for Mona's real voice — that
still needs a real test on her Mac, same as before, just with a real
number to read this time if it fails again.

## Automatic error reporting to GitHub (2026-08-28)

Mona: "أنا عايزاك تبني طريقه في الكود ان لما يكون في خطأ في النظام مبني
يصير تواصل مباشر بين امين و بينك يبلغك الخطأ عشان تصلحه" (build a way so
that when there's an error in the built system, there's direct
communication between Amin and you [Claude] so you're told about the
error and can fix it). Told her plainly what's actually buildable here:
there is no standing server for the app to call into, so instant,
literal Amin-to-Claude messaging isn't something this architecture can
offer. What's real: `error_report::report` (new module) files a GitHub
issue on Amin's own repo the moment a real backend failure happens,
labeled `amin-auto-report`, entirely optional (nothing runs, no issue is
ever filed, unless Mona pastes a GitHub token into Settings — new
`github_token` setting, same save/clear/has pattern and same local-only
storage disclosure as every other key). A new hourly Routine (this
session, self-bound) checks for new issues with that label and works
them: reads, diagnoses from the real error text in the body (never
guesses), fixes and commits locally if a real fix is possible — still
never pushes without Mona's explicit approval, the same standing rule
every other change in this doc has followed — then comments and closes
the issue.

Deliberately conservative about what triggers a report — this is for
real, actionable backend failures only (an Anthropic API call that failed,
both ElevenLabs synthesis paths failing), never for expected/user-facing
conditions (a badly-formatted key, a declined confirmation, a voice
mismatch) that already show her a clear message directly and would just
be noise here. A 1-hour in-process dedup window per failure category
(`DEDUP_WINDOW`) stops a repeatedly-failing subsystem from flooding the
repo with duplicate issues.

**What's verified vs. not**: `cargo test` (112 passed, including new
tests for the dedup window), `tsc`, `npm run build` all pass. Fixed one
real bug found by the compiler, not by inspection: the first version held
a `MutexGuard` across the `.await` inside `error_report::report`, which
`tauri::generate_handler!` requires to be `Send` — wrapping the guard's
scope in a block so it drops before the await fixed it; `cargo check`
caught this immediately as a hard compile error, not a runtime surprise.
**Not verified**: an actual GitHub API call creating a real issue (no
token exists yet to test with), and whether the new hourly Routine
actually retains GitHub tool access when it fires — it's a self-bound
Routine (resumes this exact session, not a fresh restricted one), which
should carry this session's existing GitHub access forward, but the
Routine's own prompt was written to self-diagnose and tell Mona plainly
if that assumption turns out wrong rather than silently doing nothing
forever.

## Hands-free never responded — a hard-coded on-device-only requirement (2026-08-28)

Mona, after enrolling her voiceprint and updating to the phrase-free
hands-free flow: "ناديته مليون مره لحد دلوقتي ولا مره رد عليا" (I've
called it a million times so far, not once has it responded) — total
failure, not a tuning problem.

Found the real cause by reading `HandsFreeListener.start()` next to
`Transcriber.start()` (push-to-talk) side by side: hands-free had a hard
guard — `recognizer.supportsOnDeviceRecognition` — that push-to-talk
never had. `supportsOnDeviceRecognition` is `false` whenever the Arabic
(ar-EG) offline dictation language pack isn't downloaded on the Mac in
question, which macOS does not make obvious and Mona would have had no
way to notice. When that guard failed, `start()` returned immediately
with a `voice://error` event and never opened the audio engine at all —
no tap, no listening, nothing could ever have been heard, no matter how
many times she spoke. This explains a 100% failure rate independent of
wake phrase, voiceprint, volume, or anything else tried so far.

Push-to-talk's `Transcriber.start()` already had the right pattern:
pass `recognizer.supportsOnDeviceRecognition` through to
`req.requiresOnDeviceRecognition` and let the Speech framework fall back
to server-based recognition when on-device isn't available, instead of
refusing to start. `HandsFreeListener.start()` now does the same —
dropped the hard guard, kept only the `recognizer != nil` check (a
locale genuinely unsupported at all, a different and much rarer
failure).

**What's verified vs. not**: `cargo test` (112 passed) and `tsc` pass;
brace/paren counts checked by hand since there's no `swiftc` in this
sandbox — same limitation as every other Swift change in this doc, same
mitigation (download and inspect the real dylib after this ships).
**Not verified**: whether her Mac actually lacks the on-device Arabic
language pack (the mechanism this fix addresses) or whether hands-free
was failing for some other, still-hidden reason — this is the most
plausible cause found by inspection, not a confirmed diagnosis from a
log on her machine. If hands-free still doesn't respond after this
update, the next step is getting real data from her Mac (a Developer
Mode log line, or asking her to check System Settings → Keyboard →
Dictation for Arabic) rather than guessing again.

## Hands-free's real problem might have been zero feedback, not zero function (2026-08-28)

The on-device-recognition fix (0.2.29) shipped, was verified as actually
present in the published dylib (downloaded the real release asset,
checksum-matched it, and `strings`-grepped it for both the new error
message and the expected exported symbols — all present, so this wasn't
another CI-masking-flaw placeholder), and Mona still reported hands-free
completely unchanged: no visible difference on Amin, no sound, and
pressing alt+A "did nothing" too.

That alt+A result turned out to be expected, not another data point: the
global-shortcut handler in `lib.rs` deliberately refuses push-to-talk
while `HandsFreeSession::is_active()` is true (the two native listeners
can't run at once) and emits a `voice://error` explaining that instead —
if hands-free was toggled on at the time, alt+A doing nothing new is the
designed behavior, not evidence of a second bug.

The real, separate finding: **there was no way to actually notice
hands-free arming even if it worked perfectly.** Two gaps compounded:

1. `armed`'s only visual signal was a brow/eye blendshape at 0.15-0.2
   intensity — well under the ~0.5+ this session already found necessary
   for this model's small blendshape deltas to read as visible at all
   (see "Real, hard-won finding" earlier in this doc). Bumped `armed`
   and `listening` in `ThreeDAvatar.tsx`'s `STATE_EXPRESSIONS` into the
   same visible range every other calibrated expression uses.
2. Removing the chat UI for the voice-only redesign also removed the
   only feedback hands-free ever had — the input box's live partial
   transcript. Nothing replaced it. Added a short system-sound chime
   (`afplay` of a bundled macOS system sound, not ElevenLabs — so it
   fires even if TTS keys are broken) the instant hands-free actually
   arms (kind 5) and again the instant it hears and accepts a real
   command (kind 1), in `voice.rs`'s `on_voice_event`. Deliberately not
   tracked by `AFPLAY_PID`/`kill_current_afplay` — it's a cue, never
   "Amin speaking".

This doesn't rule out a real underlying listening failure too — it's
still possible the native pipeline itself isn't hearing her. But it was
no longer possible to tell the two apart from her side: "nothing
happened" was equally consistent with "hands-free never started" and
"hands-free started fine and is just invisible." The chime turns the
next report into real signal: silence on toggling it on means arming
itself is failing; a chime on arming but nothing on speaking narrows it
to recognition/verification specifically.

**What's verified vs. not**: `cargo build`/`cargo test` (112 passed) and
`tsc` pass. **Not verified**: whether the underlying native listening
actually works now on her Mac — that's still an open question this
change is designed to get a real, unambiguous answer to on the next
test, not a claim that hands-free itself is fixed.

## The armed chime worked — narrowing to recognition/verification (2026-08-28)

Mona confirmed hearing the new "armed" chime (0.2.30), which rules out
the engine failing to start at all. She still heard nothing when she
actually spoke to it — no "heard you" chime, no reply. That narrows the
problem to one of two places, and there was no way to tell them apart
from outside the app: (1) speech recognition genuinely isn't
transcribing anything, or (2) it's transcribing fine but
`VoicePrintEngine.verifyWithScore` keeps rejecting her own voice —
which, once a voiceprint is enrolled, is *deliberately* silent (no
banner, no chime) so ordinary background noise doesn't interrupt hands-
free mode. That silence is exactly right for normal use and exactly
wrong for debugging this: a real false-rejection looks identical to "not
listening at all" from her side.

Rather than guess a fourth time, extended the existing Developer Mode
panel (`App.tsx`, already used for TTS debug info) to always record —
never shown unless she turns Developer Mode on — the last partial
transcript hands-free heard (`voice://partial`) and the last voiceprint
match score on any rejection (`voice://hands-free-voice-rejected`),
regardless of whether a banner is shown for it. Turning Developer Mode
on and trying hands-free again now gives a real three-way split instead
of another blind fix: nothing in either field means recognition itself
never produced a result; a partial transcript with no rejection score
recorded means something else swallowed it after recognition; a partial
transcript *and* a low match score means the fix is tuning
`matchThreshold` (or diagnosing why her enrolled embedding doesn't match
her own voice well) — a completely different, much narrower problem than
"hands-free doesn't work."

**What's verified vs. not**: `tsc` and `npm run build` pass. **Not
verified**: which of the three cases above is actually happening — this
change exists specifically to answer that from real data on her Mac
instead of another guess.

## Found it: hands-free's recognition task never actually finalized (2026-08-28)

The Developer Mode diagnostic (previous entry) answered the question
immediately: Mona's real words showed up verbatim as the last partial
heard, with no rejection ever recorded either. That rules out both
"recognition isn't hearing her" and "the voiceprint keeps rejecting
her" — the only thing left is that `result.isFinal` from
`SFSpeechRecognizer` simply never arrived, so the entire final-only code
path (`armPassive`/`runVerifiedListening`/`listenForCommand`'s
voiceprint check and command dispatch) was never reached, for any
utterance, ever.

The real difference from push-to-talk (which works): `Transcriber`
(push-to-talk) calls `request.endAudio()` the moment the key is
released, which reliably tells Apple's recognizer the utterance is over
and a real `isFinal` follows. Hands-free's `openTap` keeps one
continuous audio engine running across the whole session and never
calls `endAudio()` per utterance — there's no natural moment to call it,
since nothing tells the code when Mona stops talking except the
recognizer's own silence detection, which turned out not to fire in
this shape on a real Mac.

Fixed in `runRecognition` (used by all three hands-free listening
methods, not `Transcriber`, which is unaffected and still relies on its
own real `endAudio()`): track the last partial transcript and the time
it last changed; if 1.2 seconds pass with nothing new, treat that last
partial as final ourselves and cancel the now-redundant real task
(which would otherwise keep running unheard in the background forever,
since it was never going to finalize on its own). This replaces trusting
Apple's finalization for a never-ending recognition task with detecting
the same signal — she stopped talking — directly.

**What's verified vs. not**: `cargo build`/`cargo test` (112 passed) and
`tsc` pass; brace/paren-balance checked by hand (no `swiftc` in this
sandbox, same limitation as every other Swift change here — real
confirmation needs the published dylib, same discipline as always).
1.2 seconds is a first guess at the silence threshold, not a measured
value — too short would cut off a mid-sentence pause as if she'd
finished; too long would feel sluggish. **Not verified**: the actual
value on a real Mac, and whether the diagnosed cause is complete (it
explains the exact symptom observed, but a real device may still surface
something this reasoning didn't anticipate).

## Hands-free worked — and Amin immediately started talking to himself (2026-08-28)

Minutes after Mona confirmed hands-free finally responds ("اشتغل"), a
new, worse failure: Amin speaking endless strange sentences on his own
("أنا أداة نصوص عربية وأنا بشكل الكلام...") with her not talking to him
at all. Two real bugs compounding, both direct consequences of things
that had just started working:

**Bug 1 — the diacritization model sometimes answers instead of
diacritizing.** The sentences she quoted are near-verbatim the
`DIACRITIZATION_SYSTEM_PROMPT`'s own self-description ("أنتِ أداة تشكيل
نصوص عربية، مش مساعد محادثة") — i.e. the Haiku diacritization call,
handed a conversational sentence, occasionally replies *as* that persona
instead of returning the diacritized text, and that reply silently
replaced Amin's actual words in `speak_text`. A system prompt is an
instruction, not a guarantee. The guarantee is now enforced in code:
`diacritization_preserves_text` strips all harakat and whitespace from
the model's output and requires the remaining letters to equal the
input's exactly — a valid diacritization can only add harakat, so
anything else (an answer, a rewording, a truncation) fails and the
caller falls back to the plain undiacritized text. Two new unit tests,
one using the exact observed failure.

**Bug 2 — the self-conversation loop.** With silence-finalization now
actually producing finals, Amin's own played-back voice became input:
(a) during playback, any echo fragment that slipped past
`isLikelySelfEcho`'s text comparison fired the barge-in path (kind 9),
which had NO voiceprint check anywhere downstream and went straight to
the agent as a command; (b) just after playback, `setSpeakingText(nil)`
wiped the echo comparison text while the recognizer still held the tail
of Amin's own voice, whose silence-final then fired ~1.2s later with
every echo defense already disarmed. His own words became commands,
whose replies became commands. Fixed both: all three barge-in paths now
verify the voiceprint before emitting kind 9 (a mismatch is discarded
like an echo, with the score surfaced via kind 10), and the last spoken
text is kept for a 3-second grace period after playback ends so
`isLikelySelfEcho` keeps catching the tail — with the phrase-free flow's
final path now also running the echo check (it previously only ran
while `currentlySpeakingText` was still set, which is exactly when the
tail-final doesn't fire).

**What's verified vs. not**: `cargo test` (114 passed, 2 new) and `tsc`
pass; Swift brace/paren-balance checked by hand (no `swiftc` here, same
limitation and same published-dylib verification discipline as always).
**Not verified**: whether the enrolled voiceprint actually rejects
Amin's ElevenLabs voice at the current 0.25 threshold (unknowable
without a real Mac) — which is exactly why the echo grace period exists
as a second, voiceprint-independent defense; and the 3s grace period
itself is a first estimate, like the 1.2s silence threshold before it.

## Non-goals (Phase 0, and generally)

- No public app-store presence for either app, ever.
- No direct database or code coupling to the Durrat Al-Bayaan school
  platform (`durrat-bayaan-connect`) — it is an external system, integrated
  later (Phase 5) only through explicit, narrow tools.
- No banking or payment capability, at any phase, at any autonomy level.
