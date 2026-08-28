// A module-level singleton, deliberately not tied to any component's
// mount lifecycle — the whole point is that switching visualMode between
// "3d" and "portrait" must not tear down or reconnect the Simli session
// (see docs/ARCHITECTURE.md "Visual modes": session continuity while
// switching). It connects lazily on the first speak() call made while
// Portrait Mode is active, and stays open across mode toggles until it
// errors, disconnects, or the app closes.

import { startSimliSession, synthesizePcmForSimli, hasSimliKey } from "../tauri";
import { SimliClient } from "./simliClient";

export type SimliConnectionState = "idle" | "connecting" | "connected" | "error";

interface SimliSessionListeners {
  onStateChange?: (state: SimliConnectionState, detail?: string) => void;
}

let client: SimliClient | null = null;
let state: SimliConnectionState = "idle";
let videoElRef: HTMLVideoElement | null = null;
let listeners: SimliSessionListeners = {};

function setState(next: SimliConnectionState, detail?: string) {
  state = next;
  listeners.onStateChange?.(next, detail);
}

export function getSimliConnectionState(): SimliConnectionState {
  return state;
}

export function setSimliListeners(next: SimliSessionListeners): void {
  listeners = next;
}

/** Registers the <video> element Simli's incoming stream should render
 * into. Must be called before ensureConnected() — PortraitAvatar.tsx
 * calls this from its own mount effect. */
export function setSimliVideoElement(el: HTMLVideoElement | null): void {
  videoElRef = el;
}

/** Connects once if not already connected/connecting. Real errors
 * (missing key, Simli rejecting the session, WebRTC failure) surface as
 * a rejected promise — callers fall back to the static portrait rather
 * than showing a frozen or blank video element. Never fabricates a
 * "connected" state that didn't actually happen. */
export async function ensureConnected(): Promise<boolean> {
  if (state === "connected" && client?.isConnected()) return true;
  if (state === "connecting") return false; // a connect is already in flight
  if (!videoElRef) {
    setState("error", "لسه مفيش عنصر فيديو جاهز لعرض Simli");
    return false;
  }

  const hasKey = await hasSimliKey().catch(() => false);
  if (!hasKey) {
    setState("idle"); // not an error — Simli simply isn't configured yet
    return false;
  }

  setState("connecting");
  try {
    const token = await startSimliSession();
    const newClient = new SimliClient(token, videoElRef, {
      onConnected: () => setState("connected"),
      onDisconnected: (reason) => setState("error", reason),
      onError: (message) => setState("error", message),
    });
    await newClient.connect();
    client = newClient;
    return true;
  } catch (e) {
    setState("error", String(e));
    client = null;
    return false;
  }
}

/** Speaks `text` through Simli — synthesizes the same-voice PCM audio via
 * Rust, then streams it into the live session. Throws on any real
 * failure (no key, session dead, network error) rather than silently
 * doing nothing; callers must catch this and fall back to speakText's
 * local playback so Amin Core keeps talking either way. */
export async function speakViaSimli(text: string, emotion?: string | null): Promise<void> {
  const connected = await ensureConnected();
  if (!connected || !client) {
    throw new Error(`جلسة Simli مش متصلة (${state})`);
  }
  const pcmBytes = await synthesizePcmForSimli(text, emotion);
  await client.sendAudio(new Uint8Array(pcmBytes));
}

export function stopSimliSpeaking(): void {
  client?.skip();
}

export function closeSimliSession(): void {
  client?.close();
  client = null;
  setState("idle");
}
