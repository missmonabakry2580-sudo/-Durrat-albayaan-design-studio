import { useEffect, useState } from "react";
import { Orb } from "../orb/Orb";
import { CREATOR_ATTRIBUTION_AR } from "../../lib/branding";
import "./Splash.css";

interface SplashProps {
  onDone: () => void;
  /** Milliseconds before auto-dismiss; also dismissible by a click/tap. */
  duration?: number;
}

/**
 * The launch screen — Amin's identity and, per the brief, a clear,
 * respectful creator attribution shown up front rather than buried in a
 * settings menu.
 */
export function Splash({ onDone, duration = 1800 }: SplashProps) {
  const [leaving, setLeaving] = useState(false);

  useEffect(() => {
    const timer = setTimeout(() => setLeaving(true), duration);
    return () => clearTimeout(timer);
  }, [duration]);

  return (
    <div
      className={leaving ? "splash splash-leaving" : "splash"}
      onClick={() => setLeaving(true)}
      onTransitionEnd={() => leaving && onDone()}
      role="button"
      tabIndex={0}
      aria-label="أمين — اضغطي للمتابعة"
    >
      <Orb state="idle" />
      <h1 className="splash-title">أمين</h1>
      <p className="splash-attribution">{CREATOR_ATTRIBUTION_AR}</p>
    </div>
  );
}
