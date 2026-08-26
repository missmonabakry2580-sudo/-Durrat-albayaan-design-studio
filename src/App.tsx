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
  type Task,
  type WorkspaceEntry,
  appInfo,
  clearAgentConversation,
  clearApiKey,
  createTask,
  deleteWorkspaceFile,
  getAutonomyLevel,
  hasApiKey,
  isHalted,
  listAuditLog,
  listTasks,
  listWorkspaceFiles,
  openBrowserUrl,
  quickCapture,
  readWorkspaceFile,
  saveApiKey,
  sendAgentMessage,
  setAutonomyLevel,
  setKillSwitch,
  setTaskStatus,
  startVoiceCapture,
  stopVoiceCapture,
  writeWorkspaceFile,
} from "./lib/tauri";
import "./App.css";

const ORB_STATES = Object.keys(ORB_STATE_LABELS) as OrbState[];
const AUTONOMY_LEVELS: AutonomyLevel[] = ["observe", "assist", "delegate", "autopilot"];

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

  async function refresh() {
    if (!inTauri) return;
    try {
      const [i, hasKey, level, killed, log, taskList, files] = await Promise.all([
        appInfo(),
        hasApiKey(),
        getAutonomyLevel(),
        isHalted(),
        listAuditLog(10),
        listTasks(),
        listWorkspaceFiles(),
      ]);
      setInfo(i);
      setKeySaved(hasKey);
      setAutonomy(level);
      setHalted(killed);
      setAuditLog(log);
      setTasks(taskList);
      setWorkspaceFiles(files);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  useEffect(() => {
    refresh();
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

  return (
    <>
      {showSplash && <Splash onDone={() => setShowSplash(false)} />}
      <main className="app-shell">
        <header className="app-header">
          <h1>أمين — Amin</h1>
          <p className="app-subtitle">
            {info
              ? `${info.name} v${info.version}`
              : "Phase 0 — Architecture, Security & Design System"}
          </p>
        </header>

        {!inTauri && (
          <p className="banner banner-warning">
            Running outside the Tauri shell (plain browser) — backend commands are disabled. Use
            <code> npm run tauri dev</code> to exercise the Rust side.
          </p>
        )}
        {error && <p className="banner banner-danger">{error}</p>}

        <section className="panel orb-panel">
          <Orb state={orbState} />
          <div className="orb-switcher" role="group" aria-label="Orb state preview">
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
            <h2>كلمي أمين — Talk to Amin</h2>
            <button
              className="chip"
              onClick={handleNewConversation}
              disabled={!inTauri || agentLog.length === 0}
            >
              New conversation
            </button>
          </div>
          <p className="text-muted">
            Phase 1 Agent Core — conversation only, no tools yet. Amin remembers this session
            until you start a new conversation or quit the app — nothing is saved to disk yet.
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
              title="Push to talk (hold) — or hold alt+A anywhere"
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
              {agentBusy ? "..." : "Send"}
            </button>
            <button
              type="button"
              className="chip"
              onClick={handleQuickCapture}
              disabled={!inTauri || agentBusy || !agentInput.trim()}
              title="Save this text as a task instead of sending it to Amin"
            >
              📌 Capture
            </button>
          </form>
        </section>

        <section className="panel">
          <div className="panel-header-row">
            <h2>مهامي — Tasks</h2>
            <label className="show-done-toggle">
              <input
                type="checkbox"
                checked={showDoneTasks}
                onChange={(e) => setShowDoneTasks(e.currentTarget.checked)}
              />
              Show done
            </label>
          </div>
          <p className="text-muted">
            Phase 2 — local task list and Quick Capture. Use "📌 Capture" above to turn whatever's
            in the message box (typed or spoken) into a task instead of sending it to Amin.
          </p>
          {visibleTasks.length === 0 ? (
            <p className="text-muted">No tasks yet.</p>
          ) : (
            <ul className="task-list">
              {visibleTasks.map((task) => (
                <li key={task.id} className="task-row">
                  <button
                    className={task.status === "done" ? "task-check task-check-done" : "task-check"}
                    onClick={() => handleToggleTask(task)}
                    disabled={!inTauri}
                    aria-label={task.status === "done" ? "Mark as open" : "Mark as done"}
                  >
                    {task.status === "done" ? "✓" : ""}
                  </button>
                  <span className={task.status === "done" ? "task-title task-title-done" : "task-title"}>
                    {task.title}
                  </span>
                  {task.source && <span className="badge">{task.source}</span>}
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
              Add
            </button>
          </form>
        </section>

        <section className="panel">
          <h2>ملفات أمين — Workspace Files</h2>
          <p className="text-muted">
            Phase 2 — confined to one dedicated folder (<code>~/Documents/Amin</code>), never any
            other path on your Mac. See docs/SECURITY.md if you want the details.
          </p>
          {fileError && <p className="banner banner-warning">📁 {fileError}</p>}
          {workspaceFiles.length === 0 ? (
            <p className="text-muted">No files yet.</p>
          ) : (
            <ul className="task-list">
              {workspaceFiles.map((f) => (
                <li key={f.name} className="task-row">
                  <span className="task-title">{f.is_dir ? `📁 ${f.name}` : `📄 ${f.name}`}</span>
                  {!f.is_dir && (
                    <>
                      <button className="chip" onClick={() => handleViewFile(f.name)}>
                        View
                      </button>
                      <button className="chip chip-danger" onClick={() => handleDeleteFile(f.name)}>
                        Delete
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
                  Close
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
              Save file
            </button>
          </div>
        </section>

        <section className="panel">
          <h2>متصفح أمين — Browser</h2>
          <p className="text-muted">
            Phase 2 — opens a page in Amin's own isolated window (its own profile, never your
            personal browser). Amin doesn't read or act on the page yet — that's further browser
            control still to come.
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
              Open
            </button>
          </form>
        </section>

        <section className="panel">
          <h2>Security &amp; Autonomy</h2>

          <div className="field-row">
            <span className="field-label">Anthropic API key</span>
            <span className={keySaved ? "badge badge-success" : "badge"}>
              {keySaved ? "Configured (Keychain)" : "Not configured"}
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
              Save
            </button>
            <button onClick={handleClearKey} disabled={!inTauri || !keySaved}>
              Clear
            </button>
          </div>

          <div className="field-row">
            <span className="field-label">Autonomy level</span>
            <div className="segmented">
              {AUTONOMY_LEVELS.map((level) => (
                <button
                  key={level}
                  className={level === autonomy ? "chip chip-active" : "chip"}
                  onClick={() => handleAutonomyChange(level)}
                  disabled={!inTauri}
                >
                  {level}
                </button>
              ))}
            </div>
          </div>

          <div className="field-row">
            <span className="field-label">Kill switch</span>
            <button
              className={halted ? "chip chip-danger chip-active" : "chip"}
              onClick={handleKillSwitch}
              disabled={!inTauri}
            >
              {halted ? "HALTED — click to resume" : "Running — click to halt"}
            </button>
          </div>
        </section>

        <section className="panel">
          <h2>Audit log (last 10)</h2>
          {auditLog.length === 0 ? (
            <p className="text-muted">No events yet.</p>
          ) : (
            <ul className="audit-list">
              {auditLog.map((entry) => (
                <li key={entry.id}>
                  <span className="text-muted">{entry.ts}</span>
                  <span className="badge">{entry.risk_tier}</span>
                  <strong>{entry.action}</strong>
                  <span className="text-muted">{entry.decision}</span>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="panel about-panel">
          <h2>حول أمين — About Amin</h2>
          <p className="text-muted">
            {info ? `${info.name} v${info.version}` : "Amin"} — Observe → Understand → Decide
            within policy → Execute → Follow up → Report.
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
