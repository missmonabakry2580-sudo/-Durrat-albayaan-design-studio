import { invoke } from "@tauri-apps/api/core";

/**
 * Typed wrappers around the Rust commands in src-tauri/src/commands.rs.
 * Nothing in the UI should call `invoke()` directly — routing every call
 * through here keeps the frontend/backend contract in one place and makes
 * it obvious, at a glance, what surface the webview actually has.
 */

export type AutonomyLevel = "observe" | "assist" | "delegate" | "autopilot";
export type RiskTier = "auto" | "trusted_delegation" | "confirm_high_risk" | "excluded";

export interface AppInfo {
  name: string;
  version: string;
}

export interface AuditEntry {
  id: string;
  ts: string;
  actor: string;
  action: string;
  risk_tier: RiskTier;
  decision: "executed" | "confirmed" | "declined" | "blocked";
  details: string | null;
  evidence: string | null;
}

export const appInfo = () => invoke<AppInfo>("app_info");

export const hasApiKey = () => invoke<boolean>("has_api_key");
export const saveApiKey = (key: string) => invoke<void>("save_api_key", { key });
export const clearApiKey = () => invoke<void>("clear_api_key");

export const getAutonomyLevel = () => invoke<AutonomyLevel>("get_autonomy_level");
export const setAutonomyLevel = (level: AutonomyLevel) =>
  invoke<void>("set_autonomy_level", { level });

export const isHalted = () => invoke<boolean>("is_halted");
export const setKillSwitch = (active: boolean) => invoke<void>("set_kill_switch", { active });

export const classifyAction = (domain: string) => invoke<RiskTier>("classify_action", { domain });

export const listAuditLog = (limit = 20) => invoke<AuditEntry[]>("list_audit_log", { limit });
