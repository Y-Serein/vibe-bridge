# 任务：多窗口会话管理与 VT100 自适应渲染架构实现

## 一、项目背景

当前系统需要在单窗口终端界面的基础上，扩展为支持多窗口、多 session、多插件协同运行的终端交互系统。整体架构以 HID 免驱通信为基础，通过标准 HID 区域处理键盘输入，通过 HID 扩展区域传输屏幕控制、窗口状态、VT100 字节流和 session 管理信息。

系统目标是实现一个适用于嵌入式硬件的小型多窗口 TUI 运行环境，可在低资源平台上运行，例如 ARM A53 双核。系统应尽量避免大模型持续参与运行时交互，将高层语义到 VT100 序列的翻译、上下文状态维护、窗口路由等逻辑放在插件层完成。

---

## 二、核心目标

实现一套基于 session ID 的多窗口会话管理系统，支持：

1. 插件启动时自动申请唯一 session ID。
2. 所有后续按键事件、窗口渲染数据、状态同步数据均携带 session ID。
3. 主系统根据 session ID 将输入和输出准确路由到对应窗口。
4. 每个 session 独立维护上下文、窗口状态、渲染缓冲区和交互状态。
5. UI 渲染基于 VT100 字节流，支持 TUI 自适应显示。
6. 硬件端通过 HID 协议实现免驱通信，兼容 Windows / macOS / Linux。
7. 插件架构支持 Claude Code / OpenCode / codex CLI等其他上位机工具接入。
8. 支持 U 盘交付包，内置 skills、开发文档、插件和文件系统镜像。

---

## 三、系统架构要求

### 3.1 多窗口 session 管理

主系统需要实现统一的 session ID 管理机制。

要求：

- 每个插件启动时，必须向主系统请求一个唯一 session ID。
- session ID 由主系统统一分配，不允许插件自行生成。
- session ID 可使用 `uint8` 或 `uint16`，当前建议上限为 256 个。
- 每个 session 对应一个独立窗口。
- 每个窗口维护独立状态，包括：
  - 当前窗口标题
  - 当前插件类型
  - 当前 VT100 渲染缓冲区
  - 当前光标位置
  - 当前字体大小
  - 当前行高
  - 当前列宽
  - 当前滚动位置
  - 当前激活状态
  - 最近活动时间
  - 插件上下文状态

### 3.2 session 生命周期管理

需要实现 session 的创建、更新、失效和回收机制。

要求：

- 插件启动时调用 `request_session_id`。
- 主系统返回新的 session ID。
- 所有后续通信必须携带 session ID。
- 主系统定期检查 session 活跃状态。
- 当 session 长时间未使用，例如超过一个月，应自动销毁。
- 当 session 池达到上限，例如 256 个，应优先回收最久未使用的 session。
- session 被销毁后，插件端收到失效通知。
- 插件端收到失效通知后，需要重新请求新的 session ID。
- 如果插件继续使用失效 session ID，主系统应返回错误状态，例如 `SESSION_INVALID`。

建议实现状态码：

```c
SESSION_OK = 0x00
SESSION_CREATED = 0x01
SESSION_INVALID = 0x02
SESSION_EXPIRED = 0x03
SESSION_POOL_FULL = 0x04
SESSION_RECLAIMED = 0x05
```

------

## 四、HID 通信协议要求

### 4.1 HID 分区

系统基于 HID 免驱通信实现。

要求：

- 标准键盘输入使用标准 HID Keyboard 区域。
- 屏幕控制、窗口切换、session 管理、VT100 数据流使用 HID Vendor-defined 扩展区域。
- 通信应支持双向传输：
  - 上位机插件 → 硬件
  - 硬件 → 上位机插件

### 4.2 HID Report 设计

每个 HID report 需要携带以下基本字段：

```c
struct HidReport {
    uint8_t report_id;
    uint8_t command;
    uint16_t session_id;
    uint16_t payload_length;
    uint8_t payload[];
};
```

建议支持的 command 类型：

