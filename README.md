# أمين — Amin

A personal Executive AI Agent, built as a **macOS desktop app** (Tauri +
React/TypeScript + local SQLite). Not a chatbot: the loop is

> Observe → Understand → Decide within policy → Execute → Follow up → Report

No mobile app, no App Store / Google Play — ever. See
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for why and how, and
[`docs/SECURITY.md`](docs/SECURITY.md) for the permission model, the
excluded-actions list (banking, full stop), and everything else
non-negotiable about how Amin is allowed to act.

## Status: Phase 0 done, Phase 1 in progress

**Phase 0 — Architecture, Security, Design System:**

- The Tauri + React/TypeScript project shell, as a menu-bar app
  (`src-tauri/src/tray.rs`) rather than a Dock app.
- A local, on-device SQLite schema (`src-tauri/schema.sql`) with an
  append-only audit log.
- Secrets in the OS Keychain (`src-tauri/src/secrets.rs`) — never in Git,
  never in the app's own storage.
- A policy engine (`src-tauri/src/policy.rs`): risk tiers
  (Auto / Trusted Delegation / Confirm High-Risk / Excluded) and autonomy
  levels (Observe / Assist / Delegate / Autopilot).
- The design system — Deep Navy / Soft Gold / Ivory / Charcoal tokens
  (`src/styles/tokens.css`) and the Living AI Core / Orb component
  (`src/components/orb/`) with its full state set (listening, thinking,
  planning, executing, speaking, success, warning, waiting, idle).
- Creator attribution (`src/lib/branding.ts`) on the launch screen and the
  in-app About panel.

**Phase 1 — Agent Core (in progress):**

- `src-tauri/src/agent.rs` calls the Anthropic API (Claude Opus 5) with the
  Keychain-stored key; the "Talk to Amin" panel in the app exercises it
  end to end, gated by the kill switch and fully audited.
- Voice input (push-to-talk, speaker recognition, presence greeting) is
  designed but not yet built — see docs/ARCHITECTURE.md's "Agent Core"
  section for why (it needs a real Mac and microphone to build safely).

Later phases (browser/files/tasks, Gmail/Calendar, follow-ups, the school
platform connector, ads/Drive/dev workflows, Smart Home, Mobile Companion)
build on top of this — see the roadmap table in `docs/ARCHITECTURE.md`.

## Development

```bash
npm install
npm run tauri dev    # full desktop app (requires macOS + Xcode CLT for a
                      # native build; on Linux you additionally need the
                      # webkit2gtk/libsoup dev packages Tauri lists at
                      # https://tauri.app/start/prerequisites/)
npm run build         # frontend only: tsc + vite build
```

The Rust backend never reads secrets from `.env` files at runtime — copy
`.env.local.example` to `.env.local` only if you want it for local
convenience (it's gitignored). Real secrets go through the app's own
Settings UI, which stores them in the Keychain via `save_api_key`.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
