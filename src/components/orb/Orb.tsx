import type { OrbState } from "./types";
import "./Orb.css";

interface OrbProps {
  state: OrbState;
  /** Accessible label; defaults to a generic "Amin" description of state. */
  label?: string;
}

/**
 * The Living AI Core — Amin's single visual presence on screen. It must
 * always reflect a real state (see OrbState), never spin just to look busy.
 */
export function Orb({ state, label }: OrbProps) {
  return (
    <div
      className="orb"
      data-state={state}
      role="img"
      aria-label={label ?? `Amin is ${state}`}
    >
      <div className="orb-halo" />
      <div className="orb-ring" />
      <div className="orb-core" />
    </div>
  );
}
