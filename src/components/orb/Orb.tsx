import type { OrbState } from "./types";
import "./Orb.css";

interface OrbProps {
  state: OrbState;
  /** Accessible label; defaults to a generic "Amin" description of state. */
  label?: string;
}

/** Satellite gauges ringing the core — purely decorative dressing around
 * the one thing that actually carries meaning (the core's state). Angles
 * are spread unevenly and sizes vary so the ring doesn't read as a plain,
 * mechanical repeating pattern. */
const SATELLITES = [
  { angle: -100, radius: 168, size: 22, ring: true },
  { angle: -55, radius: 150, size: 14, ring: false },
  { angle: -18, radius: 172, size: 18, ring: true },
  { angle: 20, radius: 152, size: 11, ring: false },
  { angle: 58, radius: 174, size: 20, ring: true },
  { angle: 100, radius: 150, size: 13, ring: false },
  { angle: 140, radius: 170, size: 16, ring: true },
  { angle: 180, radius: 154, size: 12, ring: false },
  { angle: -140, radius: 172, size: 19, ring: true },
];

const CENTER = 200;

function polar(angleDeg: number, radius: number) {
  const rad = (angleDeg * Math.PI) / 180;
  return { x: CENTER + radius * Math.cos(rad), y: CENTER + radius * Math.sin(rad) };
}

const WAVEFORM_BARS = Array.from({ length: 28 }, (_, i) => i);

/**
 * The Living AI Core — Amin's single visual presence on screen. The core
 * itself must always reflect a real state (see OrbState), never spin just
 * to look busy; the surrounding satellite gauges and circuit traces are
 * decorative framing and carry no independent meaning of their own.
 */
export function Orb({ state, label }: OrbProps) {
  return (
    <div className="orb-hud" data-state={state} role="img" aria-label={label ?? `Amin is ${state}`}>
      <div className="orb-hud-stage">
        <svg
          className="orb-hud-svg"
          viewBox="0 0 400 400"
          aria-hidden="true"
          focusable="false"
        >
          {SATELLITES.map((sat, i) => {
            const { x, y } = polar(sat.angle, sat.radius);
            return (
              <line
                key={`line-${i}`}
                className="orb-hud-spoke"
                x1={CENTER}
                y1={CENTER}
                x2={x}
                y2={y}
                style={{ animationDelay: `${i * -0.35}s` }}
              />
            );
          })}
          {SATELLITES.map((sat, i) => {
            const { x, y } = polar(sat.angle, sat.radius);
            return (
              <g key={`sat-${i}`} className="orb-hud-satellite" style={{ animationDelay: `${i * 0.22}s` }}>
                <circle cx={x} cy={y} r={sat.size} className="orb-hud-satellite-ring" />
                {sat.ring && <circle cx={x} cy={y} r={sat.size * 0.45} className="orb-hud-satellite-dot" />}
              </g>
            );
          })}
        </svg>

        <div className="orb">
          <div className="orb-halo" />
          <div className="orb-ring" />
          <div className="orb-core" />
        </div>
      </div>

      <div className="orb-hud-waveform" aria-hidden="true">
        {WAVEFORM_BARS.map((i) => (
          <span key={i} className="orb-hud-bar" style={{ animationDelay: `${i * 0.045}s` }} />
        ))}
      </div>
    </div>
  );
}
