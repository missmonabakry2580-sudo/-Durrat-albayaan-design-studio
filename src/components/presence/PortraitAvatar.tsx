import { useEffect, useRef, useState } from "react";
import identityImage from "../../assets/amin-identity.jpg";
import {
  getSimliConnectionState,
  setSimliListeners,
  setSimliVideoElement,
  type SimliConnectionState,
} from "../../lib/simli/simliSession";

interface PortraitAvatarProps {
  className?: string;
}

/**
 * Portrait Mode's real renderer: Simli's live, lip-synced video when a
 * session is connected, falling back to the static official portrait
 * (src/assets/amin-identity.jpg) otherwise — never a blank box. The
 * `<video>` element stays mounted the whole time (Simli connects
 * asynchronously and needs a stable element to attach its stream to);
 * only its visibility toggles, and the static image is what actually
 * shows while there's no live session — which is the honest, correct
 * state whenever Simli isn't configured yet or a connection attempt
 * fails. See docs/ARCHITECTURE.md's "Visual modes" section.
 */
export function PortraitAvatar({ className }: PortraitAvatarProps) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const [connState, setConnState] = useState<SimliConnectionState>(getSimliConnectionState());

  useEffect(() => {
    setSimliListeners({ onStateChange: (s) => setConnState(s) });
    setSimliVideoElement(videoRef.current);
    // Deliberately does NOT call ensureConnected() here — connecting on
    // every mount would also fire during the splash screen (which renders
    // this same component's default "portrait" mode) and every time
    // Portrait Mode is shown, even before Amin has said a word, burning
    // Simli's free-tier minutes for no reason. speakViaSimli() in
    // App.tsx's speak() connects lazily on the first real utterance
    // instead — see simliSession.ts.
    return () => {
      setSimliVideoElement(null);
      setSimliListeners({});
    };
  }, []);

  const showVideo = connState === "connected";

  return (
    <>
      <video
        ref={videoRef}
        className={className}
        autoPlay
        playsInline
        style={{ display: showVideo ? "block" : "none" }}
      />
      {!showVideo && <img className={className} src={identityImage} alt="" aria-hidden="true" />}
      {/* Real gap this closed: connecting/error states looked identical to
          the plain idle static portrait — Mona would have no way to tell
          "Simli is trying to connect" from "nothing is happening" while
          waiting the few seconds a real WebRTC handshake takes. Small,
          non-blocking (pointer-events: none — never in the way of clicking
          the mode toggle or anything else underneath). */}
      {connState === "connecting" && (
        <span className="portrait-connecting-badge" aria-live="polite">
          جاري الاتصال بصورة أمين الحية...
        </span>
      )}
    </>
  );
}