```c
CMD_REQUEST_SESSION       // 插件请求 session ID
CMD_SESSION_RESPONSE      // 主系统返回 session ID
CMD_SESSION_INVALID       // session 已失效
CMD_KEY_EVENT             // 按键事件
CMD_ENCODER_EVENT         // 旋钮事件
CMD_WINDOW_SWITCH         // 窗口切换
CMD_WINDOW_ACTIVATE       // 窗口主动激活
CMD_VT100_STREAM          // VT100 字节流渲染数据
CMD_UI_SCALE_CHANGE       // UI 缩放参数变化
CMD_STATUS_UPDATE         // 状态同步
CMD_FEEDBACK_EVENT        // 震动 / 声音 / LED 提示
CMD_ERROR                 // 错误信息
```

------

## 五、VT100 渲染要求

### 5.1 VT100 字节流渲染

UI 渲染基于 VT100 字节流。

要求：

- 插件层负责将高层 UI 描述翻译为 VT100 序列。
- 高层输入可以是 Markdown、JSON、YAML 或自定义 UI DSL。
- 主系统只负责接收 VT100 字节流，并渲染到对应 session 窗口。
- 渲染数据必须携带 session ID。
- 不同 session 的 VT100 流不能混淆。
- 每个 session 需要维护自己的终端缓冲区和光标状态。

### 5.2 UI 与通信框架解耦

要求：

- UI 描述逻辑不能和 HID 通信逻辑强绑定。
- HID 只负责传输。
- session 管理只负责路由。
- VT100 渲染模块只负责绘制。
- 插件层负责上下文维护和高层语义转换。

建议模块划分：

```text
/plugin
  session_client.py
  hid_transport.py
  vt100_renderer.py
  markdown_to_vt100.py
  skill_loader.py

/firmware_or_host
  session_manager.c
  hid_protocol.c
  window_manager.c
  vt100_terminal.c
  ui_scaler.c
  feedback_manager.c
```

------

## 六、自适应 UI 要求

系统需要支持不同屏幕尺寸、分辨率和窗口缩放场景下的自适应显示。

### 6.1 用户可调参数

用户可以通过旋钮调节：

- 字体大小
- 行高
- 列宽
- 窗口缩放比例
- 当前激活窗口
- 滚动位置

### 6.2 自适应规则

要求：

- 窗口尺寸变化时，重新计算可显示行数和列数。
- 字体大小变化时，重新计算字符网格。
- 行高变化时，重新计算垂直布局。
- 列宽变化时，重新计算横向布局。
- 滚动条位置要跟随内容高度同步变化。
- VT100 内容需要在新尺寸下重新渲染。
- 多窗口切换时，必须恢复该 session 上次的 UI 状态。
- 分辨率变化时，TUI 不应出现内容错位、重叠、截断或跨窗口污染。

建议维护如下窗口参数：

```json
{
  "session_id": 1,
  "window_title": "Claude Code",
  "font_size": 12,
  "line_height": 16,
  "column_width": 8,
  "screen_width": 480,
  "screen_height": 800,
  "visible_rows": 50,
  "visible_cols": 60,
  "scroll_offset": 0,
  "active": true
}
```

------

## 七、插件层要求

### 7.1 插件职责

插件负责：

- 启动时请求 session ID。
- 保存 session ID。
- 后续所有通信携带 session ID。
- 维护自身上下文。
- 将 Markdown / JSON / YAML / UI DSL 转换为 VT100。
- 将 VT100 字节流发送给硬件或主系统。
- 处理 session 失效通知。
- 失效后自动重新请求 session。
- 接收按键、旋钮、窗口切换事件。
- 根据事件更新自身状态。

### 7.2 上下文隔离

每个 session 必须有独立上下文。

要求：

- session A 的输入不能影响 session B。
- session A 的 VT100 缓冲区不能写入 session B。
- session A 的插件状态不能被 session B 读取。
- 插件需要维护 session 到 context 的映射。

示例：

