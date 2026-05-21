import React from "react";
import ReactDOM from "react-dom/client";
import { RefreshCw, Terminal, Zap } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import "./styles.css";

type TerminalWindow = {
  hwnd: number;
  pid: number;
  title: string;
  processPath: string;
  kind: string;
};

type AgentSession = {
  agentId: string;
  kind: string;
  name: string;
  cwd: string;
  transcriptPath: string;
  status: string;
  terminalHwnd: number | null;
};

function App() {
  const [windows, setWindows] = React.useState<TerminalWindow[]>([]);
  const [agents, setAgents] = React.useState<AgentSession[]>([]);
  const [status, setStatus] = React.useState("Ready");
  const [loading, setLoading] = React.useState(false);

  const refresh = React.useCallback(async () => {
    setLoading(true);
    setStatus("Scanning agents and terminal windows");
    try {
      const [discoveredAgents, discoveredWindows] = await Promise.all([
        invoke<AgentSession[]>("discover_agent_sessions"),
        invoke<TerminalWindow[]>("discover_terminal_windows"),
      ]);
      setAgents(discoveredAgents);
      setWindows(discoveredWindows);
      setStatus(
        `Found ${discoveredAgents.length} agent session(s), ${discoveredWindows.length} terminal window(s)`,
      );
    } catch (error) {
      setStatus(String(error));
    } finally {
      setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    void refresh();
  }, [refresh]);

  async function focusWindow(hwnd: number) {
    setStatus(`Focusing 0x${hwnd.toString(16)}`);
    try {
      await invoke("focus_terminal_window", { hwnd });
      setStatus(`Focused 0x${hwnd.toString(16)}`);
    } catch (error) {
      setStatus(String(error));
    }
  }

  return (
    <main className="app-shell">
      <header className="topbar">
        <div>
          <h1>Vibe Bridge</h1>
          <p>{status}</p>
        </div>
        <button className="primary-button" onClick={refresh} disabled={loading}>
          <RefreshCw size={16} />
          Refresh
        </button>
      </header>

      <section className="terminal-list" aria-label="Terminal windows">
        <h2>Agent Sessions</h2>
        {agents.length === 0 ? (
          <div className="empty-state">
            <Terminal size={22} />
            <span>No agent sessions discovered from hooks or transcripts.</span>
          </div>
        ) : (
          agents.map((agent) => (
            <article className="terminal-row" key={`${agent.kind}-${agent.agentId}`}>
              <Terminal size={18} />
              <div className="terminal-main">
                <strong>{agent.name || agent.agentId}</strong>
                <span>
                  {agent.kind} · {agent.status} · hwnd{" "}
                  {agent.terminalHwnd ? `0x${agent.terminalHwnd.toString(16)}` : "unbound"}
                </span>
                <code>{agent.cwd || agent.transcriptPath}</code>
              </div>
              <button
                className="icon-button"
                disabled={!agent.terminalHwnd}
                onClick={() => agent.terminalHwnd && void focusWindow(agent.terminalHwnd)}
              >
                <Zap size={16} />
              </button>
            </article>
          ))
        )}

        <h2>Terminal Windows</h2>
        {windows.length === 0 ? (
          <div className="empty-state">
            <Terminal size={22} />
            <span>No terminal windows discovered.</span>
          </div>
        ) : (
          windows.map((window) => (
            <article className="terminal-row" key={`${window.hwnd}-${window.pid}`}>
              <Terminal size={18} />
              <div className="terminal-main">
                <strong>{window.title || "(untitled)"}</strong>
                <span>
                  {window.kind} · pid {window.pid} · hwnd 0x{window.hwnd.toString(16)}
                </span>
                <code>{window.processPath || "(process path unavailable)"}</code>
              </div>
              <button className="icon-button" onClick={() => void focusWindow(window.hwnd)}>
                <Zap size={16} />
              </button>
            </article>
          ))
        )}
      </section>
    </main>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(<App />);
