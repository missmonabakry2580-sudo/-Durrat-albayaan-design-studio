import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Orb } from "./components/orb/Orb";
import { ORB_STATE_LABELS, type OrbState } from "./components/orb/types";
import { Splash } from "./components/splash/Splash";
import { CREATOR_ATTRIBUTION_AR, CREATOR_ATTRIBUTION_EN } from "./lib/branding";
import {
  type AppInfo,
  type AuditEntry,
  type AutonomyLevel,
  type DeltaBrief,
  type FollowUp,
  type Task,
  type WorkspaceEntry,
  appInfo,
  clearAgentConversation,
  clearApiKey,
  createFollowUp,
  createTask,
  deleteWorkspaceFile,
  escalateFollowUp,
  generateDeltaBrief,
  getAutonomyLevel,
  hasApiKey,
  isHalted,
  listAuditLog,
  listDueFollowUps,
  listTasks,
  listWorkspaceFiles,
  openBrowserUrl,
  quickCapture,
  readWorkspaceFile,
  saveApiKey,
  sendAgentMessage,
  setAutonomyLevel,
  setFollowUpStatus,
  setKillSwitch,
  setTaskStatus,
  startVoiceCapture,
  stopVoiceCapture,
  writeWorkspaceFile,
} from "./lib/tauri";
import "./App.css";

const ORB_STATES = Object.keys(ORB_STATE_LABELS) as OrbState[];
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

/** True once we're actually running inside the Tauri shell, not a plain browser tab. */
const inTauri = "__TAURI_INTERNALS__" in window;