```python
sessions = {
    1: {
        "plugin": "claude_code",
        "context": {},
        "vt100_buffer": [],
        "last_active": 1710000000
    },
    2: {
        "plugin": "open_code",
        "context": {},
        "vt100_buffer": [],
        "last_active": 1710000200
    }
}
```

------

## 八、skills 和 U 盘交付要求

系统需要支持 skills 功能模块挂载，并支持通过 U 盘分发。

### 8.1 skills 目录结构

建议结构：

```text
/skills
  /claude_code
    skill.yaml
    main.py
    README.md

  /open_code
    skill.yaml
    main.py
    README.md

  /terminal_demo
    skill.yaml
    main.py
    README.md

/docs
  architecture.md
  hid_protocol.md
  vt100_rendering.md
  session_management.md
  plugin_development.md

/bin
  plugin_host
  session_server
  hid_bridge

/images
  hid_fs.img
```

### 8.2 skill.yaml 示例

```yaml
name: claude_code
version: 0.1.0
entry: main.py
requires_session: true
transport: hid
render: vt100
description: Claude Code integration skill
```

### 8.3 U 盘交付包要求

U 盘交付包需要包含：

- 所有预置 skills
- 完整开发文档
- HID 协议说明
- VT100 渲染说明
- session ID 管理说明
- HID FS 文件系统镜像
- 可执行插件
- Linux 部署说明
- demo 程序
- 最小可运行示例

要求达到：

- 插入即可读取文档和 skills。
- 不依赖特定系统驱动。
- Windows / macOS / Linux 均可识别。
- 用户可以离线查看文档、修改 skills、部署插件。

------

## 九、APP Demo 要求

需要制作一个最小可运行 APP demo，用于展示交互效果。

Demo 至少包含：

1. 旋钮切换窗口。
2. 窗口主动激活。
3. 当前窗口高亮显示。
4. session ID 显示。
5. 插件名称显示。
6. VT100 内容区域。
7. 滚动条位置显示。
8. 字体大小 / 行高 / 列宽调节。
9. 震动反馈触发。
10. 声音提示触发。

Demo 场景：

```text
窗口 1：Claude Code
窗口 2：OpenCode
窗口 3：Terminal Demo
```

操作流程：

```text
旋钮左转：切换到上一个窗口
旋钮右转：切换到下一个窗口
旋钮按下：激活当前窗口
长按旋钮：进入 UI 缩放调节模式
按键事件：携带当前 session ID 发送到对应插件
插件返回：对应窗口刷新 VT100 内容
```

------

## 十、今日内优先完成事项

请按照以下优先级执行。

### P0：必须完成

#### 1. 基础插件框架

完成一个最小插件框架，支持：

- 插件启动。
- 请求 session ID。
- 接收主系统返回的 session ID。
- 保存 session ID。
- 后续通信自动携带 session ID。

验收标准：

```text
启动插件后，终端打印：
requesting session id...
session created: 1
```

#### 2. session ID 分配机制

实现 session manager，支持：

- 分配唯一 session ID。
- 保存 session 状态。
- 根据 session ID 查询 session。
- 更新 session 最近活动时间。
- 返回 session 创建结果。

验收标准：

```text
插件 A 启动，获得 session_id = 1
插件 B 启动，获得 session_id = 2
两个插件的输入输出互不影响
```

#### 3. session ID 失效通知机制

实现：

- 超时回收。
- 池满回收。
- 失效通知。
- 插件收到失效后重新请求。

验收标准：

```text
当 session 被回收后，继续使用旧 session_id 会收到 SESSION_INVALID
插件收到 SESSION_INVALID 后自动重新请求 session_id
```

#### 4. Linux 环境部署验证

验证插件可以在 Linux 下运行，并可被 Claude Code / OpenCode 或外部调用方加载。

验收标准：

```text
Linux 下可以启动插件
可以请求 session ID
可以发送 HID / 模拟 HID report
可以收到 session response
```

#### 5. session 全流程联调

完成以下链路：

```text
Claude Code 启动
→ 插件请求 session ID
→ session manager 分配并返回 session ID
→ 后续按键事件携带 session ID
→ 服务端按 session ID 路由
→ 对应窗口激活
→ VT100 内容刷新到对应窗口
```

