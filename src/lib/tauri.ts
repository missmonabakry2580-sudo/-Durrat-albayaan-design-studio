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

export const sendAgentMessage = (message: string) =>
  invoke<string>("send_agent_message", { message });

export const clearAgentConversation = () => invoke<void>("clear_agent_conversation");

/**
 * Push-to-talk voice capture. NOT verified end to end yet — the native
 * transcriber helper these commands drive is written but has never been
 * compiled or run (no macOS/microphone in this development sandbox). See
 * docs/ARCHITECTURE.md "Voice pipeline". Until the helper is built and
 * bundled, `startVoiceCapture` is expected to reject with a clear
 * "not built yet" error rather than doing nothing silently.
 */
export const startVoiceCapture = () => invoke<void>("start_voice_capture");
export const stopVoiceCapture = () => invoke<void>("stop_voice_capture");

/** Speaks text aloud through the same on-device voice engine (macOS's
 * AVSpeechSynthesizer) — see docs/ARCHITECTURE.md "Voice pipeline". */
export const speakText = (text: string) => invoke<void>("speak_text", { text });
export const stopSpeaking = () => invoke<void>("stop_speaking");

export type TaskStatus = "open" | "in_progress" | "done" | "cancelled";

export interface Task {
  id: string;
  title: string;
  status: TaskStatus;
  source: string | null;
  created_at: string;
  updated_at: string;
  metadata: string | null;
}

export const createTask = (title: string) => invoke<Task>("create_task", { title });
export const quickCapture = (text: string) => invoke<Task>("quick_capture", { text });
export const listTasks = (status?: TaskStatus) => invoke<Task[]>("list_tasks", { status });
export const setTaskStatus = (id: string, status: TaskStatus) =>
  invoke<void>("set_task_status", { id, status });

export type EscalationStage = "friendly" | "firm" | "escalate_to_user";
export type FollowUpStatus = "pending" | "sent" | "resolved" | "cancelled";

export interface FollowUp {
  id: string;
  task_id: string;
  due_at: string;
  escalation_stage: EscalationStage;
  status: FollowUpStatus;
  created_at: string;
}

/**
 * Follow-up Engine. Escalating (see escalateFollowUp) sends a real native
 * OS notification (src-tauri/src/notify.rs) — that's the one delivery
 * channel wired up so far. No email yet; that needs Gmail's OAuth setup.
 */
export const createFollowUp = (taskId: string, dueAt: string) =>
  invoke<FollowUp>("create_follow_up", { taskId, dueAt });
export const listDueFollowUps = () => invoke<FollowUp[]>("list_due_follow_ups");
export const escalateFollowUp = (id: string) => invoke<FollowUp>("escalate_follow_up", { id });
export const setFollowUpStatus = (id: string, status: FollowUpStatus) =>
  invoke<void>("set_follow_up_status", { id, status });

export interface WorkspaceEntry {
  name: string;
  is_dir: boolean;
  size_bytes: number;
}

/**
 * Scoped to Mona's home folder (broadened from the original single
 * dedicated folder at her explicit request) — see src-tauri/src/files.rs.
 * Nothing here can escape that folder (path traversal / symlink escapes
 * are rejected), but within it every one of these — including a plain
 * list or read — is ConfirmHighRisk: the backend will not run it until
 * she's replied with an explicit confirmation word to Amin. See
 * src-tauri/src/tools.rs's `risk_for` and docs/SECURITY.md.
 */
export const listWorkspaceFiles = () => invoke<WorkspaceEntry[]>("list_workspace_files");
export const readWorkspaceFile = (path: string) =>
  invoke<string>("read_workspace_file", { path });
export const writeWorkspaceFile = (path: string, contents: string) =>
  invoke<void>("write_workspace_file", { path, contents });
export const deleteWorkspaceFile = (path: string) =>
  invoke<void>("delete_workspace_file", { path });

/**
 * Opens a URL in Amin's own isolated browser window (its own profile,
 * never Mona's personal browser) — see src-tauri/src/browser.rs. This is
 * intentionally just "show a page"; Amin does not read or act on its
 * content yet.
 */
export const openBrowserUrl = (url: string) => invoke<void>("open_browser_url", { url });

export interface DeltaBrief {
  open_tasks: number;
  tasks_created_last_24h: number;
  tasks_completed_last_24h: number;
  due_follow_ups: number;
  recent_audit_events: string[];
}

/**
 * Local-only "what changed" summary (Phase 3 slice that needs no Gmail/
 * Calendar). See src-tauri/src/brief.rs.
 */
export const generateDeltaBrief = () => invoke<DeltaBrief>("generate_delta_brief");
