import { useEffect, useState } from "react";
import { Orb } from "./components/orb/Orb";
import { ORB_STATE_LABELS, type OrbState } from "./components/orb/types";
import { Splash } from "./components/splash/Splash";
import { CREATOR_ATTRIBUTION_AR, CREATOR_ATTRIBUTION_EN } from "./lib/branding";
import {
  type AppInfo,
  type AuditEntry,
  type AutonomyLevel,
  appInfo,
  clearApiKey,
  getAutonomyLevel,
  hasApiKey,
  isHalted,
  listAuditLog,
  saveApiKey,
  sendAgentMessage,
  setAutonomyLevel,
  setKillSwitch,
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

  async function refresh() {
    if (!inTauri) return;
    try {
      const [i, hasKey, level, killed, log] = await Promise.all([
        appInfo(),
        hasApiKey(),
        getAutonomyLevel(),
        isHalted(),
        listAuditLog(10),
      ]);
      setInfo(i);
      setKeySaved(hasKey);
      setAutonomy(level);
      setHalted(killed);
      setAuditLog(log);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  useEffect(() => {
    refresh();
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
          <h2>كلمي أمين — Talk to Amin</h2>
          <p className="text-muted">
            Phase 1 Agent Core — conversation only, no tools yet. Each message is a fresh turn
            (no memory between messages until Phase 4).
          </p>
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
            <input
              type="text"
              placeholder="اكتبي رسالة لأمين..."
              value={agentInput}
              onChange={(e) => setAgentInput(e.currentTarget.value)}
              disabled={!inTauri || agentBusy}
            />
            <button type="submit" disabled={!inTauri || agentBusy || !agentInput.trim()}>
              {agentBusy ? "..." : "Send"}
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
