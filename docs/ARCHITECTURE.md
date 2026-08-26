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

**What's still unverified:** everything here is covered by unit tests (49
passing, including `tools.rs`'s dispatcher exercised end to end with a
mocked Tauri `AppHandle`) and a clean `cargo check`/`clippy`/`tsc`/`vite
build`, but the actual back-and-forth — Claude proposing a real tool call,
Mona typing "موافقة" back, Amin executing and narrating — has not been
run against the live Anthropic API or on a real Mac. That's the natural
next thing to try together once she's at the keyboard.

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