function App() {
  const [showSplash, setShowSplash] = useState(true);
  const [orbState, setOrbState] = useState<OrbState>("idle");
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [keySaved, setKeySaved] = useState(false);
  const [keyInput, setKeyInput] = useState("");
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

  async function refresh() {
    if (!inTauri) return;
    // One failing call (e.g. listing a home directory that now contains a
    // broken symlink or a permission-restricted item) must never hide the
    // others — a single Promise.all would let one rejection blank out
    // everything, including the API key badge that has nothing to do with
    // it. Each piece of state updates independently instead.
    const [i, hasKey, level, killed, log, taskList, files, dueList] = await Promise.allSettled([
      appInfo(),
      hasApiKey(),
      getAutonomyLevel(),
      isHalted(),
      listAuditLog(10),
      listTasks(),
      listWorkspaceFiles(),
      listDueFollowUps(),
    ]);
    if (i.status === "fulfilled") setInfo(i.value);
    if (hasKey.status === "fulfilled") setKeySaved(hasKey.value);
    if (level.status === "fulfilled") setAutonomy(level.value);
    if (killed.status === "fulfilled") setHalted(killed.value);
    if (log.status === "fulfilled") setAuditLog(log.value);
    if (taskList.status === "fulfilled") setTasks(taskList.value);
    if (files.status === "fulfilled") setWorkspaceFiles(files.value);
    if (dueList.status === "fulfilled") setDueFollowUps(dueList.value);

    const firstFailure = [i, hasKey, level, killed, log, taskList, files, dueList].find(
      (r) => r.status === "rejected",
    );
    setError(firstFailure ? String((firstFailure as PromiseRejectedResult).reason) : null);
  }

  useEffect(() => {
    refresh();
    if (inTauri) generateDeltaBrief().then(setDeltaBrief);
  }, []);

  // Voice events can arrive from either the mic button below or the global
  // alt+A shortcut (which works even while Amin's window isn't focused) —
  // both are handled by the same Rust code, so one listener here covers
  // both triggers. See docs/ARCHITECTURE.md "Voice pipeline": unverified
  // end to end until the native transcriber helper is built on a real Mac.
  useEffect(() => {
    if (!inTauri) return;
    const unlistenPromises = [
      listen<string>("voice://partial", (e) => setAgentInput(e.payload)),
      listen<string>("voice://final", (e) => setAgentInput(e.payload)),
      listen<string>("voice://error", (e) => {
        setVoiceError(e.payload);
        setIsListening(false);
        setOrbState("warning");
        setTimeout(() => setOrbState("idle"), 1400);
      }),
      listen<string>("voice://state", (e) => {
        setIsListening(e.payload === "listening");
        setOrbState(e.payload === "listening" ? "listening" : "idle");
      }),
    ];
    return () => {
      unlistenPromises.forEach((p) => p.then((unlisten) => unlisten()));
    };
  }, []);

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

  async function handleAutonomyChange(level: AutonomyLevel) {
    await setAutonomyLevel(level);
    await refresh();
  }

  async function handleKillSwitch() {
    await setKillSwitch(!halted);
    await refresh();
  }

  async function handleSendToAgent() {
    const text = agentInput.trim();
    if (!text || agentBusy) return;

    setAgentLog((log) => [...log, { role: "user", text }]);
    setAgentInput("");
    setAgentBusy(true);
    setOrbState("thinking");

    try {
      const reply = await sendAgentMessage(text);
      setAgentLog((log) => [...log, { role: "amin", text: reply }]);
      setOrbState("speaking");
    } catch (e) {
      setAgentLog((log) => [...log, { role: "amin", text: `⚠️ ${String(e)}` }]);
      setOrbState("warning");
    } finally {
      setAgentBusy(false);
      await refresh();
      setTimeout(() => setOrbState("idle"), 1400);
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
      setOrbState("listening");
    } catch (e) {
      setVoiceError(String(e));
    }
  }

  async function handleMicUp() {
    if (!isListening) return;
    await stopVoiceCapture();
    setIsListening(false);
    setOrbState("idle");
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

    setAgentLog((log) => [...log, { role: "user", text: "📋 Delta Brief" }]);
    setBriefBusy(true);
    setOrbState("thinking");
    try {
      const reply = await sendAgentMessage(prompt);
      setAgentLog((log) => [...log, { role: "amin", text: reply }]);
      setOrbState("speaking");
    } catch (e) {
      setAgentLog((log) => [...log, { role: "amin", text: `⚠️ ${String(e)}` }]);
      setOrbState("warning");
    } finally {
      setBriefBusy(false);
      await refresh();
      setTimeout(() => setOrbState("idle"), 1400);
    }
  }

  return (
    <>
      {showSplash && <Splash onDone={() => setShowSplash(false)} />}
      <main className="app-shell">
        <header className="app-header">
          <h1>أمين</h1>
          <p className="app-subtitle">
            {info ? `الإصدار ${info.version}` : "المرحلة صفر — البنية والأمان ونظام التصميم"}
          </p>
        </header>

        {!inTauri && (
          <p className="banner banner-warning">
            شغّال خارج بيئة Tauri (متصفح عادي) — أوامر الخلفية معطّلة. استخدمي
            <code> npm run tauri dev</code> لتشغيل الجزء الخاص بـ Rust.
          </p>
        )}
        {error && <p className="banner banner-danger">{error}</p>}

        <section className="panel orb-panel">
          <Orb state={orbState} />
          <div className="orb-switcher" role="group" aria-label="معاينة حالات أمين">
            {ORB_STATES.map((s) => (
              <button
                key={s}
                className={s === orbState ? "chip chip-active" : "chip"}
                onClick={() => setOrbState(s)}
              >
                {ORB_STATE_LABELS[s]}
              </button>
            ))}
          </div>
        </section>

        <section className="panel">
          <div className="panel-header-row">
            <h2>ملخص التغييرات</h2>
            <button className="chip" onClick={handleGetBrief} disabled={!inTauri}>
              تحديث
            </button>
          </div>
          <p className="text-muted">
            ملخص محلي بس (لسه من غير جيميل/كالندر) — "إيه اللي اتغير" بالاعتماد على بيانات أمين
            المحلية فقط. البيانات الحقيقية من جيميل والكالندر هتنضم لما الربط يتم.
          </p>
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
        </section>

        <section className="panel">
          <div className="panel-header-row">
            <h2>كلمي أمين</h2>
            <button
              className="chip"
              onClick={handleNewConversation}
              disabled={!inTauri || agentLog.length === 0}
            >
              محادثة جديدة
            </button>
          </div>
          <p className="text-muted">
            محادثة نصية وصوتية مع أمين — بيفتكر كلامكم لحد ما تبدئي محادثة جديدة أو تقفلي
            التطبيق؛ لسه مفيش حفظ دائم على القرص.
          </p>
          {voiceError && <p className="banner banner-warning">🎤 {voiceError}</p>}
          {agentLog.length > 0 && (
            <ul className="agent-log">
              {agentLog.map((turn, i) => (
                <li key={i} className={`agent-turn agent-turn-${turn.role}`}>
                  <span className="agent-turn-role">{turn.role === "user" ? "أنتِ" : "أمين"}</span>
                  <span className="agent-turn-text">{turn.text}</span>
                </li>
              ))}
            </ul>
          )}
          <form
            className="field-row"
            onSubmit={(e) => {
              e.preventDefault();
              handleSendToAgent();
            }}
          >
            <button
              type="button"
              className={isListening ? "chip chip-active" : "chip"}
              onMouseDown={handleMicDown}
              onMouseUp={handleMicUp}
              onMouseLeave={handleMicUp}
              disabled={!inTauri || agentBusy}
              title="اضغطي مع الاستمرار للتحدث — أو استخدمي alt+A من أي مكان"
            >
              🎤
            </button>
            <input
              type="text"
              placeholder="اكتبي رسالة لأمين... أو استخدمي المايك"
              value={agentInput}
              onChange={(e) => setAgentInput(e.currentTarget.value)}
              disabled={!inTauri || agentBusy}
            />
            <button type="submit" disabled={!inTauri || agentBusy || !agentInput.trim()}>
              {agentBusy ? "..." : "إرسال"}
            </button>
            <button
              type="button"
              className="chip"
              onClick={handleQuickCapture}
              disabled={!inTauri || agentBusy || !agentInput.trim()}
              title="احفظي النص ده كمهمة بدل ما تبعتيه لأمين"
            >
              📌 تدوين
            </button>
          </form>
        </section>

        <section className="panel">
          <div className="panel-header-row">
            <h2>مهامي</h2>
            <label className="show-done-toggle">
              <input
                type="checkbox"
                checked={showDoneTasks}
                onChange={(e) => setShowDoneTasks(e.currentTarget.checked)}
              />
              عرض المنجزة
            </label>
          </div>
          <p className="text-muted">
            قائمة المهام والتدوين السريع. استخدمي "📌 تدوين" فوق عشان تحوّلي أي حاجة في خانة
            الرسالة (مكتوبة أو متكلمة) لمهمة بدل ما تتبعت لأمين.
          </p>
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
                      title="جدولة متابعة مستحقة الآن (تجربة لمحرك المتابعات)"
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
        </section>

        <section className="panel">
          <h2>متابعات مستحقة</h2>
          <p className="text-muted">
            لسه مفيش إيميل (محتاج ربط جيميل)، لكن تصعيد المتابعة تحت بيبعت إشعار حقيقي من النظام.
            دوسي ⏰ جنب أي مهمة فوق عشان تجربيها.
          </p>
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
        </section>

        <section className="panel">
          <h2>ملفات أمين</h2>
          <p className="text-muted">
            الوصول محصور داخل مجلدك الشخصي فقط، ولا يقدر يخرج منه أبدًا. كل ملف — حتى مجرد عرضه —
            بيستنى موافقتك الصريحة قبل ما ينفذ.
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
        </section>

        <section className="panel">
          <h2>متصفح أمين</h2>
          <p className="text-muted">
            بيفتح الصفحة في نافذة منعزلة خاصة بأمين (بروفايل خاص بيه، مش متصفحك الشخصي أبدًا).
            أمين لسه ما بيقراش أو يتصرف في محتوى الصفحة — ده تطوير قادم منفصل.
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
        </section>

        <section className="panel">
          <h2>الأمان والاستقلالية</h2>

          <div className="field-row">
            <span className="field-label">مفتاح الاتصال بأنثروبيك</span>
            <span className={keySaved ? "badge badge-success" : "badge"}>
              {keySaved ? "متحط (في الـ Keychain)" : "مش متحط"}
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
        </section>

        <section className="panel">
          <h2>سجل المراجعة (آخر ١٠)</h2>
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
        </section>

        <section className="panel about-panel">
          <h2>حول أمين</h2>
          <p className="text-muted">
            {info ? `أمين — الإصدار ${info.version}` : "أمين"} — يلاحظ ← يفهم ← يقرر ضمن السياسة ←
            ينفذ ← يتابع ← يبلّغ.
          </p>
          <p className="creator-attribution">
            {CREATOR_ATTRIBUTION_AR}
            <span className="text-muted"> · {CREATOR_ATTRIBUTION_EN}</span>
          </p>
        </section>
      </main>
    </>
  );
}

export default App;
