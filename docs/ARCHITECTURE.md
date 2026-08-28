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
installed `.app`, not a code bug inside it. This release's only bundling
change was adding the ECAPA-TDNN model above as a second
`bundle.resources` entry, referenced via a path escaping `src-tauri`
(`"../macos/transcriber/Resources/ECAPA_TDNN.mlpackage"`). Tauri's own
docs say array-format resources bundle with "the original directory
structure preserved" — for a source path that starts with `../`, what
that resolves to inside the app's `Resources/` folder is genuinely
unclear, and the timing (this is the first release where any voice
feature failed to load at all, immediately after this entry was added) is
strong circumstantial evidence it broke resource bundling for the *whole*
`Resources/` folder, not just its own entry. Not proven with certainty —
no way to inspect a shipped `.app`'s actual bundle contents from this
sandbox — but treated as the working theory rather than left unaddressed.

Fixed by removing the cross-directory reference entirely: a new CI step
("Copy the voiceprint model into src-tauri") copies the committed
`macos/transcriber/Resources/ECAPA_TDNN.mlpackage` into `src-tauri/`
(git-ignored there, same treatment as `libaminvoice.dylib` itself — a
build-time copy, never a source file) right before the bundling step, and
`tauri.conf.json`'s resource entry is now a plain `"ECAPA_TDNN.mlpackage"`
— matching `libaminvoice.dylib`'s own entry, which never had this problem.
**Not yet verified against the actual failure** — the only way to confirm
is Mona's next update actually loading voice again — but avoiding an
ambiguous, only-lightly-documented resource-path shape in favor of the
exact pattern already proven to work is the right fix regardless of
whether this specific causal theory is 100% correct.

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

## Non-goals (Phase 0, and generally)

- No public app-store presence for either app, ever.
- No direct database or code coupling to the Durrat Al-Bayaan school
  platform (`durrat-bayaan-connect`) — it is an external system, integrated
  later (Phase 5) only through explicit, narrow tools.
- No banking or payment capability, at any phase, at any autonomy level.
