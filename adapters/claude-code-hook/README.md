# vibe-bridge Claude Code hook adapter

让 Claude Code 在 PreToolUse / UserPromptSubmit / SessionStart / Stop / SessionEnd
等关键事件上报给本机 `vb-daemon`,daemon 路由到 AIKB 板端显示 session /
token / pending permission。

## 设计

- 单一 Node.js 脚本 (`index.js`),零 npm 依赖,只用 Node 标准库 `net` `path`。
- 通过 TCP `127.0.0.1:8765` 与 `vb-daemon` 通信 (JSON-per-line)。
- daemon 不在线 / 超时,静默继续,绝不阻塞 Claude Code。
- 事件名靠 `argv[2]` 区分 (或 `VIBE_BRIDGE_HOOK_NAME` env 兜底)。

## 安装

1. 启动 `vb-daemon`:

   ```powershell
   # Windows native
   cd $env:USERPROFILE\Sipeed\rv_nano\tools\vibe-bridge
   cargo run -p vb-daemon -- snapshot   # 测试 daemon 能起
   ```

2. 写入 Claude Code hook 配置 `~/.claude/settings.json`:

   ```json
   {
     "hooks": {
       "SessionStart": [
         { "matcher": "*", "hooks": [
           { "type": "command",
             "command": "node /absolute/path/to/vibe-bridge/adapters/claude-code-hook/index.js SessionStart" }
         ]}
       ],
       "UserPromptSubmit": [
         { "matcher": "*", "hooks": [
           { "type": "command",
             "command": "node /absolute/path/to/vibe-bridge/adapters/claude-code-hook/index.js UserPromptSubmit" }
         ]}
       ],
       "PreToolUse": [
         { "matcher": "*", "hooks": [
           { "type": "command",
             "command": "node /absolute/path/to/vibe-bridge/adapters/claude-code-hook/index.js PreToolUse" }
         ]}
       ],
       "PostToolUse": [
         { "matcher": "*", "hooks": [
           { "type": "command",
             "command": "node /absolute/path/to/vibe-bridge/adapters/claude-code-hook/index.js PostToolUse" }
         ]}
       ],
       "Stop": [
         { "matcher": "*", "hooks": [
           { "type": "command",
             "command": "node /absolute/path/to/vibe-bridge/adapters/claude-code-hook/index.js Stop" }
         ]}
       ],
       "SessionEnd": [
         { "matcher": "*", "hooks": [
           { "type": "command",
             "command": "node /absolute/path/to/vibe-bridge/adapters/claude-code-hook/index.js SessionEnd" }
         ]}
       ]
     }
   }
   ```

3. 起一个 Claude Code session 验证 daemon snapshot 出现 `kind=claude`:

   ```powershell
   cargo run -p vb-daemon -- snapshot
   ```

## M4 权限闭环

- `PreToolUse` 会先上报 `permission.request`,然后轮询 daemon 的
  `permission.poll`。板端 picker 对该 permission 按 `CONFIRM` / encoder push
  会返回 allow,按 `REJECT` 会返回 deny。
- 如果 daemon 不在线或上报失败,hook 会继续放行,避免阻塞 Claude Code。
- 如果 daemon 在线但超时未收到板端决定,hook 返回 `permissionDecision=ask`,
  交回 Claude Code 原生权限确认。
- token 用量 hook 拿不到 (Claude Code hook 上下文不带 usage),后续要单独
  起一个 transcript watcher 解析 `~/.claude/projects/*/jsonl` 上报。

## 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `VIBE_BRIDGE_HOST` | `127.0.0.1` | daemon TCP host |
| `VIBE_BRIDGE_PORT` | `8765` | daemon TCP port |
| `VIBE_BRIDGE_HOOK_NAME` | (无) | 替代 `argv[2]` 指定事件名 |
| `VIBE_BRIDGE_PERMISSION_POLL_MS` | `250` | `PreToolUse` 等待板端决定时的轮询间隔 |
| `VIBE_BRIDGE_PERMISSION_TIMEOUT_MS` | `300000` | 超时后返回 Claude Code 原生 `ask` |
