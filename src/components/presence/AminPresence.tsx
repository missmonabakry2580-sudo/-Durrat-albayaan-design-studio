import identityImage from "../../assets/amin-identity.jpg";
import { AMIN_STATE_LABELS, type AminState } from "./types";
import "./AminPresence.css";

interface AminPresenceProps {
  state: AminState;
  className?: string;
}

const WAVE_BARS = Array.from({ length: 14 }, (_, i) => i);

/**
 * Amin's own visual identity — Mona's approved reference artwork itself,
 * used exactly as designed (not redrawn or reinterpreted). State lives on
 * the root element (data-state): the glow behind the portrait and the
 * voice waveform react to it, so the character's own light and motion are
 * the only indicator of what Amin is doing — no separate status row, no
 * buttons. The portrait's edges fade into the surrounding page instead of
 * sitting inside a hard-edged card, so it reads as part of the app rather
 * than a picture pinned on top of it.
 */
export function AminPresence({ state, className }: AminPresenceProps) {
  return (
    <div
      className={["amin-presence", className].filter(Boolean).join(" ")}
      data-state={state}
      role="img"
      aria-label={`أمين ${AMIN_STATE_LABELS[state]}`}
    >
      <div className="amin-presence-glow" aria-hidden="true" />
      <img className="amin-presence-portrait" src={identityImage} alt="" aria-hidden="true" />
      <div className="amin-presence-wave" aria-hidden="true">
        {WAVE_BARS.map((i) => (
          <span key={i} className="amin-presence-wave-bar" style={{ animationDelay: `${i * 0.05}s` }} />
        ))}
      </div>
    </div>
  );
}
