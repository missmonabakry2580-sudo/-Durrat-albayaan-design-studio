/**
 * The Living AI Core's states. Every long-running Amin operation should map
 * onto one of these so the orb is always an honest reflection of what Amin
 * is actually doing — never decorative animation with no real state behind
 * it.
 */
export type OrbState =
  | "idle"
  | "listening"
  | "thinking"
  | "planning"
  | "executing"
  | "speaking"
  | "success"
  | "warning"
  | "waiting";

export const ORB_STATE_LABELS: Record<OrbState, string> = {
  idle: "خامل",
  listening: "بيسمع",
  thinking: "بيفكر",
  planning: "بيخطط",
  executing: "بينفذ",
  speaking: "بيتكلم",
  success: "تم بنجاح",
  warning: "تنبيه",
  waiting: "بينتظر",
};
