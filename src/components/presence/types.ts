/**
 * Amin's own states. Every long-running operation should map onto one of
 * these so the presence is always an honest reflection of what Amin is
 * actually doing — never decorative animation with no real state behind it.
 * There is deliberately no button or switcher for these anywhere in the
 * app; only Amin's own light and motion express them.
 */
export type AminState =
  | "idle"
  | "armed"
  | "listening"
  | "thinking"
  | "planning"
  | "executing"
  | "speaking"
  | "success"
  | "warning"
  | "waiting";

export const AMIN_STATE_LABELS: Record<AminState, string> = {
  idle: "خامل",
  armed: "بيراقب صوتك",
  listening: "بيسمع",
  thinking: "بيفكر",
  planning: "بيخطط",
  executing: "بينفذ",
  speaking: "بيتكلم",
  success: "تم بنجاح",
  warning: "تنبيه",
  waiting: "بينتظر",
};
