import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { Update } from "@tauri-apps/plugin-updater";
import { AminPresence } from "./components/presence/AminPresence";
import type { AminState } from "./components/presence/types";
import { Splash } from "./components/splash/Splash";
import { CREATOR_ATTRIBUTION_AR, CREATOR_ATTRIBUTION_EN } from "./lib/branding";
import { checkForUpdate, installUpdateAndRestart } from "./lib/updater";
import {
  type AppInfo,
  type AuditEntry,
  type AutonomyLevel,
  type DeltaBrief,
  type FollowUp,
  type PendingActionSummary,
  type Task,
  type WorkspaceEntry,
  appInfo,
  clearAgentConversation,
  clearApiKey,
  clearElevenLabsKey,
  createFollowUp,
  createTask,
  deleteWorkspaceFile,
  escalateFollowUp,
  generateDeltaBrief,
  getAutonomyLevel,
  getElevenLabsVoiceId,
  getHandsFreeSettings,
  getPendingAction,
  hasApiKey,
  hasElevenLabsKey,
  isHalted,
  listAuditLog,
  listDueFollowUps,
  listTasks,
  listWorkspaceFiles,
  openBrowserUrl,
  quickCapture,
  readWorkspaceFile,
  saveApiKey,
  saveElevenLabsKey,
  saveElevenLabsVoiceId,
  saveHandsFreePhrases,
  sendAgentMessage,
  setAutonomyLevel,
  setFollowUpStatus,
  setHandsFreeMode,
  setKillSwitch,
  setTaskStatus,
  speakText,
  startVoiceCapture,
  stopSpeaking,
  stopVoiceCapture,
  writeWorkspaceFile,
} from "./lib/tauri";
import "./App.css";

const AUTONOMY_LEVELS: AutonomyLevel[] = ["observe", "assist", "delegate", "autopilot"];

// Arabic display labels for values that travel to/from the Rust backend as
// fixed English identifiers (risk_tier, decision, autonomy level, task
// source, escalation stage) — the wire values never change, only what's
// shown on screen.
const AUTONOMY_LABELS: Record<AutonomyLevel, string> = {
  observe: "مراقبة فقط",
  assist: "مساعدة",
  delegate: "تفويض",
  autopilot: "تلقائي بالكامل",
};

const RISK_TIER_LABELS: Record<string, string> = {
  auto: "تلقائي",
  trusted_delegation: "تفويض موثوق",
  confirm_high_risk: "يحتاج تأكيدك",
  excluded: "ممنوع",
};

const DECISION_LABELS: Record<string, string> = {
  executed: "تم التنفيذ",
  confirmed: "تم التأكيد",
  declined: "تم الرفض",
  blocked: "محظور",
  proposed: "مقترح — بانتظار موافقتك",
};

const TASK_SOURCE_LABELS: Record<string, string> = {
  manual: "يدوي",
  quick_capture: "تدوين سريع",
  amin: "من أمين",
  amin_quick_capture: "تدوين سريع من أمين",
};

const ESCALATION_STAGE_LABELS: Record<string, string> = {
  friendly: "ودّي",
  firm: "حازم",
  escalate_to_user: "محتاجة انتباهك",
};

function arabicLabel(map: Record<string, string>, value: string): string {
  return map[value] ?? value;
}

interface AgentTurn {
  role: "user" | "amin";
  text: string;
}

type PanelKey = "brief" | "tasks" | "followups" | "files" | "browser" | "audit" | "settings";

const NAV_ITEMS: { key: PanelKey; icon: string; label: string }[] = [
  { key: "brief", icon: "◈", label: "ملخص التغييرات" },
  { key: "tasks", icon: "✓", label: "المهام" },
  { key: "followups", icon: "⏰", label: "المتابعات" },
  { key: "files", icon: "📁", label: "الملفات" },
  { key: "browser", icon: "🌐", label: "المتصفح" },
  { key: "audit", icon: "🕘", label: "سجل المراجعة" },
  { key: "settings", icon: "⚙", label: "الإعدادات والأمان" },
];

/** True once we're actually running inside the Tauri shell, not a plain browser tab. */
const inTauri = "__TAURI_INTERNALS__" in window;

