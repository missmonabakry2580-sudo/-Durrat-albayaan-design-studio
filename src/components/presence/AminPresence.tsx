import { useState } from "react";
import { PortraitAvatar } from "./PortraitAvatar";
import { ThreeDAvatar } from "./ThreeDAvatar";
import { AMIN_STATE_LABELS, type AminState } from "./types";
import type { VisualMode } from "../../lib/visual/visualMode";
import "./AminPresence.css";

interface AminPresenceProps {
  state: AminState;
  className?: string;
  /** The tone Claude tagged its own reply with (see agent::extract_emotion).
   * Carried as data-emotion for a future hologram/avatar face to react to;
   * no visual behavior hooks into it yet. */
  emotion?: string | null;
  /** "3d" loads the real facial-rig GLB (public/models/amin_facial_rig.glb);
   * "portrait" shows Mona's approved reference artwork as before. Falls
   * back to portrait automatically — see onModelFailure below — so a
   * WebGL/model-load problem never blanks out Amin's presence entirely.
   * Defaults to "portrait" — the splash screen renders this with neither
   * prop set, since a brief brand moment is never the place to attempt a
   * 3D load. */
  visualMode?: VisualMode;
  /** Called once if the 3D renderer can't come up (no WebGL, model missing
   * or malformed) — the parent switches visualMode back to "portrait" and
   * tells Mona why, since silently reverting with no explanation would look
   * like the toggle just didn't work. */
  onModelFailure?: (reason: string) => void;
}

const WAVE_BARS = Array.from({ length: 14 }, (_, i) => i);

/**
 * Amin's own visual identity. Two renderers share this same slot — the 3D
 * facial rig or Mona's approved reference artwork — chosen by visualMode;
 * everything else here (the glow, the voice waveform, the state driving
 * both) is identical either way, since switching modes must never look
 * like switching to a different assistant. See docs/ARCHITECTURE.md's
 * "Visual modes" section.
 */
export function AminPresence({
  state,
  className,
  emotion,
  visualMode = "portrait",
  onModelFailure,
}: AminPresenceProps) {
  // Once a 3D load failure is reported for this mount, keep rendering the
  // portrait locally even if the parent is slow to flip visualMode — avoids
  // a one-frame flash back to a canvas that just proved it can't render.
  const [localFailure, setLocalFailure] = useState<string | null>(null);
  const showThreeD = visualMode === "3d" && !localFailure;

  function handleFailure(reason: string) {
    setLocalFailure(reason);
    onModelFailure?.(reason);
  }

  return (
    <div
      className={["amin-presence", className].filter(Boolean).join(" ")}
      data-state={state}
      data-emotion={emotion ?? undefined}
      role="img"
      aria-label={`أمين ${AMIN_STATE_LABELS[state]}`}
    >
      <div className="amin-presence-glow" aria-hidden="true" />
      {showThreeD ? (
        <ThreeDAvatar
          className="amin-presence-portrait amin-presence-portrait-3d"
          state={state}
          emotion={emotion}
          onFailure={handleFailure}
        />
      ) : (
        <PortraitAvatar className="amin-presence-portrait" />
      )}
      <div className="amin-presence-wave" aria-hidden="true">
        {WAVE_BARS.map((i) => (
          <span key={i} className="amin-presence-wave-bar" style={{ animationDelay: `${i * 0.05}s` }} />
        ))}
      </div>
    </div>
  );
}
