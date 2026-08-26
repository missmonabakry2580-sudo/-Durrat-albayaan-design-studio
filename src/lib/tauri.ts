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

/** A reply from Amin's Agent Core. `emotion` — the tone Claude tagged its
 * own reply with — is groundwork for a future hologram/avatar face; not
 * shown anywhere yet. */
export interface AgentReply {
  text: string;
  emotion: string | null;
}

export const appInfo = () => invoke<AppInfo>("app_info");

export const hasApiKey = () => invoke<boolean>("has_api_key");
export const saveApiKey = (key: string) => invoke<void>("save_api_key", { key });
export const clearApiKey = () => invoke<void>("clear_api_key");

/** Optional — a more expressive, human-sounding voice (ElevenLabs) Mona
 * can add on top of the free, local, on-device voice. Costs money per use
 * and sends reply text to ElevenLabs' API, unlike the on-device default. */
export const hasElevenLabsKey = () => invoke<boolean>("has_elevenlabs_key");
export const saveElevenLabsKey = (key: string) => invoke<void>("save_elevenlabs_key", { key });
export const clearElevenLabsKey = () => invoke<void>("clear_elevenlabs_key");

/** Which ElevenLabs voice speaks Amin's replies — empty means the default
 * (Rachel, English), which mangles Arabic. Pick a voice from Mona's own
 * ElevenLabs library (elevenlabs.io → Voices) and paste its Voice ID here. */
export const getElevenLabsVoiceId = () => invoke<string>("get_elevenlabs_voice_id");
export const saveElevenLabsVoiceId = (voiceId: string) =>
  invoke<void>("save_elevenlabs_voice_id", { voiceId });

export const getAutonomyLevel = () => invoke<AutonomyLevel>("get_autonomy_level");
export const setAutonomyLevel = (level: AutonomyLevel) =>
  invoke<void>("set_autonomy_level", { level });

export const isHalted = () => invoke<boolean>("is_halted");
export const setKillSwitch = (active: boolean) => invoke<void>("set_kill_switch", { active });

export const classifyAction = (domain: string) => invoke<RiskTier>("classify_action", { domain });

export const listAuditLog = (limit = 20) => invoke<AuditEntry[]>("list_audit_log", { limit });

export const sendAgentMessage = (message: string) =>
  invoke<AgentReply>("send_agent_message", { message });

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

/** Speaks text aloud — the on-device engine (macOS's AVSpeechSynthesizer)
 * by default, or ElevenLabs when Mona has added her own key (see
 * hasElevenLabsKey). `emotion` is Claude's own `[[emotion:VALUE]]` tag for
 * this reply (see AgentReply) — ElevenLabs uses it to shape delivery
 * (see src-tauri/src/elevenlabs.rs's voice_settings_for_emotion); the
 * on-device engine ignores it. See docs/ARCHITECTURE.md "Voice pipeline". */
export const speakText = (text: string, emotion?: string | null) =>
  invoke<void>("speak_text", { text, emotion: emotion ?? null });
export const stopSpeaking = () => invoke<void>("stop_speaking");

export interface HandsFreeSettings {
  enabled: boolean;
  wake_phrase: string;
  close_phrase: string;
}

/**
 * Hands-free mode: say the wake phrase to open a session, the close phrase
 * (or just go quiet) to end it — see macos/transcriber/AminVoice.swift's
 * `HandsFreeListener` and docs/SECURITY.md. Off by default: while it's on,
 * the microphone stays open continuously (macOS's own mic indicator shows
 * the whole time), and the wake-phrase-watching phase runs on-device only.
 * Not yet verified end to end on a real Mac.
 */
export const getHandsFreeSettings = () => invoke<HandsFreeSettings>("get_hands_free_settings");
export const saveHandsFreePhrases = (wakePhrase: string, closePhrase: string) =>
  invoke<void>("save_hands_free_phrases", { wakePhrase, closePhrase });
export const setHandsFreeMode = (enabled: boolean) =>
  invoke<void>("set_hands_free_mode", { enabled });

export type TaskStatus = "open" | "in_progress" | "done" | "cancelled";

export interface Task {
  id: string;
  title: string;
  status: TaskStatus;
  source: string | null;
  created_at: string;
  updated_at: string;
  metadata: string | null;
  /** Mona's explicit task shape — see src-tauri/src/tasks.rs's
   * NewTaskDetails. Claude fills these in from context, not required.
   * No dedicated UI shows them yet; they exist so Claude and the Morning
   * Brief can reason over priority/deadline/dependencies, not just to
   * round-trip through the type. */
  priority: string | null;
  deadline: string | null;
  project: string | null;
  next_action: string | null;
  approval_required: boolean;
  dependencies: string[];
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

export interface PendingActionSummary {
  tool_name: string;
  description: string;
  proposed_at: string;
  expired: boolean;
}

/**
 * Mona's Permission Model, LEVEL 2: whatever Amin is currently waiting on
 * her explicit word for, if anything — a read-only look at the same
 * ConfirmHighRisk proposal `sendAgentMessage`'s reply already describes in
 * chat, kept visible even if she's scrolled away from that message. See
 * src-tauri/src/confirmation.rs's PendingAction and its 10-minute expiry.
 */
export const getPendingAction = () => invoke<PendingActionSummary | null>("get_pending_action");