function App() {
  const [showSplash, setShowSplash] = useState(true);
  const [aminState, setAminState] = useState<AminState>("idle");
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [keySaved, setKeySaved] = useState(false);
  const [keyInput, setKeyInput] = useState("");
  const [elevenLabsKeySaved, setElevenLabsKeySaved] = useState(false);
  const [elevenLabsKeyInput, setElevenLabsKeyInput] = useState("");
  const [elevenLabsVoiceIdInput, setElevenLabsVoiceIdInput] = useState("");
  const [lastEmotion, setLastEmotion] = useState<string | null>(null);
  const [handsFreeEnabled, setHandsFreeEnabled] = useState(false);
  const [handsFreeBusy, setHandsFreeBusy] = useState(false);
  const [wakePhraseInput, setWakePhraseInput] = useState("");
  const [closePhraseInput, setClosePhraseInput] = useState("");
  const [handsFreeSessionOpen, setHandsFreeSessionOpenState] = useState(false);
  // Mirrors handsFreeSessionOpen for synchronous reads from event handlers.
  // React (StrictMode especially) may invoke a functional state updater
  // more than once as a purity check, so a real side effect — sending a
  // just-heard command to the agent — can never live inside one; this ref
  // is what the voice://final handler below reads instead.
  const handsFreeSessionOpenRef = useRef(false);
  function setHandsFreeSessionOpen(open: boolean) {
    handsFreeSessionOpenRef.current = open;
    setHandsFreeSessionOpenState(open);
  }
  const [autonomy, setAutonomy] = useState<AutonomyLevel>("observe");
  const [halted, setHalted] = useState(false);
  const [auditLog, setAuditLog] = useState<AuditEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [agentInput, setAgentInput] = useState("");
  const [agentLog, setAgentLog] = useState<AgentTurn[]>([]);
  const [agentBusy, setAgentBusy] = useState(false);
  const [voiceError, setVoiceError] = useState<string | null>(null);
  const [isListening, setIsListening] = useState(false);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [taskInput, setTaskInput] = useState("");
  const [showDoneTasks, setShowDoneTasks] = useState(false);
  const [workspaceFiles, setWorkspaceFiles] = useState<WorkspaceEntry[]>([]);
  const [noteFilename, setNoteFilename] = useState("");
  const [noteContent, setNoteContent] = useState("");
  const [filePreview, setFilePreview] = useState<{ name: string; content: string } | null>(null);
  const [fileError, setFileError] = useState<string | null>(null);
  const [browserUrl, setBrowserUrl] = useState("");
  const [browserError, setBrowserError] = useState<string | null>(null);
  const [dueFollowUps, setDueFollowUps] = useState<FollowUp[]>([]);
  const [deltaBrief, setDeltaBrief] = useState<DeltaBrief | null>(null);
  const [briefBusy, setBriefBusy] = useState(false);
  const [pendingAction, setPendingAction] = useState<PendingActionSummary | null>(null);
  const [approvalBusy, setApprovalBusy] = useState(false);
  const [voiceIdSaveStatus, setVoiceIdSaveStatus] = useState<string | null>(null);
  const [activePanel, setActivePanel] = useState<PanelKey | null>(null);
  const [availableUpdate, setAvailableUpdate] = useState<Update | null>(null);
  const [updateBusy, setUpdateBusy] = useState(false);
  const [updateError, setUpdateError] = useState<string | null>(null);
  const [updateCheckBusy, setUpdateCheckBusy] = useState(false);
  const [upToDateMessage, setUpToDateMessage] = useState(false);

  function togglePanel(key: PanelKey) {
    setActivePanel((current) => (current === key ? null : key));
  }

  async function refresh() {
    if (!inTauri) return;
    // One failing call (e.g. listing a home directory that now contains a
    // broken symlink or a permission-restricted item) must never hide the
    // others — a single Promise.all would let one rejection blank out
    // everything, including the API key badge that has nothing to do with
    // it. Each piece of state updates independently instead.
    const [i, hasKey, hasElevenKey, level, killed, log, taskList, files, dueList, pending] =
      await Promise.allSettled([
        appInfo(),
        hasApiKey(),
        hasElevenLabsKey(),
        getAutonomyLevel(),
        isHalted(),
        listAuditLog(10),
        listTasks(),
        listWorkspaceFiles(),
        listDueFollowUps(),
        getPendingAction(),
      ]);
    if (i.status === "fulfilled") setInfo(i.value);
    if (hasKey.status === "fulfilled") setKeySaved(hasKey.value);
    if (hasElevenKey.status === "fulfilled") setElevenLabsKeySaved(hasElevenKey.value);
    if (level.status === "fulfilled") setAutonomy(level.value);
    if (killed.status === "fulfilled") setHalted(killed.value);
    if (log.status === "fulfilled") setAuditLog(log.value);
    if (taskList.status === "fulfilled") setTasks(taskList.value);
    if (files.status === "fulfilled") setWorkspaceFiles(files.value);
    if (dueList.status === "fulfilled") setDueFollowUps(dueList.value);
    if (pending.status === "fulfilled") setPendingAction(pending.value);

    const firstFailure = [i, hasKey, hasElevenKey, level, killed, log, taskList, files, dueList, pending].find(
      (r) => r.status === "rejected",
    );
    setError(firstFailure ? String((firstFailure as PromiseRejectedResult).reason) : null);
  }

  useEffect(() => {
    refresh();
    if (inTauri) {
      generateDeltaBrief().then(setDeltaBrief);
      // Loaded once here (not folded into `refresh`'s Promise.allSettled)
      // so a later refresh — triggered after some unrelated action — never
      // clobbers phrases Mona is mid-way through editing in the Settings
      // panel below.
      getHandsFreeSettings().then((s) => {
        setHandsFreeEnabled(s.enabled);
        setWakePhraseInput(s.wake_phrase);
        setClosePhraseInput(s.close_phrase);
      });
      getElevenLabsVoiceId().then(setElevenLabsVoiceIdInput);
      // Silent on failure (e.g. offline, or the update endpoint has
      // nothing newer) — this is a background convenience check, not
      // something that should ever interrupt Mona with an error banner
      // just for not finding an update.
      checkForUpdate()
        .then((update) => setAvailableUpdate(update))
        .catch(() => {});
    }
  }, []);

  async function handleCheckForUpdate() {
    setUpdateCheckBusy(true);
    setUpdateError(null);
    setUpToDateMessage(false);
    try {
      const update = await checkForUpdate();
      setAvailableUpdate(update);
      if (!update) setUpToDateMessage(true);
    } catch (e) {
      setUpdateError(String(e));
    } finally {
      setUpdateCheckBusy(false);
    }
  }

  async function handleInstallUpdate() {
    if (!availableUpdate) return;
    setUpdateBusy(true);
    setUpdateError(null);
    try {
      await installUpdateAndRestart(availableUpdate);
    } catch (e) {
      setUpdateError(String(e));
      setUpdateBusy(false);
    }
  }

  // Voice events can arrive from either the mic button below or the global
  // alt+A shortcut (which works even while Amin's window isn't focused) —
  // both are handled by the same Rust code, so one listener here covers
  // both triggers.
  useEffect(() => {
    if (!inTauri) return;
    const unlistenPromises = [
      listen<string>("voice://partial", (e) => setAgentInput(e.payload)),
      // Manual tap-to-toggle only fills the input box — Mona still presses
      // "إرسال". During an open hands-free session (handsFreeSessionOpen)
      // the same event instead sends itself straight to the agent, since
      // the whole point of hands-free is not touching anything.
      listen<string>("voice://final", (e) => {
        setAgentInput(e.payload);
        if (handsFreeSessionOpenRef.current) handleSendToAgent(e.payload);
      }),
      listen<string>("voice://error", (e) => {
        setVoiceError(e.payload);
        setIsListening(false);
        setAminState("warning");
        setTimeout(
          () => setAminState((s) => (s === "warning" ? (handsFreeEnabled ? "armed" : "idle") : s)),
          1400,
        );
      }),
      listen<string>("voice://state", (e) => {
        setIsListening(e.payload === "listening");
        setAminState(e.payload === "listening" ? "listening" : "idle");
      }),
      listen("voice://speaking-finished", () =>
        setAminState((s) => (s === "speaking" ? (handsFreeEnabled ? "armed" : "idle") : s)),
      ),
      // Hands-free: armed = passively watching for the wake phrase (nothing
      // said yet reaches the input box or the agent); "listening" fires
      // once the wake phrase opens an actual command session.
      listen("voice://hands-free-armed", () => {
        setHandsFreeSessionOpen(false);
        setAminState("armed");
      }),
      listen("voice://hands-free-listening", () => {
        setHandsFreeSessionOpen(true);
        setAminState("listening");
      }),
      listen("voice://hands-free-closed", () => {
        setHandsFreeSessionOpen(false);
      }),
    ];
    return () => {
      unlistenPromises.forEach((p) => p.then((unlisten) => unlisten()));
    };
  }, [handsFreeEnabled]);

  async function handleSaveKey() {
    if (!keyInput.trim()) return;
    await saveApiKey(keyInput.trim());
    setKeyInput("");
    await refresh();
  }

  async function handleClearKey() {
    await clearApiKey();
    await refresh();
  }

  async function handleSaveElevenLabsKey() {
    if (!elevenLabsKeyInput.trim()) return;
    await saveElevenLabsKey(elevenLabsKeyInput.trim());
    setElevenLabsKeyInput("");
    await refresh();
  }

  async function handleClearElevenLabsKey() {
    await clearElevenLabsKey();
    await refresh();
  }

  async function handleSaveElevenLabsVoiceId() {
    setVoiceIdSaveStatus(null);
    try {
      await saveElevenLabsVoiceId(elevenLabsVoiceIdInput.trim());
      setVoiceIdSaveStatus("تم الحفظ ✅");
    } catch (e) {
      setVoiceIdSaveStatus(`⚠️ ما اتحفظش: ${String(e)}`);
    }
  }

  async function handleToggleHandsFree() {
    if (handsFreeBusy) return;
    setHandsFreeBusy(true);
    setVoiceError(null);
    try {
      const next = !handsFreeEnabled;
      await setHandsFreeMode(next);
      setHandsFreeEnabled(next);
      if (!next) {
        setHandsFreeSessionOpen(false);
        setAminState((s) => (s === "armed" || s === "listening" ? "idle" : s));
      }
    } catch (e) {
      setVoiceError(String(e));
    } finally {
      setHandsFreeBusy(false);
    }
  }

  async function handleSaveHandsFreePhrases() {
    setVoiceError(null);
    try {
      await saveHandsFreePhrases(wakePhraseInput.trim(), closePhraseInput.trim());
    } catch (e) {
      setVoiceError(String(e));
    }
  }

  async function handleAutonomyChange(level: AutonomyLevel) {
    await setAutonomyLevel(level);
    await refresh();
  }

  async function handleKillSwitch() {
    await setKillSwitch(!halted);
    await refresh();
  }

  /** Speaks Amin's reply aloud. `voice://speaking-finished` (listened to
   * above) resets aminState to idle when the real speech ends; the timeout
   * here is only a safety net for when speaking never actually starts
   * (engine missing, TTS failure) so Amin doesn't stay stuck mid-state. */
  function speak(text: string, emotion?: string | null) {
    if (!inTauri) return;
    speakText(text, emotion).catch((e) => {
      setVoiceError(`تعذّر نطق الرد: ${String(e)}`);
      setAminState((s) => (s === "speaking" ? (handsFreeEnabled ? "armed" : "idle") : s));
    });
    setTimeout(() => {
      setAminState((s) => (s === "speaking" ? (handsFreeEnabled ? "armed" : "idle") : s));
    }, 25000);
  }

  /** Interrupts Amin mid-reply — there was no way to do this at all before;
   * only the "wait it out" option existed. */
  async function handleStopSpeaking() {
    try {
      await stopSpeaking();
    } catch (e) {
      setVoiceError(String(e));
    }
    setAminState((s) => (s === "speaking" ? (handsFreeEnabled ? "armed" : "idle") : s));
  }

  /** `overrideText` lets hands-free mode send a just-heard command straight
   * through without a round trip via `agentInput` state, which would
   * otherwise be stale in the same tick it's set. */
  async function handleSendToAgent(overrideText?: string) {
    const text = (overrideText ?? agentInput).trim();
    if (!text || agentBusy) return;

    setAgentLog((log) => [...log, { role: "user", text }]);
    setAgentInput("");
    setAgentBusy(true);
    setAminState("thinking");

    try {
      const reply = await sendAgentMessage(text);
      setLastEmotion(reply.emotion);
      setAgentLog((log) => [...log, { role: "amin", text: reply.text }]);
      setAminState("speaking");
      speak(reply.text, reply.emotion);
    } catch (e) {
      setAgentLog((log) => [...log, { role: "amin", text: `⚠️ ${String(e)}` }]);
      setAminState("warning");
      setTimeout(() => setAminState("idle"), 1400);
    } finally {
      setAgentBusy(false);
      await refresh();
    }
  }

  /**
   * The [تنفيذ]/[إلغاء] buttons on the pending-approval card send exactly
   * the same words `confirmation::interpret` already recognizes from typed
   * or spoken replies — this is a shortcut to that same real approval
   * path (send_agent_message → resolve_pending_action), not a separate
   * bypass of it.
   */
  async function handleApprovalDecision(word: "موافقة" | "إلغاء") {
    setApprovalBusy(true);
    try {
      await handleSendToAgent(word);
    } finally {
      setApprovalBusy(false);
    }
  }

  async function handleNewConversation() {
    await clearAgentConversation();
    setAgentLog([]);
  }

  async function handleMicDown() {
    setVoiceError(null);
    try {
      await startVoiceCapture();
      setIsListening(true);
      setAminState("listening");
    } catch (e) {
      setVoiceError(String(e));
    }
  }

  async function handleMicUp() {
    if (!isListening) return;
    await stopVoiceCapture();
    setIsListening(false);
    setAminState("idle");
  }

  /** Tap once to start, tap again to stop — holding a mouse button down
   * the whole time she's talking isn't a reasonable ask. Recognition also
   * still ends on its own after a natural pause in speech, so tapping
   * again is "I'm done early," not the only way to stop. */
  function handleMicToggle() {
    if (isListening) {
      handleMicUp();
    } else {
      handleMicDown();
    }
  }

  async function handleCreateTask() {
    const title = taskInput.trim();
    if (!title) return;
    setTaskInput("");
    await createTask(title);
    await refresh();
  }

  async function handleQuickCapture() {
    const text = agentInput.trim();
    if (!text) return;
    setAgentInput("");
    await quickCapture(text);
    await refresh();
  }

  async function handleToggleTask(task: Task) {
    await setTaskStatus(task.id, task.status === "done" ? "open" : "done");
    await refresh();
  }

  const visibleTasks = showDoneTasks ? tasks : tasks.filter((t) => t.status !== "done");

  async function handleSaveNote() {
    const name = noteFilename.trim();
    if (!name || !noteContent.trim()) return;
    setFileError(null);
    try {
      await writeWorkspaceFile(name, noteContent);
      setNoteFilename("");
      setNoteContent("");
      await refresh();
    } catch (e) {
      setFileError(String(e));
    }
  }

  async function handleViewFile(name: string) {
    setFileError(null);
    try {
      const content = await readWorkspaceFile(name);
      setFilePreview({ name, content });
    } catch (e) {
      setFileError(String(e));
    }
  }

  async function handleDeleteFile(name: string) {
    setFileError(null);
    try {
      await deleteWorkspaceFile(name);
      if (filePreview?.name === name) setFilePreview(null);
      await refresh();
    } catch (e) {
      setFileError(String(e));
    }
  }

  async function handleOpenBrowser() {
    const url = browserUrl.trim();
    if (!url) return;
    setBrowserError(null);
    try {
      await openBrowserUrl(url);
    } catch (e) {
      setBrowserError(String(e));
    }
  }

  async function handleRemindMe(task: Task) {
    await createFollowUp(task.id, new Date().toISOString());
    await refresh();
  }

  async function handleEscalate(followUp: FollowUp) {
    await escalateFollowUp(followUp.id);
    await refresh();
  }

  async function handleResolveFollowUp(followUp: FollowUp) {
    await setFollowUpStatus(followUp.id, "resolved");
    await refresh();
  }

  async function handleGetBrief() {
    const b = await generateDeltaBrief();
    setDeltaBrief(b);
  }

  async function handleNarrateBrief() {
    if (!deltaBrief || agentBusy) return;
    const prompt =
      "هات لي ملخص سريع وطبيعي بناءً على البيانات دي (Delta Brief محلي، لسه معندهوش Gmail/Calendar):\n" +
      JSON.stringify(deltaBrief, null, 2);

    setAgentLog((log) => [...log, { role: "user", text: "📋 ملخص التغييرات" }]);
    setBriefBusy(true);
    setAminState("thinking");
    try {
      const reply = await sendAgentMessage(prompt);
      setLastEmotion(reply.emotion);
      setAgentLog((log) => [...log, { role: "amin", text: reply.text }]);
      setAminState("speaking");
      speak(reply.text, reply.emotion);
    } catch (e) {
      setAgentLog((log) => [...log, { role: "amin", text: `⚠️ ${String(e)}` }]);
      setAminState("warning");
      setTimeout(() => setAminState("idle"), 1400);
    } finally {
      setBriefBusy(false);
      await refresh();
    }
  }

  const activeNavItem = NAV_ITEMS.find((n) => n.key === activePanel) ?? null;

  return (
    <>
      {showSplash && <Splash onDone={() => setShowSplash(false)} />}
      <main className="amin-world">
        <div className="amin-world-presence">
          <AminPresence state={aminState} emotion={lastEmotion} />
        </div>
        <div className="amin-world-ambient" aria-hidden="true" />

        <div className="amin-world-stage">
          {!inTauri && (
            <p className="banner banner-warning">
              شغّال خارج بيئة Tauri (متصفح عادي) — أوامر الخلفية معطّلة. استخدمي
              <code> npm run tauri dev</code> لتشغيل الجزء الخاص بـ Rust.
            </p>
          )}
          {error && <p className="banner banner-danger">{error}</p>}
          {voiceError && <p className="banner banner-warning">🎤 {voiceError}</p>}
          {availableUpdate && (
            <p className="banner banner-info">
              ⬆️ في تحديث جديد لأمين (نسخة {availableUpdate.version}) — تحديث تلقائي، من غير ما
              تنزّلي أي حاجة بنفسك.
              <button
                type="button"
                className="banner-action"
                onClick={handleInstallUpdate}
                disabled={updateBusy}
              >
                {updateBusy ? "جاري التحديث…" : "حدّثي الآن"}
              </button>
            </p>
          )}
          {updateError && <p className="banner banner-warning">⬆️ {updateError}</p>}

          <div className="amin-world-chips">
            {deltaBrief && (
              <>
                <span className="badge">{deltaBrief.open_tasks} مهمة مفتوحة</span>
                <span className="badge">{deltaBrief.due_follow_ups} متابعة مستحقة</span>
              </>
            )}
          </div>

          {agentLog.length > 0 && (
            <div className="amin-world-log-wrap">
              <ul className="agent-log">
                {agentLog.map((turn, i) => (
                  <li key={i} className={`agent-turn agent-turn-${turn.role}`}>
                    <span className="agent-turn-role">{turn.role === "user" ? "أنتِ" : "أمين"}</span>
                    <span className="agent-turn-text">{turn.text}</span>
                  </li>
                ))}
              </ul>
              <button
                className="chip amin-world-log-clear"
                onClick={handleNewConversation}
                disabled={!inTauri}
              >
                محادثة جديدة
              </button>
            </div>
          )}
        </div>

        <nav className="side-rail" aria-label="أقسام أمين">
          {NAV_ITEMS.map((item) => (
            <button
              key={item.key}
              className={activePanel === item.key ? "side-rail-btn side-rail-btn-active" : "side-rail-btn"}
              onClick={() => togglePanel(item.key)}
              title={item.label}
              aria-label={item.label}
              aria-pressed={activePanel === item.key}
            >
              <span aria-hidden="true">{item.icon}</span>
            </button>
          ))}
        </nav>

        <button
          className="status-pill"
          onClick={() => togglePanel("settings")}
          title="الاستقلالية والإيقاف الطارئ"
        >
          <span className={halted ? "status-dot status-dot-danger" : "status-dot status-dot-ok"} />
          {AUTONOMY_LABELS[autonomy]}
        </button>

        {activeNavItem && (
          <div className="work-area" role="dialog" aria-label={activeNavItem.label}>
            <div className="work-area-header">
              <h2>{activeNavItem.label}</h2>
              <button className="chip" onClick={() => setActivePanel(null)}>
                إغلاق
              </button>
            </div>
            <div className="work-area-body">
              {activePanel === "brief" && (
                <>
                  <div className="panel-header-row">
                    <p className="text-muted">لمحة سريعة على مهامك ومتابعاتك.</p>
                    <button className="chip" onClick={handleGetBrief} disabled={!inTauri}>
                      تحديث
                    </button>
                  </div>
                  {deltaBrief && (
                    <>
                      <div className="brief-stats">
                        <span className="badge">{deltaBrief.open_tasks} مهمة مفتوحة</span>
                        <span className="badge">+{deltaBrief.tasks_created_last_24h} اتضافت (24 ساعة)</span>
                        <span className="badge">✓{deltaBrief.tasks_completed_last_24h} خلصت (24 ساعة)</span>
                        <span className="badge">{deltaBrief.due_follow_ups} متابعة مستحقة</span>
                      </div>
                      <button className="chip" onClick={handleNarrateBrief} disabled={!inTauri || briefBusy}>
                        {briefBusy ? "..." : "🎙️ اطلبي من أمين يحكيلك الملخص"}
                      </button>
                    </>
                  )}
                </>
              )}

              {activePanel === "tasks" && (
                <>
                  <div className="panel-header-row">
                    <p className="text-muted">مهامك وتدوينك السريع.</p>
                    <label className="show-done-toggle">
                      <input
                        type="checkbox"
                        checked={showDoneTasks}
                        onChange={(e) => setShowDoneTasks(e.currentTarget.checked)}
                      />
                      عرض المنجزة
                    </label>
                  </div>
                  {visibleTasks.length === 0 ? (
                    <p className="text-muted">لسه مفيش مهام.</p>
                  ) : (
                    <ul className="task-list">
                      {visibleTasks.map((task) => (
                        <li key={task.id} className="task-row">
                          <button
                            className={task.status === "done" ? "task-check task-check-done" : "task-check"}
                            onClick={() => handleToggleTask(task)}
                            disabled={!inTauri}
                            aria-label={task.status === "done" ? "إرجاعها مفتوحة" : "تحديدها كمنجزة"}
                          >
                            {task.status === "done" ? "✓" : ""}
                          </button>
                          <span className={task.status === "done" ? "task-title task-title-done" : "task-title"}>
                            {task.title}
                          </span>
                          {task.source && (
                            <span className="badge">{arabicLabel(TASK_SOURCE_LABELS, task.source)}</span>
                          )}
                          {task.status !== "done" && (
                            <button
                              className="chip"
                              onClick={() => handleRemindMe(task)}
                              disabled={!inTauri}
                              title="جدولة متابعة مستحقة الآن"
                            >
                              ⏰
                            </button>
                          )}
                        </li>
                      ))}
                    </ul>
                  )}
                  <form
                    className="field-row"
                    onSubmit={(e) => {
                      e.preventDefault();
                      handleCreateTask();
                    }}
                  >
                    <input
                      type="text"
                      placeholder="أضيفي مهمة..."
                      value={taskInput}
                      onChange={(e) => setTaskInput(e.currentTarget.value)}
                      disabled={!inTauri}
                    />
                    <button type="submit" disabled={!inTauri || !taskInput.trim()}>
                      إضافة
                    </button>
                  </form>
                </>
              )}

              {activePanel === "followups" && (
                <>
                  <p className="text-muted">تصعيد المتابعة يرسل تنبيهًا فعليًا من النظام.</p>
                  {dueFollowUps.length === 0 ? (
                    <p className="text-muted">مفيش حاجة مستحقة دلوقتي.</p>
                  ) : (
                    <ul className="task-list">
                      {dueFollowUps.map((f) => {
                        const task = tasks.find((t) => t.id === f.task_id);
                        return (
                          <li key={f.id} className="task-row">
                            <span className="task-title">{task?.title ?? f.task_id}</span>
                            <span className="badge">{arabicLabel(ESCALATION_STAGE_LABELS, f.escalation_stage)}</span>
                            <button className="chip" onClick={() => handleEscalate(f)} disabled={!inTauri}>
                              تصعيد
                            </button>
                            <button
                              className="chip"
                              onClick={() => handleResolveFollowUp(f)}
                              disabled={!inTauri}
                            >
                              تمت المعالجة
                            </button>
                          </li>
                        );
                      })}
                    </ul>
                  )}
                </>
              )}

              {activePanel === "files" && (
                <>
                  <p className="text-muted">
                    الوصول محصور داخل مجلدك الشخصي فقط، ولا يقدر يخرج منه أبدًا. كل ملف — حتى مجرد
                    عرضه — بيستنى موافقتك الصريحة قبل ما ينفذ.
                  </p>
                  {fileError && <p className="banner banner-warning">📁 {fileError}</p>}
                  {workspaceFiles.length === 0 ? (
                    <p className="text-muted">لسه مفيش ملفات.</p>
                  ) : (
                    <ul className="task-list">
                      {workspaceFiles.map((f) => (
                        <li key={f.name} className="task-row">
                          <span className="task-title">{f.is_dir ? `📁 ${f.name}` : `📄 ${f.name}`}</span>
                          {!f.is_dir && (
                            <>
                              <button className="chip" onClick={() => handleViewFile(f.name)}>
                                عرض
                              </button>
                              <button className="chip chip-danger" onClick={() => handleDeleteFile(f.name)}>
                                حذف
                              </button>
                            </>
                          )}
                        </li>
                      ))}
                    </ul>
                  )}
                  {filePreview && (
                    <div className="file-preview">
                      <div className="panel-header-row">
                        <strong>{filePreview.name}</strong>
                        <button className="chip" onClick={() => setFilePreview(null)}>
                          إغلاق
                        </button>
                      </div>
                      <pre className="file-preview-content">{filePreview.content}</pre>
                    </div>
                  )}
                  <div className="field-row">
                    <input
                      type="text"
                      placeholder="اسم الملف (مثال: note.txt)"
                      value={noteFilename}
                      onChange={(e) => setNoteFilename(e.currentTarget.value)}
                      disabled={!inTauri}
                    />
                  </div>
                  <div className="field-row">
                    <textarea
                      className="note-textarea"
                      placeholder="اكتبي محتوى الملف هنا..."
                      value={noteContent}
                      onChange={(e) => setNoteContent(e.currentTarget.value)}
                      disabled={!inTauri}
                    />
                  </div>
                  <div className="field-row">
                    <button
                      onClick={handleSaveNote}
                      disabled={!inTauri || !noteFilename.trim() || !noteContent.trim()}
                    >
                      حفظ الملف
                    </button>
                  </div>
                </>
              )}

              {activePanel === "browser" && (
                <>
                  <p className="text-muted">
                    بيفتح الرابط في نافذة متصفح منعزلة خاصة بأمين، بعيدة عن متصفحك الشخصي.
                  </p>
                  {browserError && <p className="banner banner-warning">🌐 {browserError}</p>}
                  <form
                    className="field-row"
                    onSubmit={(e) => {
                      e.preventDefault();
                      handleOpenBrowser();
                    }}
                  >
                    <input
                      type="text"
                      placeholder="https://..."
                      value={browserUrl}
                      onChange={(e) => setBrowserUrl(e.currentTarget.value)}
                      disabled={!inTauri}
                    />
                    <button type="submit" disabled={!inTauri || !browserUrl.trim()}>
                      فتح
                    </button>
                  </form>
                </>
              )}

              {activePanel === "audit" && (
                <>
                  {auditLog.length === 0 ? (
                    <p className="text-muted">لسه مفيش أحداث.</p>
                  ) : (
                    <ul className="audit-list">
                      {auditLog.map((entry) => (
                        <li key={entry.id}>
                          <span className="text-muted">{entry.ts}</span>
                          <span className="badge">{arabicLabel(RISK_TIER_LABELS, entry.risk_tier)}</span>
                          <strong>{entry.action}</strong>
                          <span className="text-muted">{arabicLabel(DECISION_LABELS, entry.decision)}</span>
                        </li>
                      ))}
                    </ul>
                  )}
                </>
              )}

              {activePanel === "settings" && (
                <>
                  <div className="field-row">
                    <span className="field-label">مفتاح الاتصال بأنثروبيك</span>
                    <span className={keySaved ? "badge badge-success" : "badge"}>
                      {keySaved ? "متحط" : "مش متحط"}
                    </span>
                  </div>
                  <div className="field-row">
                    <input
                      type="password"
                      placeholder="sk-ant-..."
                      value={keyInput}
                      onChange={(e) => setKeyInput(e.currentTarget.value)}
                      disabled={!inTauri}
                    />
                    <button onClick={handleSaveKey} disabled={!inTauri || !keyInput.trim()}>
                      حفظ
                    </button>
                    <button onClick={handleClearKey} disabled={!inTauri || !keySaved}>
                      مسح
                    </button>
                  </div>

                  <div className="field-row">
                    <span className="field-label">صوت أمين البشري (ElevenLabs)</span>
                    <span className={elevenLabsKeySaved ? "badge badge-success" : "badge"}>
                      {elevenLabsKeySaved ? "متحط" : "مش متحط"}
                    </span>
                  </div>
                  <p className="text-muted">
                    اختياري ومدفوع — لو حطيتي المفتاح، هيتكلم أمين بصوت أشبه بالإنسان بدل الصوت
                    الافتراضي المجاني. النص اللي بيقوله بيروح لسيرفرات ElevenLabs عشان تتحول لصوت.
                    من غيره، هيفضل يتكلم بالصوت المحلي زي ما هو دلوقتي.
                  </p>
                  <div className="field-row">
                    <input
                      type="password"
                      placeholder="ElevenLabs API key"
                      value={elevenLabsKeyInput}
                      onChange={(e) => setElevenLabsKeyInput(e.currentTarget.value)}
                      disabled={!inTauri}
                    />
                    <button
                      onClick={handleSaveElevenLabsKey}
                      disabled={!inTauri || !elevenLabsKeyInput.trim()}
                    >
                      حفظ
                    </button>
                    <button onClick={handleClearElevenLabsKey} disabled={!inTauri || !elevenLabsKeySaved}>
                      مسح
                    </button>
                  </div>
                  {elevenLabsKeySaved && (
                    <>
                      <p className="text-muted">
                        مهم: من غير ما تحطي هنا صوت عربي حقيقي من مكتبتك في ElevenLabs، هيتكلم
                        بصوت إنجليزي افتراضي حتى وهو بيقول كلام عربي — وده سبب النطق المكسور اللي
                        سمعتيه. روحي elevenlabs.io ← Voices، اختاري صوت عربي، وانسخي الـ Voice ID
                        بتاعه هنا.
                      </p>
                      <div className="field-row">
                        <input
                          type="text"
                          placeholder="ElevenLabs Voice ID"
                          value={elevenLabsVoiceIdInput}
                          onChange={(e) => {
                            setElevenLabsVoiceIdInput(e.currentTarget.value);
                            setVoiceIdSaveStatus(null);
                          }}
                          disabled={!inTauri}
                        />
                        <button onClick={handleSaveElevenLabsVoiceId} disabled={!inTauri}>
                          حفظ
                        </button>
                      </div>
                      {voiceIdSaveStatus && <p className="text-muted">{voiceIdSaveStatus}</p>}
                    </>
                  )}

                  <div className="field-row">
                    <span className="field-label">الاستماع الحر (بدون لمس أي زرار)</span>
                    <button
                      className={handsFreeEnabled ? "chip chip-active" : "chip"}
                      onClick={handleToggleHandsFree}
                      disabled={!inTauri || handsFreeBusy}
                    >
                      {handsFreeEnabled ? "شغّال — دوسي للإيقاف" : "متوقف — دوسي للتشغيل"}
                    </button>
                  </div>
                  <p className="text-muted">
                    لو شغّلتيه، المايك هيفضل شغّال باستمرار (هتشوفي مؤشر المايك في الماك شغّال طول
                    الوقت) وهو بيراقب محليًا على جهازك بس عشان يسمع عبارة الفتح — مفيش صوت بيتبعت
                    لحد قبل ما تقوليها. أمين هيبدأ يسمعك فعليًا لما تقولي عبارة الفتح، وهيفضل سامعك
                    لحد ما تقولي عبارة القفل أو تسكتي شوية. العبارة دي سر بينك وبينه بس — أي حد يعرفها
                    يقدر يفتحه، فاختاري عبارة مش سهل حد يخمنها.
                  </p>
                  <div className="field-row">
                    <label className="field-label" htmlFor="wake-phrase-input">
                      عبارة الفتح
                    </label>
                    <input
                      id="wake-phrase-input"
                      type="text"
                      value={wakePhraseInput}
                      onChange={(e) => setWakePhraseInput(e.currentTarget.value)}
                      disabled={!inTauri}
                    />
                  </div>
                  <div className="field-row">
                    <label className="field-label" htmlFor="close-phrase-input">
                      عبارة القفل
                    </label>
                    <input
                      id="close-phrase-input"
                      type="text"
                      value={closePhraseInput}
                      onChange={(e) => setClosePhraseInput(e.currentTarget.value)}
                      disabled={!inTauri}
                    />
                    <button
                      onClick={handleSaveHandsFreePhrases}
                      disabled={!inTauri || !wakePhraseInput.trim() || !closePhraseInput.trim()}
                    >
                      حفظ
                    </button>
                  </div>

                  <div className="field-row">
                    <span className="field-label">مستوى الاستقلالية</span>
                    <div className="segmented">
                      {AUTONOMY_LEVELS.map((level) => (
                        <button
                          key={level}
                          className={level === autonomy ? "chip chip-active" : "chip"}
                          onClick={() => handleAutonomyChange(level)}
                          disabled={!inTauri}
                        >
                          {AUTONOMY_LABELS[level]}
                        </button>
                      ))}
                    </div>
                  </div>

                  <div className="field-row">
                    <span className="field-label">مفتاح الإيقاف الطارئ</span>
                    <button
                      className={halted ? "chip chip-danger chip-active" : "chip"}
                      onClick={handleKillSwitch}
                      disabled={!inTauri}
                    >
                      {halted ? "متوقف — دوسي للاستئناف" : "شغّال — دوسي للإيقاف"}
                    </button>
                  </div>

                  <div className="field-row">
                    <span className="text-muted settings-about">
                      {info ? `أمين — الإصدار ${info.version}` : "أمين"}
                    </span>
                    <button onClick={handleCheckForUpdate} disabled={!inTauri || updateCheckBusy}>
                      {updateCheckBusy ? "جاري التحقق…" : "تحقّقي من التحديثات الآن"}
                    </button>
                  </div>
                  {upToDateMessage && (
                    <p className="text-muted">هذه أحدث نسخة متاحة — مفيش تحديث جديد دلوقتي.</p>
                  )}
                  <p className="creator-attribution">
                    {CREATOR_ATTRIBUTION_AR}
                    <span className="text-muted"> · {CREATOR_ATTRIBUTION_EN}</span>
                  </p>
                </>
              )}
            </div>
          </div>
        )}

        {pendingAction && (
          <div className={pendingAction.expired ? "approval-card approval-card-expired" : "approval-card"}>
            <div className="approval-card-body">
              <span className="field-label">
                {pendingAction.expired ? "انتهت صلاحية هذا الطلب" : "أمين مستني موافقتك"}
              </span>
              <p>الإجراء: {pendingAction.description}</p>
            </div>
            {!pendingAction.expired && (
              <div className="approval-card-actions">
                <button
                  className="chip"
                  onClick={() => handleApprovalDecision("موافقة")}
                  disabled={!inTauri || approvalBusy}
                >
                  تنفيذ
                </button>
                <button
                  className="chip chip-danger"
                  onClick={() => handleApprovalDecision("إلغاء")}
                  disabled={!inTauri || approvalBusy}
                >
                  إلغاء
                </button>
              </div>
            )}
          </div>
        )}

        <form
          className="command-bar"
          onSubmit={(e) => {
            e.preventDefault();
            handleSendToAgent();
          }}
        >
          <button
            type="button"
            className={isListening ? "command-bar-mic command-bar-mic-active" : "command-bar-mic"}
            onClick={handleMicToggle}
            disabled={!inTauri || agentBusy || handsFreeEnabled}
            title={
              handsFreeEnabled
                ? handsFreeSessionOpen
                  ? "أمين سامعك دلوقتي — قولي عبارة القفل أو استني شوية لما تخلصي"
                  : "الاستماع الحر شغّال — كلّمي أمين بعبارة الفتح من غير ما تدوسي حاجة"
                : isListening
                  ? "دوسي تاني لو خلصتي كلامك — أو استنيه يوقف من نفسه"
                  : "دوسي وابدئي الكلام — أو استخدمي alt+A من أي مكان"
            }
          >
            🎤
          </button>
          {aminState === "speaking" && (
            <button
              type="button"
              className="command-bar-mic"
              onClick={handleStopSpeaking}
              title="اسكتي أمين دلوقتي"
            >
              🔇
            </button>
          )}
          <input
            type="text"
            className="command-bar-input"
            placeholder="كلّمي أمين أو اكتبي ما تريدين..."
            value={agentInput}
            onChange={(e) => setAgentInput(e.currentTarget.value)}
            disabled={!inTauri || agentBusy}
          />
          <button
            type="button"
            className="command-bar-action"
            onClick={handleQuickCapture}
            disabled={!inTauri || agentBusy || !agentInput.trim()}
            title="احفظي النص ده كمهمة بدل ما تبعتيه لأمين"
          >
            📌
          </button>
          <button type="submit" className="command-bar-send" disabled={!inTauri || agentBusy || !agentInput.trim()}>
            {agentBusy ? "..." : "إرسال"}
          </button>
        </form>
      </main>
    </>
  );
}

export default App;
