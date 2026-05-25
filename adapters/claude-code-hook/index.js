#!/usr/bin/env node
// vibe-bridge Claude Code hook adapter.
//
// 安装方式见 ./README.md。该脚本被 Claude Code 在 PreToolUse / UserPromptSubmit
// / SessionStart / Stop / SessionEnd 等事件时调用,通过 TCP 上报给本机 vb-daemon。
//
// 环境变量:
//   VIBE_BRIDGE_HOST       默认 127.0.0.1
//   VIBE_BRIDGE_PORT       默认 8765
//   VIBE_BRIDGE_HOOK_NAME  事件名 (也可以用 argv[2])
//
// daemon 不在线或拒绝时静默继续,绝不阻塞 Claude Code。

const net = require('node:net');
const fs = require('node:fs');
const path = require('node:path');
const { stdin, stdout, stderr, env, argv, exit } = require('node:process');

const HOST = env.VIBE_BRIDGE_HOST || '127.0.0.1';
const PORT = parseInt(env.VIBE_BRIDGE_PORT || '8765', 10);
const KIND = 'claude';
const SEND_TIMEOUT_MS = 1500;
const PERMISSION_POLL_INTERVAL_MS = parseInt(env.VIBE_BRIDGE_PERMISSION_POLL_MS || '250', 10);
const PERMISSION_TIMEOUT_MS = parseInt(env.VIBE_BRIDGE_PERMISSION_TIMEOUT_MS || '300000', 10);

function readJsonStdin() {
  let raw = '';
  try {
    const buf = fs.readFileSync(0);
    raw = decodeStdinBuffer(buf);
    if (env.VIBE_BRIDGE_DEBUG === '1') {
      stderr.write(`[vibe-bridge hook debug] stdin isTTY=${stdin.isTTY} bytes=${raw.length}\n`);
    }
    return JSON.parse(stripJsonText(raw) || '{}');
  } catch (err) {
    if (env.VIBE_BRIDGE_DEBUG === '1') {
      stderr.write(`[vibe-bridge hook debug] stdin parse failed: ${err.message}; raw=${JSON.stringify(raw.slice(0, 200))}\n`);
    }
    return {};
  }
}

function decodeStdinBuffer(buf) {
  if (buf.length >= 2 && buf[0] === 0xff && buf[1] === 0xfe) {
    return buf.subarray(2).toString('utf16le');
  }
  if (buf.length >= 4 && buf[0] === 0xff && buf[1] === 0xfe && buf[2] === 0x00 && buf[3] === 0x00) {
    return buf.subarray(4).toString('utf16le');
  }
  let raw = buf.toString('utf8');
  const nulCount = [...raw].filter((ch) => ch === '\u0000').length;
  if (nulCount > raw.length / 4) {
    raw = buf.toString('utf16le');
  }
  return raw;
}

function stripJsonText(raw) {
  return raw.replace(/^\uFEFF/, '').replace(/\u0000/g, '').trim();
}

function sendDaemon(msg) {
  return new Promise((resolve, reject) => {
    const sock = net.connect({ host: HOST, port: PORT });
    let resp = '';
    let settled = false;
    const settle = (fn, value) => {
      if (!settled) {
        settled = true;
        sock.destroy();
        fn(value);
      }
    };

    sock.setEncoding('utf-8');
    sock.on('connect', () => sock.write(JSON.stringify(msg) + '\n'));
    sock.on('data', (chunk) => {
      resp += chunk;
      const lineEnd = resp.indexOf('\n');
      if (lineEnd >= 0) {
        settle(resolve, parseJsonSafe(resp.slice(0, lineEnd).trim()));
      }
    });
    sock.on('end', () => settle(resolve, parseJsonSafe(resp.trim())));
    sock.on('error', (err) => settle(reject, err));
    setTimeout(() => {
      sock.destroy();
      settle(reject, new Error('daemon send timeout'));
    }, SEND_TIMEOUT_MS);
  });
}

function parseJsonSafe(text) {
  try { return JSON.parse(text); } catch { return { raw: text }; }
}

function resolveAgentId(input) {
  // If Claude was launched by `vb-daemon agent-shim`, prefer the launch id so
  // live ConPTY replay and hook-driven permissions bind to the same board sid.
  // MUST match the agent_id format used by `vb-host`'s passive transcript
  // scan, otherwise hook-driven `permission.request` and the board SID
  // allocated from `register_discovered_sessions` end up on two different
  // RegisteredAgent entries — daemon sees two SIDs for one claude, and the
  // CONFIRM/REJECT path never reaches the hook's poll loop.
  //
  // Passive scan derives agent_id from the jsonl filename stem
  // (`<UUID>.jsonl` → `<UUID>`), which is the same string Claude Code
  // exposes as `input.session_id` in the hook stdin payload. So we use it
  // raw, no `claude-` prefix.
  return env.VIBE_BRIDGE_LAUNCH_AGENT_ID ||
    input.session_id ||
    env.CLAUDE_SESSION_ID ||
    `claude-pid-${process.pid}`;
}

