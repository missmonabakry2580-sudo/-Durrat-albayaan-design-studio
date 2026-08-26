import { AMIN_STATE_LABELS, type AminState } from "./types";
import "./AminPresence.css";

interface AminPresenceProps {
  state: AminState;
  className?: string;
}

interface Node {
  id: string;
  x: number;
  y: number;
  r: number;
  gold?: boolean;
}

/** The network inside Amin's silhouette — a hand-placed neural web, not a
 * random scatter, so it reads as a face/mind rather than noise. */
const CORE = { x: 150, y: 132 };

const NODES: Node[] = [
  { id: "n1", x: 150, y: 88, r: 3.6 },
  { id: "n2", x: 114, y: 108, r: 3 },
  { id: "n3", x: 186, y: 108, r: 3 },
  { id: "n4", x: 128, y: 152, r: 4.4, gold: true },
  { id: "n5", x: 172, y: 152, r: 4.4, gold: true },
  { id: "n7", x: 98, y: 158, r: 2.8 },
  { id: "n8", x: 202, y: 158, r: 2.8 },
  { id: "n9", x: 120, y: 192, r: 3.4 },
  { id: "n10", x: 180, y: 192, r: 3.4 },
  { id: "n11", x: 150, y: 208, r: 3, gold: true },
  { id: "n12", x: 88, y: 118, r: 2.6 },
  { id: "n13", x: 212, y: 118, r: 2.6 },
];

const CORE_LINKS = ["n1", "n2", "n3", "n4", "n5", "n7", "n8", "n9", "n10", "n11"];
const PEER_LINKS: [string, string][] = [
  ["n1", "n2"],
  ["n1", "n3"],
  ["n4", "n9"],
  ["n5", "n10"],
  ["n2", "n12"],
  ["n3", "n13"],
];

/** Escape rays — the design brief's "lines extending calmly into the rest
 * of the interface": a few strands that leave the silhouette and fade into
 * open space rather than stopping at a hard edge. */
const RAYS: { from: string; to: [number, number] }[] = [
  { from: "n12", to: [-70, 96] },
  { from: "n13", to: [370, 96] },
  { from: "n11", to: [150, 420] },
];

function nodeById(id: string) {
  return NODES.find((n) => n.id === id)!;
}

const WAVE_BARS = Array.from({ length: 14 }, (_, i) => i);

/**
 * Amin's own visual identity — a faceted head-and-shoulders silhouette
 * built entirely from a neural network of connection points, in place of
 * any generic orb or avatar. State lives on the root element (data-state);
 * every layer below reacts to it, so the character itself is the only
 * indicator of what Amin is doing — no separate status row, no buttons.
 */
export function AminPresence({ state, className }: AminPresenceProps) {
  return (
    <div
      className={["amin-presence", className].filter(Boolean).join(" ")}
      data-state={state}
      role="img"
      aria-label={`أمين ${AMIN_STATE_LABELS[state]}`}
    >
      <svg className="amin-presence-svg" viewBox="0 0 300 340" aria-hidden="true" focusable="false">
        <defs>
          {RAYS.map((ray, i) => {
            const from = nodeById(ray.from);
            return (
              <linearGradient
                key={`ray-fade-${i}`}
                id={`ray-fade-${i}`}
                gradientUnits="userSpaceOnUse"
                x1={from.x}
                y1={from.y}
                x2={ray.to[0]}
                y2={ray.to[1]}
              >
                <stop offset="0%" className="amin-presence-ray-stop-start" />
                <stop offset="100%" className="amin-presence-ray-stop-end" />
              </linearGradient>
            );
          })}
        </defs>

        <polygon
          className="amin-presence-shoulders"
          points="112,228 188,228 258,338 42,338"
        />
        <polygon
          className="amin-presence-head"
          points="150,40 210,70 226,140 196,206 150,226 104,206 74,140 90,70"
        />

        {RAYS.map((ray, i) => {
          const from = nodeById(ray.from);
          return (
            <line
              key={`ray-${i}`}
              className="amin-presence-ray"
              x1={from.x}
              y1={from.y}
              x2={ray.to[0]}
              y2={ray.to[1]}
              stroke={`url(#ray-fade-${i})`}
              style={{ animationDelay: `${i * -0.6}s` }}
            />
          );
        })}

        {CORE_LINKS.map((id) => {
          const n = nodeById(id);
          return (
            <line
              key={`core-link-${id}`}
              className="amin-presence-link"
              x1={CORE.x}
              y1={CORE.y}
              x2={n.x}
              y2={n.y}
            />
          );
        })}
        {PEER_LINKS.map(([a, b], i) => {
          const na = nodeById(a);
          const nb = nodeById(b);
          return (
            <line
              key={`peer-link-${i}`}
              className="amin-presence-link amin-presence-link-peer"
              x1={na.x}
              y1={na.y}
              x2={nb.x}
              y2={nb.y}
            />
          );
        })}

        {NODES.map((n, i) => (
          <circle
            key={n.id}
            className={n.gold ? "amin-presence-node amin-presence-node-gold" : "amin-presence-node"}
            cx={n.x}
            cy={n.y}
            r={n.r}
            style={{ animationDelay: `${i * 0.18}s` }}
          />
        ))}

        <circle className="amin-presence-core-halo" cx={CORE.x} cy={CORE.y} r={26} />
        <circle className="amin-presence-core" cx={CORE.x} cy={CORE.y} r={13} />
      </svg>

      <div className="amin-presence-wave" aria-hidden="true">
        {WAVE_BARS.map((i) => (
          <span key={i} className="amin-presence-wave-bar" style={{ animationDelay: `${i * 0.05}s` }} />
        ))}
      </div>
    </div>
  );
}