验收标准：

```text
每个按键事件都带 session_id
每个 VT100 输出都能正确进入对应窗口
窗口切换后不会串 session
```

------

### P1：今日尽量完成

#### 6. 多窗口自适应 UI 验证

实现窗口缩放、自适应渲染验证。

需要验证：

- 字体大小调整
- 行高调整
- 列宽调整
- 屏幕分辨率变化
- 窗口尺寸变化
- 滚动条位置同步
- 震动反馈同步
- 声音提示同步

验收标准：

```text
窗口尺寸变化后，TUI 内容重新布局
不同 session 保留自己的 UI 缩放参数
切换窗口后 UI 状态能恢复
```

#### 7. APP Demo

制作一个可操作 demo，展示：

- 旋钮切换窗口
- 窗口主动激活
- 震动提示
- 声音提示
- 多 session 显示
- VT100 内容刷新

验收标准：

```text
可以看到至少 3 个 session 窗口
旋钮可以切换窗口
当前窗口有明显激活态
触发操作时有声音或震动状态反馈
```

#### 8. 架构文档

输出完整 Markdown 文档。

至少包含：

- 总体架构
- session ID 管理机制
- session 生命周期
- HID report 协议
- VT100 渲染流程
- 插件上下文隔离方式
- Markdown / JSON / YAML 到 VT100 的翻译逻辑
- skills 挂载规范
- U 盘交付包结构
- Linux 部署方式
- demo 运行方式

输出文件：

```text
docs/architecture.md
docs/session_management.md
docs/hid_protocol.md
docs/vt100_rendering.md
docs/plugin_development.md
docs/usb_delivery_package.md
```

#### 9. U 盘交付包准备

整理交付包目录。

要求：

```text
release_usb_package/
  skills/
  docs/
  bin/
  images/
  README.md
```

验收标准：

```text
release_usb_package 可以直接复制到 U 盘
用户打开 README.md 后能知道如何运行 demo 和开发 skill
```

------

## 十一、建议优先实现的最小可运行版本

如果时间有限，先实现以下 MVP：

```text
1. Python 版 session_manager
2. Python 版 plugin_client
3. 模拟 HID report 通信
4. 三个模拟插件窗口
5. 每个窗口独立 session_id
6. 每个窗口可以接收 VT100 字符流
7. 旋钮事件用键盘按键模拟
8. session 失效后自动重新申请
9. 输出 architecture.md
```

MVP 目录建议：

```text
vibe_session_demo/
  main.py
  session_manager.py
  plugin_client.py
  hid_protocol.py
  vt100_renderer.py
  window_manager.py
  demo_plugins/
    claude_code.py
    open_code.py
    terminal_demo.py
  docs/
    architecture.md
  README.md
```

------

## 十二、最终交付物

请最终交付以下内容：

```text
1. 可运行的插件 demo
2. session manager 源码
3. HID report 协议定义
4. VT100 渲染 demo
5. 多窗口切换 demo
6. session 失效与重连 demo
7. Linux 运行说明
8. 完整架构文档
9. skills 挂载规范
10. U 盘交付包目录
```

------

## 十三、重点注意事项

1. 不要把 UI 渲染逻辑和 HID 通信逻辑写死在一起。
2. 不要让插件自己生成 session ID。
3. 所有输入输出必须携带 session ID。
4. 每个 session 必须上下文隔离。
5. VT100 字节流必须按 session 路由。
6. session 失效后必须能重新申请。
7. 优先完成 Linux 下可运行的 MVP。
8. 先做模拟 HID，再对接真实 HID。
9. 文档和代码要同步输出。
10. Demo 必须能证明多窗口不会串流。

```
可以再加一句给 AI 的执行指令：

```md
请直接开始实现，不要只写方案。优先完成 MVP 代码、运行说明和架构文档。所有代码要求可复制运行，遇到真实 HID 暂不可用时，先用模拟 HID report 完成全流程验证。
```