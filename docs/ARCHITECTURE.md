# Amin — Architecture (Phase 0)

Amin is a personal Executive AI Agent: a **macOS desktop app**, not a chatbot,
not a mobile app. Its loop is:

> Observe → Understand → Decide within policy → Execute → Follow up → Report

This document is the Phase 0 baseline: the shape of the system, the tech
choices behind it, and why. It gets extended, not rewritten, as later phases
land.

## Why desktop-only, why Tauri

- **No mobile.** No Apple App Store, no Google Play, ever. Amin holds
  standing access to email, calendar, files, and (later) a browser session —
  that is not something to ship through a mobile app review process or run
  on a phone's OS sandboxing model. `src-tauri/src/lib.rs` has no mobile
  entry point at all; there is no `gen/android` or `gen/ios` in this repo,
  and there should never be one.
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

### Availability model: push-to-talk now, wake word later

Phase 1 ships **push-to-talk only** (a shortcut/menu-bar click arms
listening) — always running in the background, never a fully-quit app, but
never listening to raw audio until asked to. Continuous listening / wake
word is a later, separate milestone gated on the voice privacy model in
docs/SECURITY.md actually holding up in practice — it is not something
Phase 1 turns on by default.

## Repo layout

```
src/                      React + TypeScript frontend (the webview)
  styles/tokens.css        design tokens: color, type, spacing, motion
  styles/global.css        base resets, applies tokens
  components/orb/          the Living AI Core (see Design System below)
  lib/tauri.ts             the ONLY place the frontend calls invoke() —
                            typed wrappers around every Rust command
  App.tsx                  Phase 0 shell: orb preview + security panel

src-tauri/                 Rust backend
  schema.sql                the local SQLite schema (settings, audit_log,
                             tasks, follow_ups)
  src/db.rs                 opens the on-device DB, applies schema.sql
  src/secrets.rs            OS Keychain wrapper (via the `keyring` crate) —
                             the only place a secret is ever read or written
  src/policy.rs             risk tiers, autonomy levels, the excluded-domain
                             list (banking, payments, ...)
  src/audit.rs              append-only audit log writer
  src/commands.rs           every #[tauri::command] the frontend can call
  src/tray.rs               menu-bar tray icon + Show/Quit menu
  src/lib.rs                wires plugins, DB, tray, and commands together

docs/                       this file, SECURITY.md
.env.local.example          dev-only convenience; see SECURITY.md
```

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

## Roadmap (for orientation — each phase gets its own design notes)

| Phase | Scope |
|---|---|
| 0 | Architecture, security foundation, design system *(this doc)* |
| 1 | Desktop shell (menu bar), push-to-talk voice, speaker recognition + presence greeting, Agent Core (Anthropic API wiring) |
| 2 | Browser control, file access, task management |
| 3 | Gmail, Calendar, Morning Brief |
| 4 | Follow-up Engine, Executive Delegate Mode |
| 5 | Durrat Al-Bayaan school platform connector (specific read/action tools only — see SECURITY.md on why this is never a direct DB/code link) |
| 6 | Ads, Drive, developer workflows, Smart Home connector (Philips Hue-style lighting/outlets — same connector pattern as Gmail/Calendar) |

## Non-goals (Phase 0, and generally)

- No mobile build, no app store presence.
- No direct database or code coupling to the Durrat Al-Bayaan school
  platform (`durrat-bayaan-connect`) — it is an external system, integrated
  later (Phase 5) only through explicit, narrow tools.
- No banking or payment capability, at any phase, at any autonomy level.
