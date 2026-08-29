// Low-level Simli (simli.ai) WebRTC client — implements their documented
// plain peer-to-peer protocol directly (docs.simli.com/api-reference/
// websockets/peer_to_peer, .../simli-webrtc) rather than pulling in their
// `simli-client` npm package, whose docs only show the LiveKit-backed
// path with an unpinned version. This is the one exception in this app
// to "the frontend never reaches the network directly" (see
// tauri.conf.json's CSP comment and docs/ARCHITECTURE.md's "Visual
// modes" section) — WebRTC is a browser API with no Rust equivalent
// here, and Simli's protocol carries both signaling and raw audio frames
// over the same WebSocket, so there is no way to keep this in Rust and
// only hand the frontend a finished video element.
//
// Confirmed against Simli's own docs, not guessed: the WebSocket is
// wss://api.simli.ai/compose/webrtc/peer_to_peer?session_token=...&enableSFU=...;
// the client creates recvonly audio+video transceivers, sends an SDP
// offer as {"type":"offer","sdp":...} within 30s of opening the socket,
// and receives {"type":"answer","sdp":...} back; media then arrives via
// RTCPeerConnection's `track` event. Audio goes out as raw binary
// WebSocket frames — PCM16, 16kHz, mono, ideally ~6000 bytes/chunk (no
// container, no JSON wrapper) — and "SKIP"/"DONE" text frames control
// segment boundaries. Non-JSON text frames Simli sends are prefixed
// control/error strings ("ERROR:", "RATE:", "CLOSING:"), not structured
// JSON — surfaced via onError here.

const ICE_GATHER_TIMEOUT_MS = 3000;
const WS_OPEN_TIMEOUT_MS = 10000;
export const SIMLI_AUDIO_CHUNK_BYTES = 6000;

export interface SimliClientCallbacks {
  /** Fired once the RTCPeerConnection reaches "connected". */
  onConnected?: () => void;
  /** Fired on a clean or unexpected disconnect, with the raw
   * RTCPeerConnectionState or WebSocket close reason for diagnostics. */
  onDisconnected?: (reason: string) => void;
  /** Fired on a Simli-reported error string or a local WebRTC failure —
   * never thrown silently, so the caller can fall back to the static
   * portrait instead of showing a frozen video element. */
  onError?: (message: string) => void;
}

function waitForIceGatheringComplete(pc: RTCPeerConnection): Promise<void> {
  if (pc.iceGatheringState === "complete") return Promise.resolve();
  return new Promise((resolve) => {
    function check() {
      if (pc.iceGatheringState === "complete") {
        pc.removeEventListener("icegatheringstatechange", check);
        resolve();
      }
    }
    pc.addEventListener("icegatheringstatechange", check);
    // Simli's own 30s deadline is generous; this shorter local timeout
    // just means "send what we have" rather than blocking on a slow or
    // stalled ICE gather — a real, if imperfect, offer is better than
    // waiting indefinitely for a "complete" event that some network
    // conditions never fire.
    setTimeout(() => {
      pc.removeEventListener("icegatheringstatechange", check);
      resolve();
    }, ICE_GATHER_TIMEOUT_MS);
  });
}

/** Splits raw PCM bytes into Simli's recommended chunk size. A pure
 * function — independently testable without any real connection. */
export function pcmToChunks(pcm: Uint8Array, chunkSize = SIMLI_AUDIO_CHUNK_BYTES): Uint8Array[] {
  const chunks: Uint8Array[] = [];
  for (let i = 0; i < pcm.length; i += chunkSize) {
    chunks.push(pcm.slice(i, i + chunkSize));
  }
  return chunks;
}

export class SimliClient {
  private ws: WebSocket | null = null;
  private pc: RTCPeerConnection | null = null;
  private connected = false;

  constructor(
    private sessionToken: string,
    private videoEl: HTMLVideoElement,
    private callbacks: SimliClientCallbacks = {},
  ) {}

  isConnected(): boolean {
    return this.connected;
  }