function firstDefined(...values) {
  return values.find((value) => value !== undefined && value !== null);
}

async function sendChecked(msg) {
  if (env.VIBE_BRIDGE_DEBUG === '1') {
    stderr.write(`[vibe-bridge hook debug] send ${JSON.stringify(msg)}\n`);
  }
  const resp = await sendDaemon(msg);
  if (env.VIBE_BRIDGE_DEBUG === '1') {
    stderr.write(`[vibe-bridge hook debug] recv ${JSON.stringify(resp)}\n`);
  }
  if (resp && resp.ok === false) {
    throw new Error(resp.error || 'daemon rejected message');
  }
  return resp;
}

async function registerAgent(agentId, cwd) {
  return sendChecked({
    type: 'agent.register',
    agent: {
      agentId,
      kind: KIND,
      name: path.basename(cwd),
      cwd,
    },
  });
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function preToolDecision(decision, reason) {
  return {
    hookSpecificOutput: {
      hookEventName: 'PreToolUse',
      permissionDecision: decision,
      permissionDecisionReason: reason,
    },
    suppressOutput: true,
  };
}

async function pollPermission(agentId, reqId) {
  const deadline = Date.now() + PERMISSION_TIMEOUT_MS;
  while (Date.now() < deadline) {
    const resp = await sendChecked({
      type: 'permission.poll',
      poll: {
        agentId,
        kind: KIND,
        reqId,
      },
    });
    if (resp && resp.type === 'permission.decision') {
      return resp.decision;
    }
    if (resp && resp.type === 'permission.pending' && resp.pending === false) {
      return 'ask';
    }
    await sleep(PERMISSION_POLL_INTERVAL_MS);
  }
  return 'ask';
}

async function main() {
  const hookName = argv[2] || env.VIBE_BRIDGE_HOOK_NAME || 'unknown';
  const input = await readJsonStdin();
  const agentId = resolveAgentId(input);
  const cwd = input.cwd || process.cwd();

  try {
    switch (hookName) {
      case 'SessionStart':
        await registerAgent(agentId, cwd);
        break;

      case 'UserPromptSubmit':
        await registerAgent(agentId, cwd);
        await sendChecked({
          type: 'turn.append',
          turn: {
            agentId,
            kind: KIND,
            role: 'user',
            text: String(input.prompt || '').slice(0, 4000),
            tsMs: Date.now(),
          },
        });
        break;

      case 'PreToolUse': {
        const reqId = Date.now();
        const toolName = firstDefined(
          input.tool_name,
          input.toolName,
          input.tool,
          input.name,
          'unknown',
        );
        const toolInput = firstDefined(
          input.tool_input,
          input.toolInput,
          input.input,
          input.args,
          {},
        );
        await registerAgent(agentId, cwd);
        await sendChecked({
          type: 'permission.request',
          permission: {
            agentId,
            kind: KIND,
            reqId,
            tool: String(toolName || 'unknown'),
            argsSummary: JSON.stringify(toolInput || {}).slice(0, 80),
          },
        });
        const decision = await pollPermission(agentId, reqId);
        const reason = decision === 'ask'
          ? 'AIKB board did not return a permission decision before timeout.'
          : `AIKB board permission decision: ${decision}`;
        stdout.write(JSON.stringify(preToolDecision(
          decision === 'deny' ? 'deny' : decision === 'always' ? 'allow' : decision,
          reason,
        )));
        return;
      }

      case 'PostToolUse':
        await registerAgent(agentId, cwd);
        await sendChecked({
          type: 'agent.activity',
          activity: {
            agentId,
            kind: KIND,
            activity: 'tool-activity',
            status: 'running',
          },
        });
        break;

      case 'Stop':
        await registerAgent(agentId, cwd);
        await sendChecked({
          type: 'agent.activity',
          activity: {
            agentId,
            kind: KIND,
            activity: 'completed',
            status: 'idle',
          },
        });
        break;

      case 'SessionEnd':
        await sendChecked({
          type: 'session.abort',
          abort: { agentId, kind: KIND },
        });
        break;

      default:
        // 未识别事件 — 静默 (允许 user 把任何 hook 都接到这里)
        break;
    }
  } catch (err) {
    stderr.write(`[vibe-bridge hook ${hookName}] ${err.message}\n`);
  }

  stdout.write(JSON.stringify({ continue: true }));
}

main().catch((err) => {
  stderr.write(`[vibe-bridge hook fatal] ${err.message}\n`);
  // 不要让 hook 失败阻塞 Claude Code
  exit(0);
});