  async connect(): Promise<void> {
    const url = `wss://api.simli.ai/compose/webrtc/peer_to_peer?session_token=${encodeURIComponent(this.sessionToken)}&enableSFU=false`;
    const ws = new WebSocket(url);
    ws.binaryType = "arraybuffer";
    this.ws = ws;

    const pc = new RTCPeerConnection({ iceServers: [{ urls: "stun:stun.l.google.com:19302" }] });
    this.pc = pc;
    pc.addTransceiver("audio", { direction: "recvonly" });
    pc.addTransceiver("video", { direction: "recvonly" });

    const remoteStream = new MediaStream();
    pc.ontrack = (event) => {
      remoteStream.addTrack(event.track);
      this.videoEl.srcObject = remoteStream;
    };
    pc.onconnectionstatechange = () => {
      const state = pc.connectionState;
      if (state === "connected") {
        this.connected = true;
        this.callbacks.onConnected?.();
      } else if (state === "failed" || state === "disconnected" || state === "closed") {
        this.connected = false;
        this.callbacks.onDisconnected?.(state);
      }
    };

    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("Simli WebSocket لم يفتح في الوقت المتوقع")), WS_OPEN_TIMEOUT_MS);
      ws.onopen = () => {
        clearTimeout(timer);
        resolve();
      };
      ws.onerror = () => {
        clearTimeout(timer);
        reject(new Error("خطأ في اتصال Simli WebSocket"));
      };
    });

    ws.onmessage = (event) => {
      if (typeof event.data !== "string") return; // binary frames are outbound-only in this protocol
      let msg: { type?: string; sdp?: string } | null = null;
      try {
        msg = JSON.parse(event.data);
      } catch {
        if (event.data.startsWith("ERROR:") || event.data.startsWith("CLOSING:") || event.data.startsWith("RATE:")) {
          this.callbacks.onError?.(event.data);
        }
        return;
      }
      if (msg?.type === "answer" && msg.sdp) {
        pc.setRemoteDescription({ type: "answer", sdp: msg.sdp }).catch((e) =>
          this.callbacks.onError?.(`تعذّر ضبط رد Simli: ${String(e)}`),
        );
      }
    };
    ws.onclose = () => {
      this.connected = false;
      this.callbacks.onDisconnected?.("websocket-closed");
    };

    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    await waitForIceGatheringComplete(pc);
    if (!pc.localDescription) throw new Error("تعذّر إنشاء عرض WebRTC لـ Simli");
    ws.send(JSON.stringify({ type: "offer", sdp: pc.localDescription.sdp }));
  }

  /** Streams one utterance's PCM audio as binary WS frames, paced close
   * to real-time (rather than dumping every chunk at once) since this is
   * a live streaming protocol, not a file upload — matches how the audio
   * would actually be produced if this were true incremental TTS. */
  async sendAudio(pcm: Uint8Array): Promise<void> {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      throw new Error("جلسة Simli مش متصلة");
    }
    const chunks = pcmToChunks(pcm);
    // Bytes-per-second for 16kHz/16-bit/mono PCM = 16000 * 2 = 32000.
    const msPerChunk = (SIMLI_AUDIO_CHUNK_BYTES / 32000) * 1000;
    for (const chunk of chunks) {
      if (this.ws.readyState !== WebSocket.OPEN) throw new Error("انقطع اتصال Simli أثناء الإرسال");
      this.ws.send(chunk);
      await new Promise((r) => setTimeout(r, msPerChunk));
    }
    this.ws.send("DONE");
  }

  /** Stops whatever Simli is currently saying/rendering — the Portrait
   * equivalent of stop_speaking's afplay kill. */
  skip(): void {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send("SKIP");
  }

  close(): void {
    this.connected = false;
    try {
      this.ws?.close();
    } catch {
      // already closed — fine
    }
    try {
      this.pc?.close();
    } catch {
      // already closed — fine
    }
    this.ws = null;
    this.pc = null;
  }
}
