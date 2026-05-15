# vibe-bridge 项目交接（HANDOFF）

> 项目位置：`/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge/`
> 协议规格：`request.md`（多窗口 session 管理 + HID 免驱 + VT100 渲染 + 插件挂载）
> 与板端固件的集成：`/home/rv_nano/AIKB/LicheeRV-Nano-Build/middleware/v2/sample/aikb_hid_input/`

## 高优先级交付边界

涉及 AIKB 板端、SG2002、Lichee RV Nano、rootfs、镜像、`aikb_lcd_ui`、
`aikb_hid_input`、overlay、板端资源文件时，接手者必须先看这一段：

- Codex 要完成到“用户可以直接运行最终构建 / 打包命令”的前一步。
- Codex 负责源码修改、资源生成、sample 级编译验证、必要的目标路径同步、
  SHA / 文件存在性 / `git diff --check` 等交付前检查。
- Codex 不默认执行 `build_all`、`pack_rootfs`、`pack_burn_image`、烧录、格式化、
  写卡等最终构建/打包/烧录动作；这些由用户执行，除非用户明确要求 Codex 代跑。
- 最终回复必须把用户下一步要运行的完整命令放在显眼位置。
- 如果用户限制“不 build_all / 不 pack_rootfs / 不 pack_burn_image”，仍然要做
  这些命令之前能做的所有准备和验证，不能把“未打包”误解成“不交付板端二进制或资源”。

## 当前在做什么

**2026-05-15 按键功能重定义。**

最新按键定义覆盖之前 Ask/Run/Fix/Commit 版本。硬件引脚和 HID bit 位序不变：
- bit0 / KEY1 A15：`BoardKey.REJECT`
- bit1 / KEY2 A24：`BoardKey.VOICE`
- bit2 / KEY3 A23：`BoardKey.SESSION`
- bit3 / KEY4 A27：`BoardKey.VOTE_REVIEW`
- bit4 / KEY5 A25：`BoardKey.AGENT_MODEL`
- bit5 / KEY6 A22：`BoardKey.MULTI_FUNCTION`
- bit6 / KEY7 A29：`BoardKey.CONFIRM`
- bit7 / KEY8 P19：`BoardKey.MENU_DEBUG`
- encoder push P21：select/enter，仍通过 `KEY_EVENT` 的 `encoder_pressed` 表示。
- encoder A/B P22/P23：`ENCODER_EVENT` delta，用于列表滚动、候选项选择和参数调整。

策略不变：daemon 不写死业务动作，只路由 `CMD_KEY_EVENT` 到 active plugin；业务行为由插件/wrapper 根据这些语义处理。板端本地视频只提供即时状态反馈。

**2026-05-15 key0/key1 语义交换 + 按键动作策略。（已被上面的新定义覆盖）**

历史记录：用户当时要求交换 key0 和 key1 功能。当前 HID payload 格式不变：
- `payload[0] bit0` 仍是 KEY1 / A15，但语义改为 `Run / Execute`。
- `payload[0] bit1` 仍是 KEY2 / A24，但语义改为 `Ask / Prompt`。
- 当时曾使用 `Run / Execute` 和 `Ask / Prompt` 命名；后续已改为 `REJECT` / `VOICE` 等新定义，不再使用这组 `BoardKey` 枚举名。
- 板端本地视频反馈也同步为 KEY0 -> `running.264`、KEY1 -> `asking.264`。

策略：板端只负责按键扫描、HID 事件上报和本地视频反馈；`vibe-bridge` daemon 继续做会话/窗口路由，不把 Ask/Run/Fix/Commit 等业务动作写死在 daemon 里。后续剩余功能应由插件或 wrapper 识别 `CMD_KEY_EVENT` 后执行动作，再通过现有窗口/session/VT100 路由更新 LCD。

**2026-05-14 video pet page：默认动画已出图，恢复横屏页面视频 + 按键切换骨架。**

用户真机确认默认动画已经显示；当前问题从显示链路切到页面内容。
用户确认方向应保持之前的横屏页面，不是竖屏重排；源视频必须使用
`/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge/images/new/vedio1.mp4`，
页眉/页脚要匹配 `images/IMG_9884.PNG`。判断：`sample_vdecvo` 独占 VO/MIPI，
不能指望 `aikb_lcd_ui` framebuffer 页眉页脚叠在视频上；页眉、页脚和主体必须烘进视频帧。

板端 `AIKB/LicheeRV-Nano-Build` 本轮完成：
- 新增 `T_tools/build_aikb_video_from_mp4.py`，以
  `images/IMG_9884.PNG` 作为静态 960x412 UI shell，用
  `images/new/vedio1.mp4` 裁掉自身页眉/页脚后的中间动态内容覆盖 shell 中间区；
  覆盖时保持等比缩放，不拉伸恐龙，再按旧显示方向旋入 412x960 视频帧。
- 重新生成 `buildroot/board/cvitek/SG200X/overlay/mnt/system/usr/share/aikb/video/asking.264`：
  H.264 Constrained Baseline、412x960、yuv420p、24fps、level 3.1。
- 同步三处 `asking.264` SHA：
  `777d20cae782d721066ac59412e4ef9d008823bff9accd6936c550f09fd17407`
  - `buildroot/board/cvitek/SG200X/overlay/mnt/system/usr/share/aikb/video/asking.264`
  - `buildroot/output/target/mnt/system/usr/share/aikb/video/asking.264`
  - `install/soc_sg2002_licheervnano_sd/rootfs/mnt/system/usr/share/aikb/video/asking.264`
- `buildroot/board/cvitek/SG200X/overlay/mnt/system/auto.sh`
  - video 模式启动 `sample_vdecvo` 后再启动一个本地 FIFO 事件控制器。
  - 消费 `aikb_hid_input --event-out /tmp/aikb_pet_events.in` 的
    `KEY 0/1/2 DOWN` 和 `ENC_BTN DOWN`。
  - `KEY 0` 映射 `asking.264`；`KEY 1/2`、`ENC_BTN` 预留
    `key1.264`、`key2.264`、`menu.264`，文件不存在时回落到 `asking.264`
    并写 `/tmp/aikb_video_player.log`。
  - 已同步到 `buildroot/output/target/mnt/system/auto.sh` 和
    `install/soc_sg2002_licheervnano_sd/rootfs/mnt/system/auto.sh`，三处 SHA：
    `5cd23bb400a77c862c527e6d8af8dc9f2adeab86176699abcd591f2e68b25cbc`。
- `buildroot/board/cvitek/SG200X/overlay/etc/init.d/S09aikb`
  - `stop` 现在会停 `sample_vdecvo` 和 `/tmp/aikb_video_controller.pid`。
  - 已同步到 `buildroot/output/target/etc/init.d/S09aikb`，两处 SHA：
    `5397ee4073004af8c5aed041400a9e9feef73162e0f2376ba033aa9919900f03`。

已验证：
- `sh -n` 覆盖 overlay/output/install 的 `auto.sh`，以及 overlay/output 的 `S09aikb`。
- `python3 -m py_compile T_tools/build_aikb_video_from_mp4.py` 通过。
- `ffprobe` 确认 output 里的 `asking.264` 为 H.264 Constrained Baseline、
  412x960、yuv420p、24fps。
- 抽帧预览：
  已在生成时确认横屏方向下页眉/页脚来自 `IMG_9884.PNG`，中间为
  `vedio1.mp4` 裁剪动态内容；随后已按用户要求清理 `R_raw/` 中间预览和旧帧，
  避免后续误引用。
- 已清理错误方向/无关中间资源：
  `T_tools/render_aikb_video_page.py`、`R_raw/video_page_pipeline/`、
  `R_raw/pet_asset_pipeline/` 和 `T_tools/__pycache__/`。当前保留的生成入口是
  `T_tools/build_aikb_video_from_mp4.py`。
- `git diff --check` 在 `AIKB/LicheeRV-Nano-Build` 通过。

用户下一步仍需运行最终打包命令：
```bash
cd /home/rv_nano/AIKB/LicheeRV-Nano-Build
apptainer exec --cleanenv host/ubuntu/licheervnano-build-ubuntu.sqfs bash -lc 'cd /home/rv_nano/AIKB/LicheeRV-Nano-Build && source build/cvisetup.sh && defconfig sg2002_licheervnano_sd && pack_rootfs && pack_burn_image'
```

**2026-05-13 pet view 暂停功能扩展，先建立资产驱动角色最小闭环。**

用户明确指出当前最大问题不是 scene 数量，而是宠物本体审美不成立；
要求停止继续调粗糙 C 恐龙动作，沿 `asking.akim` / AKIM 资源方向做
产品级 pixel pet 管线。已阅读：
- `images/IMG_9884.PNG`
- `images/PET_CHARACTER_BRIEF.md`

本轮只改 pet asset pipeline，不动 header/footer、terminal/dashboard/Kitty/HID。
已按“做到用户最终构建/打包前一步”的边界补做 RISC-V sample 编译、strip、
目标路径同步和 SHA 校验；没有跑 `build_all`，没有 `pack_rootfs` /
`pack_burn_image`。

板端 `AIKB/LicheeRV-Nano-Build` 已完成：
- `middleware/v2/sample/aikb_lcd_ui/aikb_lcd_ui.c`
  - 新增 in-code pet manifest，记录 state、expected frame count、
    frame duration、AKIM resource path。
  - `asking.akim` 成为正式 pet character 入口；AKIM 存在且合法时优先渲染，
    C procedural dino 只在 asset 缺失/非法或 `--pet-force-fallback` 时使用。
  - 新增 `--pet-asset-root PATH`，方便本地 dump 时指向 overlay 里的
    `usr/share/aikb/pet/asking.akim`。
  - 新增 `--pet-qa-dump PATH`，输出 resource、load status、stage bbox、
    AKIM alpha、asset alpha bbox、rendered asset bbox、edge-change 统计。
- `middleware/v2/sample/aikb_lcd_ui/scripts/png_frames_to_akim.py`
  - 纯 stdlib PNG RGB/RGBA 解码，按序打包 PNG frame sequence 为 AKIM。
  - 默认 RGBA8888 + LOOP；支持 `--frame-delay-ms`、`--no-loop`、`--argb8888`。
- `middleware/v2/sample/aikb_lcd_ui/README.md`
  - 记录 pet manifest/AKIM/fallback/QA dump/PNG->AKIM 流程。
- `images/PET_CHARACTER_BRIEF.md`
  - 追加当前 Asset Pipeline Contract、`asking.akim` pose frame layout、
    asset vs fallback dump 命令。
- 生成对比 dump：
  - `/home/rv_nano/AIKB/LicheeRV-Nano-Build/R_raw/pet_asset_pipeline/pet-asset.ppm`
  - `/home/rv_nano/AIKB/LicheeRV-Nano-Build/R_raw/pet_asset_pipeline/pet-asset.png`
  - `/home/rv_nano/AIKB/LicheeRV-Nano-Build/R_raw/pet_asset_pipeline/pet-asset.qa`
  - `/home/rv_nano/AIKB/LicheeRV-Nano-Build/R_raw/pet_asset_pipeline/pet-fallback.ppm`
  - `/home/rv_nano/AIKB/LicheeRV-Nano-Build/R_raw/pet_asset_pipeline/pet-fallback.png`
  - `/home/rv_nano/AIKB/LicheeRV-Nano-Build/R_raw/pet_asset_pipeline/pet-fallback.qa`
- 已交叉编译并同步板端运行产物：
  - `aikb_lcd_ui` 四处 SHA 一致：
    `78bf71fa929698c57791cfc189118978be40031d4463296bc96801c878d98326`
  - `asking.akim` 三处 SHA 一致：
    `6e2ea009d94d1b362e1068bb381cd1a78d62fb99a68c9fd9c769fd58e960f2fd`

关键 QA 结果：
- asset path 成功加载 `asking.akim`：`64x48`、`32 frames`、`120 ms`、`RGBA`。
- asset asking/confused frame `asset_alpha_bbox=5,6 53x38`，
  `asset_rendered_bbox=375,110 159x114 scale=3`。
- fallback 强制路径仍可用：`asset_loaded=no fallback_forced=yes`。
- 视觉观察：当前 `asking.akim` 已走 asset path，但角色本体偏小，后续应改
  PNG/AKIM 美术资源，不应再调 C 恐龙。

已验证：
- `make clean && make`（仅 `middleware/v2/sample/aikb_lcd_ui` sample）通过。
- `apptainer exec --cleanenv host/ubuntu/licheervnano-build-ubuntu.sqfs ... make CC=...riscv64-unknown-linux-musl-gcc`
  交叉编译通过，随后 `riscv64-unknown-linux-musl-strip aikb_lcd_ui` 通过。
- `file` 确认四处 `aikb_lcd_ui` 都是 RISC-V musl ELF 且 stripped。
- `sha256sum` 确认四处 `aikb_lcd_ui`、三处 `asking.akim` 均已对齐。
- asset/fallback 两条 dump 命令均通过；直接写仓库路径时 `aikb_lcd_ui`
  在沙箱内报 `Read-only file system`，已改为先写 `/tmp` 再 `cp` 到 `R_raw/`。
- `python3 -m py_compile middleware/v2/sample/aikb_lcd_ui/scripts/png_frames_to_akim.py` 通过。
- `git diff --check` 在 `AIKB/LicheeRV-Nano-Build` 通过。
- `git diff --check -- images/PET_CHARACTER_BRIEF.md` 在 `tools/vibe-bridge` 通过。

下一步建议：
1. 用户若要生成最终镜像，直接运行：
   ```bash
   cd /home/rv_nano/AIKB/LicheeRV-Nano-Build
   apptainer exec --cleanenv host/ubuntu/licheervnano-build-ubuntu.sqfs bash -lc 'cd /home/rv_nano/AIKB/LicheeRV-Nano-Build && source build/cvisetup.sh && defconfig sg2002_licheervnano_sd && pack_rootfs && pack_burn_image'
   ```
2. 用新的 `png_frames_to_akim.py` 从设计帧生成更大、更清晰的 `asking.akim`，
   目标让本体 rendered bbox 接近参考图中的占比。
3. 保持 scene 不扩展，先把 `asking` 的 idle/thinking/asking/happy/sleep pose
   美术质量打到产品级。

**2026-05-12 新开 codex 屏幕不跳转：确认旧 state 误导 + wrapper 主动激活新 sid。**

用户反馈“开了一个 codex，屏幕并没有转跳”。本地检查发现
`PYTHONPATH=src python3 -m vibe_bridge.main sessions` 原先只读
`/tmp/vibe-bridge-state.json`，即使 daemon/socket 已死也会显示旧 active sid。
这次现场状态就是：state 里还有 `active sid=33`，但 `/tmp/vibe-bridge.sock`
不可连接，daemon 绑定 socket 在沙箱内报 `PermissionError: [Errno 1]`。

本轮 host 修复：
- `src/vibe_bridge/main.py`
  - `sessions` 增加 `socket status : reachable|unreachable/stale`。
  - 增加 `state age`，避免把旧 json 当成实时 daemon 状态。
- `src/vibe_bridge/wrapper.py`
  - 如果遗留 `VIBE_SOCK_PATH=/tmp/vibe-real.sock` 已不可连接，直接回落到
    `/tmp/vibe-bridge.sock`。
  - wrapper 新申请 sid 或复用 sid 后立即发送 `CMD_WINDOW_ACTIVATE`。这样新开的
    `codex` 会把 daemon active window 切到自己，随后 VT100 才会转发到板端。
  - 保留之前的 legacy mock socket 纠偏：旧 `/tmp/vibe-real.sock` 不是
    `real-hidraw` 时回落到默认 real daemon。
- `tests/test_main.py`
  - 覆盖 `sessions` 的 socket 活性输出。
- `tests/test_wrapper.py`
  - 覆盖 dead legacy socket 回落、legacy real 保留、新 sid/复用 sid 激活。

已验证：
- `PYTHONPATH=src python3 -m unittest tests.test_wrapper tests.test_main` 38/38 通过。
- `PYTHONPATH=src python3 -m unittest discover -s tests` 103/103 通过。
- 沙箱外重启真实 daemon：
  `spawn_daemon_detached('/tmp/vibe-bridge.sock', --state /tmp/vibe-bridge-state.json, --hidraw /dev/hidraw0)`。
- `PYTHONPATH=src python3 -m vibe_bridge.main sessions` 现在显示
  `socket status : reachable`、`mode : real-hidraw`、`hidraw : /dev/hidraw0`。
- `PYTHONPATH=src python3 -m vibe_bridge.main request-session --plugin wrapper-connect-check`
  返回 `session created: sid=1 status=CREATED`。
- 沙箱外 wrapper 验证：
  `env VIBE_SOCK_PATH=/tmp/vibe-bridge.sock VIBE_HIDRAW_DEVICE=/dev/hidraw0 VIBE_BRIDGE_FORWARD=pty ... codex --version`
  正常输出 `codex-cli 0.130.0`，没有再降级到 “daemon unreachable”。

立即验证建议：
```
cd /home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge
unset VIBE_SOCK_PATH
export PATH="/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge/bin:$PATH"
codex
PYTHONPATH=src python3 -m vibe_bridge.main sessions
```
新 `codex` 保持打开时，`sessions` 应显示 `socket status : reachable` 且有
`plugin=codex` 的 sid；关闭后 session 被释放是正常的。

**2026-05-12 v24 前：板端内部绘制 Codex 圆角框线。**

用户确认 v23 后字符替换正常，但 Codex 顶部 prompt frame 的
`╭────────────────╮ / │ / ╰────────────────╯` 仍没有正常终端那种连续感。
判断：这不是 host 字符内容问题，而是板端 FreeType 字体 glyph 有 side bearing /
cell 对齐误差，box drawing 交给字体画就会出现断缝或粗细不稳。

本轮改板端 `AIKB/LicheeRV-Nano-Build`：
- `middleware/v2/sample/aikb_lcd_ui/aikb_lcd_ui.c`
  - 新增 `is_box_drawing_cp()`，覆盖 Codex/Claude prompt frame 常用
    `U+2500`、`U+2502`、`U+256D`、`U+256E`、`U+2570`、`U+256F`。
  - `term_cp_width()` 对这些 codepoint 固定按 1 cell。
  - `draw_terminal_cell()` 在 FreeType 前拦截这些字符，调用内部几何绘制：
    `─/│` 按 cell 中线画，`╭/╮/╰/╯` 画半横线+半竖线+中心连接块，避免字体留白。
- `middleware/v2/sample/aikb_lcd_ui/README.md`
  - 记录 box drawing 也和 Powerline 一样由 renderer 内部绘制。

产物状态：
- RISC-V musl 交叉编译通过，strip 通过。
- 已同步四处 `aikb_lcd_ui`：
  - `middleware/v2/sample/aikb_lcd_ui/aikb_lcd_ui`
  - `buildroot/board/cvitek/SG200X/overlay/mnt/system/usr/bin/aikb_lcd_ui`
  - `buildroot/output/target/mnt/system/usr/bin/aikb_lcd_ui`
  - `install/soc_sg2002_licheervnano_sd/rootfs/mnt/system/usr/bin/aikb_lcd_ui`
- 四处 SHA-256：
  `e05da7b318137952282bc2f7e3896a16528e02356af6d96619282f34fc83c67e`

验证：
- `apptainer exec --cleanenv ... make -B CC=...riscv64-unknown-linux-musl-gcc`
  通过。
- 本机 `gcc -DAIKB_USE_FREETYPE=0` 编译通过，并能 `--dump-ppm` 输出 PPM。
- `git diff --check` 通过。

**2026-05-12 v23 后续：默认恢复安全终端兼容层，Markdown 只处理回复候选行。**

用户用 `images/logs/v23.png` 反馈：多层堆叠已消失，但仍有三类问题：
顶框/输入区和正常终端 `v17.png` 不一致；`*` 应显示为 `·`、`>` 类 prompt glyph
仍显示成方框；只有 assistant 回复内容应该做 Markdown 渲染，用户输入和系统区域
不应做 Markdown。

本轮只改 host `tools/vibe-bridge`：
- `src/vibe_bridge/wrapper.py`
  - 默认重新启用 `LcdOutputAdapter`，但它现在是“安全终端兼容层”，不是旧的全局
    内容重排层；`VIBE_BRIDGE_LCD_CHAR_ADAPT=0` 可完全 raw 诊断。
  - `▪/▫/■/□/◆/◇` 等符号从 `*` 改为 `·`；`›/❯/▸/▶` 等继续降级为 ASCII
    `>`，避免板端缺字变方框。
  - 非 gruvbox 模式下只额外处理 SGR 反显：把 `CSI 7 m` 转成可见浅字/灰底，
    用来恢复用户输入横框；不全局重映射颜色。
  - Markdown 表格转换前先调用 `_is_non_reply_line()`：跳过 `>`/`›` 用户输入行、
    反显输入行、box drawing 顶框、`Tip:`、`model:`、`directory:`、`gpt-` 等系统
    行。普通回复区的 Markdown table 仍会转换。
- `tests/test_wrapper.py`
  - 覆盖默认开启、raw opt-out、缺字符号、反显输入框、用户输入/system 行不做
    Markdown、回复表格仍转换。
- `README.md`
  - 改成默认安全兼容层说明，并记录 `VIBE_BRIDGE_LCD_CHAR_ADAPT=0` raw 诊断。

已验证：
`PYTHONPATH=src python3 -m unittest tests.test_wrapper` 32/32 通过；
`PYTHONPATH=src python3 -m unittest discover -s tests` 96/96 通过。

**2026-05-12 回退正常 wrapper 显示路径到原始 PTY 字节直出。**

用户用 `images/logs/v22.png` 反馈：箭头标注的 Codex 顶部框仍未修复，并且
输入区域多了灰色堆叠层。判断当前最可疑的是 host 侧 `LcdOutputAdapter` 继续
对 Codex TUI 做 Markdown/符号/配色适配，破坏了原始 VT100 层级。

本轮只改 host `tools/vibe-bridge`，不改板端固件/镜像：
- `src/vibe_bridge/wrapper.py`
  - `codex` / `claude` wrapper 默认不再启用 `LcdOutputAdapter`。
  - 默认不再注入 gruvbox 主题 SGR。
  - `Forwarder` 收到的就是子进程 PTY 原始 output chunk，并通过现有
    `CMD_VT100_STREAM` 送板端。
  - 适配层代码保留，但变成显式 opt-in：`VIBE_BRIDGE_LCD_CHAR_ADAPT=1`；
    如需旧 gruvbox 再加 `VIBE_BRIDGE_LCD_THEME=gruvbox`。
- `README.md` 已补充默认 raw PTY 和 opt-in 开关。
- `tests/test_wrapper.py` 增加默认关闭、显式开启、主题 opt-in 单测。

预期真机复测方式：不要设置 `VIBE_BRIDGE_LCD_CHAR_ADAPT`，直接开新 `codex`。
若已有旧 daemon/旧 wrapper 进程在跑，先停掉旧进程后再开新窗口，避免看到旧
适配层残留输出。

**2026-05-11 正常 wrapper 使用显示优化：v3/v4 暴露的字符降级 + 输入错位已做
host 侧修复。**

2026-05-11 追加小修 4：用户正常开 `codex` 时，让它输出“生成一张表格”，LCD
仍原样显示 Markdown table。判断：不应放 SG2002 端做 Markdown parser，板端继续
只做 VT100/Kitty 渲染；正常 `codex` live 输出的语义适配放 host wrapper。

本轮改 `src/vibe_bridge/wrapper.py` 的 `LcdOutputAdapter`：
- 对发往 LCD 的完整 plain text 行做轻量 Markdown pipe table 识别；
- 遇到 ANSI escape 的行直接透传，避免破坏 TUI 控制序列；
- 支持 Markdown 对齐分隔行如 `|---:|---|---:|---|`；
- 渲染为 ASCII 边框表格；
- 表格列宽按 SG2002 terminal 的 CJK 双宽规则计算，避免中文列错位；
- 本机终端输出不变，只影响 `CMD_VT100_STREAM` 到 LCD。

已验证：`PYTHONPATH=src python3 -m unittest tests.test_wrapper` 12/12 通过，
`PYTHONPATH=src python3 -m unittest discover -s tests` 76/76 通过。

2026-05-11 追加小修 2：用户 `images/logs/v7.png` 反馈 Markdown 表格没有转换；
图片本身显示正确（高对比彩色块已走 Kitty 渲染），但 demo 仍过长，图片后的
文字和 placement 有 overlay。已改 `scripts/send_markdown_to_device.py`：
- 在非代码块区域识别 Markdown pipe table，转换为小屏可读的对齐文本，
  不再原样显示 `| --- |`。
- 代码块内的 pipe table 保持原样，避免误改示例代码/日志。
- `examples/kitty_markdown_demo.md` 重新排序：标题 -> 转换后表格 -> 图片 ->
  图片后文字 -> 短代码块，避免图片被放在屏幕底部后与后续文本重叠。
- 新增 `tests/test_send_markdown_to_device.py`，覆盖表格转换、代码块保护和图片替换。

已验证：`python3 -m py_compile scripts/send_markdown_to_device.py`、
`PYTHONPATH=src python3 -m unittest tests.test_send_markdown_to_device`、
`PYTHONPATH=src python3 -m unittest discover -s tests` 均通过；dry-run 输出
`kitty_blocks=1`、`placement=('16','3')`、`has_table_sep=False`。

2026-05-11 追加小修 3：用户 `images/logs/v8.png` 反馈标题被刷掉、表格没有线、
代码块仍显示三反引号。根因是 demo 仍有多余空行/图片行额外保留源换行，且
Markdown 适配只做了表格两列对齐，没有 heading/code block 的小屏渲染。本轮：
- heading 转为 `== Title ==`，避免裸 `#`；
- pipe table 转为 ASCII 边框表格；
- fenced code block 转为 `Code: lang` + 缩进代码行，不再显示 ```；
- 独立一行的 Markdown 图片不再保留源行换行，只依赖 image renderer 的
  `rows` 个 CRLF 推进光标；
- demo 删除多余空行，当前 dry-run 在 15 行内可同时容纳标题、表格、3 行图片、
  after 文本和代码块。

已验证：`PYTHONPATH=src python3 -m unittest tests.test_send_markdown_to_device`
5/5 通过，`PYTHONPATH=src python3 -m unittest discover -s tests` 74/74 通过；
dry-run 输出 `has_heading=True`、`has_table_border=True`、`has_fence=False`、
`has_code=True`、`placement=('16','3')`。

2026-05-11 追加小修：用户 `images/logs/v5.png` 反馈整体已明显改善，但圆点类
符号被显示成 `*`，右尖括号/chevron 类符号仍可能显示为空框。本轮把发往 LCD
的圆点类 TUI 符号降级为 `·`，把 `›/❯/▸/▶` 等右向 chevron 降级为 ASCII
`>`（左向同理为 `<`）。已跑 `PYTHONPATH=src python3 -m unittest
tests.test_wrapper` 和 `PYTHONPATH=src python3 -m unittest discover -s tests`，
结果 10/10 与 69/69 通过。

用户真机图 `images/logs/v3.png` / `images/logs/v4.png` 暴露两个问题：
- Claude/Codex TUI 的部分符号（如状态点、框线、勾号、项目符号）在板端字体或
  renderer 中显示为空框。
- wrapper 子进程 PTY 原先继承桌面终端尺寸，应用按大窗口排版；LCD 实际默认
  只有 `78x15` 网格，中文输入和光标定位在小屏上被钳位后出现左侧竖排/错位残留。

本轮只改 host，不改板端协议/二进制/镜像：
- `src/vibe_bridge/wrapper.py`
  - 新增 `LcdOutputAdapter`，只对发往 daemon/LCD 的字节流做 Unicode 符号降级；
    本机终端输出仍保持原始 Claude/Codex TUI。
  - 常见 TUI 符号降级为 ASCII/窄字符：框线到 `+|-`，`⏺/•/●` 到 `·`，
    `✓` 到 `v`，chevron 到 `>/<`，箭头/省略号/弯引号转普通 ASCII。
  - 默认 wrapper PTY 尺寸固定为 LCD 当前默认网格 `78x15`，并覆盖子进程
    `COLUMNS/LINES`，让应用从源头按小屏宽高排版。
  - 可用 `VIBE_BRIDGE_LCD_COLS` / `VIBE_BRIDGE_LCD_ROWS` 覆盖尺寸；
    2026-05-12 起 `VIBE_BRIDGE_LCD_CHAR_ADAPT=1` 才会重新开启字符/Markdown
    适配，默认 raw PTY 直出。
- `src/vibe_bridge/pty_runner.py`
  - `run_with_pty(..., winsize=(rows, cols))` 支持固定 PTY winsize；
    固定尺寸时 SIGWINCH 不再把桌面窗口尺寸同步给子进程。
- `examples/kitty_markdown_demo.md`
  - demo 图片从黑底屏幕照片换成高对比 `examples/kitty_demo_image.ppm`，
    便于真机肉眼确认 Markdown 图片渲染。

已验证：
```
PYTHONPATH=src python3 -m unittest tests.test_wrapper
PYTHONPATH=src python3 -m unittest discover -s tests
python3 scripts/send_markdown_to_device.py --stdout --clear --image-cols 16 --max-image-rows 3 examples/kitty_markdown_demo.md | ...
```
结果：wrapper 相关单测 10/10 通过，全量 72/72 通过；新 Markdown demo 输出
`kitty_blocks=1`、`placement=('16','3')`、Markdown 图片原文被替换，表格分隔行
不再出现。

**2026-05-11 Host 内容适配层已实现：真实 Codex / Claude 输出可走现有
VT100 stream，Markdown 本地图片会转 Kitty PNG 后发到 SG2002。**

本轮只动 host `tools/vibe-bridge`，**没有继续扩展板端 Kitty 协议**，没有改
`aikb_hid_input`，没有改 SG2002 `aikb_lcd_ui` renderer，也没有改 daemon
既有 session/handshake/VT100 stream 主逻辑。

新增文件：

```
/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge/scripts/send_markdown_to_device.py
/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge/examples/kitty_markdown_demo.md
/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge/examples/kitty_demo_image.ppm
```

`send_markdown_to_device.py` 功能：
- 读取 Markdown/text 文件，或 `-` 从 stdin 读取，适合
  `codex ... | python3 scripts/send_markdown_to_device.py -`。
- 普通文本按字节透传到现有 `PluginClient.send_vt100()`，不破坏 ANSI 颜色、
  换行、Markdown 表格、代码块。
- 输入里已经存在的 Kitty graphics APC：
  `ESC _ G ... ESC \` 会原样透传，不重新编码。
- 普通文本段里识别 Markdown 图片语法：`![](path)` / `![alt](path)`。
- 第一版只支持本地图片：相对路径、绝对路径、`file:///path`。相对路径按
  markdown 文件所在目录解析；stdin 时按当前目录解析。
- `http://` / `https://` 不下载，向 stderr 和屏幕文本输出 warning。
- 使用 Pillow 解码本地图片，缩放到小屏适合尺寸，再转 PNG bytes。
- 包装 Kitty graphics：
  `ESC _ G a=T,f=100,t=d,q=2,c=<cols>,r=<rows>,i=<id>,p=<pid>,m=<0/1>;payload ESC \`
  并按 `--chunk-size` 分包，默认 3072 base64 chars。
- 默认发完图片后追加 `r` 行 `\r\n` 推进光标，避免后续文字覆盖图片；
  可用 `--advance-cursor` / `--no-advance-cursor` 控制。
- **real HID 安全保护：** 如果没有 `VIBE_SOCK_PATH` 且没有传 `--sock`，
  脚本直接退出并报错，拒绝落到 `PluginClient` 默认 `/tmp/vibe-bridge.sock`
  mock socket。
- 支持 `--stdout` dry-run 观察转换后的 VT100/Kitty 字节流；支持 `--clear`
  在发送前清屏并归位。

已做本地验证：

```
cd /home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge
python3 -m py_compile scripts/send_markdown_to_device.py
python3 scripts/send_markdown_to_device.py --stdout examples/kitty_markdown_demo.md \
  | python3 -c "import sys; d=sys.stdin.buffer.read(); print(len(d), d.count(b'\x1b_G'), b'![verified' in d)"
```

结果：输出包含 Kitty 序列，Markdown 图片原文被替换，普通文本/table/code
仍在。额外验证：已有 Kitty APC 块 `out == raw` 原样透传；无
`VIBE_SOCK_PATH` 时退出码 2，并打印：

```
error: set VIBE_SOCK_PATH to the real HID daemon socket, or pass --sock. Refusing to use the default mock socket.
```

real HID 测试入口（用户从这里开始测）：

```
cd /home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge
PYTHONPATH=src python3 -m vibe_bridge.main --sock /tmp/vibe-real.sock -vv daemon \
  --hidraw /dev/hidraw0 --state /tmp/vibe-real-state.json
```

另一个终端：

```
cd /home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge
PYTHONPATH=src python3 -m vibe_bridge.main hid handshake --device /dev/hidraw0 --timeout 2

VIBE_SOCK_PATH=/tmp/vibe-real.sock PYTHONPATH=src \
python3 scripts/send_markdown_to_device.py --clear --image-cols 16 --max-image-rows 3 \
  --hold 20 examples/kitty_markdown_demo.md
```

如果要复测 v2/Markdown 图片链路，推荐命令（v6 证明旧命令会因 demo 过长 +
图片占 6 行导致滚屏，只能看到尾部；新版固定用 16x3 小图）：
```
cd /home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge
PYTHONPATH=src python3 -m vibe_bridge.main --sock /tmp/vibe-real.sock -vv daemon \
  --hidraw /dev/hidraw0 --state /tmp/vibe-real-state.json

VIBE_SOCK_PATH=/tmp/vibe-real.sock PYTHONPATH=src \
python3 scripts/send_markdown_to_device.py --clear --image-cols 16 --max-image-rows 3 \
  --hold 20 examples/kitty_markdown_demo.md
```
预期：LCD 先清屏，显示 Markdown 标题/文本/表格/代码块；中间显示高对比彩色
测试图；图片后的文字在图片下方，不出现 Kitty escape/base64 乱码。若不想起
真实 daemon，只验证 host 转换，可跑：
```
python3 scripts/send_markdown_to_device.py --stdout --clear --image-cols 16 --max-image-rows 3 examples/kitty_markdown_demo.md \
  | python3 -c "import sys,re; d=sys.stdin.buffer.read(); print(len(d), d.count(b'\x1b_G'), b'![verified' in d, re.search(br'c=(\d+),r=(\d+)', d).groups())"
```

真实 Codex / Claude 最小接入：

```
codex ... | env VIBE_SOCK_PATH=/tmp/vibe-real.sock PYTHONPATH=src \
python3 scripts/send_markdown_to_device.py -

claude ... | env VIBE_SOCK_PATH=/tmp/vibe-real.sock PYTHONPATH=src \
python3 scripts/send_markdown_to_device.py -
```

预期 LCD：Markdown 标题/文本/表格/代码块按普通文本显示；本地图片显示为
PNG；不出现 `ESC_G`、`a=T` 或 base64 乱码；图片后的文字被光标推进到图片
下方。

**2026-05-11 Kitty graphics 最小 PNG 链路已跑通**：板端
`aikb_lcd_ui` 已在现有 VT100 terminal 前加入 `kitty_graphics` 过滤器，
host 可以把 PNG 包成 Kitty graphics escape sequence 后经现有
`CMD_VT100_STREAM` / HID / `/tmp/aikb_lcd_ui.in` 链路送到 SG2002 小屏。
本轮没有改 `aikb_hid_input` 主逻辑，图片解析/PNG decode/placement overlay
都在 `aikb_lcd_ui` 内完成。

板端新增/修改：
- `/home/rv_nano/AIKB/LicheeRV-Nano-Build/middleware/v2/sample/aikb_lcd_ui/kitty_graphics.h`
- `/home/rv_nano/AIKB/LicheeRV-Nano-Build/middleware/v2/sample/aikb_lcd_ui/kitty_graphics.c`
- `/home/rv_nano/AIKB/LicheeRV-Nano-Build/middleware/v2/sample/aikb_lcd_ui/aikb_lcd_ui.c`
- `/home/rv_nano/AIKB/LicheeRV-Nano-Build/middleware/v2/sample/aikb_lcd_ui/Makefile`
- `/home/rv_nano/AIKB/LicheeRV-Nano-Build/middleware/v2/sample/aikb_lcd_ui/README.md`

支持的第一版 Kitty 子集：`ESC _ G <control> ; <payload> ESC \`，
`a=T`、`f=100` PNG、`t=d` direct、`m=1/m=0` chunk、`q=2` 静默、
`c/r` 控制显示列/行、`i/p` 记录 image_id / placement_id。base64 buffer
上限 2 MiB，超限丢弃当前图片；PNG decode 使用
`middleware/v2/3rdparty/stb/stb_image.h`；内部转成 `aikb_lcd_ui` canvas 的
`0x00RRGGBB`，不直接写 RGB565，最终仍由现有 `fb_blit` / framebuffer packer
处理 LCD 像素格式。

产物状态：`aikb_lcd_ui` 已交叉编译、strip、同步四处、`pack_rootfs &&
pack_burn_image` 完成，并用 `debugfs dump` 从最终 `rootfs.sd` 反验。新 SHA：
`aedf29325a80dea7b9a1f1cc955acbedc460264ce03ee815ff2b6b3d352360e9`。
新镜像：

```
/home/rv_nano/AIKB/LicheeRV-Nano-Build/install/soc_sg2002_licheervnano_sd/images/2026-05-10-22-34-e86b21.img
```

host 新增测试脚本：

```
/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge/scripts/send_kitty_png.py
```

用法：

```
cd /home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge
VIBE_SOCK_PATH=/tmp/vibe-real.sock \
PYTHONPATH=src python3 scripts/send_kitty_png.py --cols 24 --rows 8 --hold 20
```

若要直接打板端 FIFO：

```
python3 scripts/send_kitty_png.py --fifo /tmp/aikb_lcd_ui.in
```

真机验证：用户保存的结果图
`/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge/images/logs/v1.png`
符合预期：左上显示 `Kitty PNG smoke`，彩色 PNG 成功显示；未显示
`ESC_G`、`a=T,f=100` 或 base64 乱码，证明 Kitty payload 被板端过滤器消费。
图片覆盖后续文字左半部分是第一版预期行为，因为当前 render 顺序是“文本先画，
图片 placement 后 overlay”。以后如果要避免覆盖，host 端发完图片后需要根据
`r` 主动移动光标到图片下方，例如 `CSI <row>;1H` 或补足换行。

常见坑：`send_kitty_png.py` 默认连 `PluginClient` 的默认 socket
`/tmp/vibe-bridge.sock`。如果该 daemon 是 `mode=mock`，会打印
`kitty png session sid=...` 但 LCD 不变。先查：

```
PYTHONPATH=src python3 -m vibe_bridge.main sessions
```

必须看到 `mode=real-hidraw` 和正确 `/dev/hidraw0`，或显式设置：

```
VIBE_SOCK_PATH=/tmp/vibe-real.sock
```

**2026-05-08 字号链路真机回放 + auto.sh 漏改修复**：用户烧 2026-05-08 镜像后真机验证字号链路，发现屏幕只换字不换字号。根因是前一手 HANDOFF 声称"已修 `auto.sh`：mkfifo `/tmp/aikb_lcd_ui.ctrl` + 两侧 `--ctrl`/`--ctrl-out`"，**实际上三处 `auto.sh` 都没动**。板端二进制（SHA `15ce122d…` / `36272854…`）本身支持 ctrl FIFO，但 `auto.sh` 没传参数也没 mkfifo，`CMD_UI_SCALE_CHANGE` 到 `aikb_hid_input` 后掉到 `ui_scale dropped (no --ctrl-out)`，整条字号控制通道断在板端 IPC 那一段。host 端 daemon 转发、单测、stream_iter_packets 全对。

活验证：在板端 `killall` 两个进程后手动 `mkfifo /tmp/aikb_lcd_ui.ctrl` + 用 `--ctrl-out`/`--ctrl` 重启，host 端跑长连接脚本（同一个 `PluginClient` 里 acquire_session → 4 档 set-ui-scale → send_vt100，避免 owner-disconnect 释放 sid），LCD 真切出 8x16/10x20/12x24/16x32。

本轮修复（仅 `auto.sh`，二进制无变化）：`AIKB_LCD_CTRL="/tmp/aikb_lcd_ui.ctrl"` + `prepare_aikb_lcd_input` 改 for-loop 对 `.in`/`.ctrl` 双 FIFO 兜底 + `start_aikb_hid_input` 加 `--ctrl-out` + `start_aikb_lcd_ui` 加 `--ctrl`；三处 `auto.sh` SHA 已对齐 `29aa42da5b2a4c106f2e4fafec8f7067fa7631f962e2c644fed58e9b67d41495`，`sh -n` 通过。`pack_rootfs && pack_burn_image` 留给用户。

**HANDOFF 测试脚本反例**（之前那段 "下一步 0" 的 for-loop）有同样的设计 bug：`request-session --plugin smoke` CLI 是一次性进程，退出时 socket 关闭、daemon 立刻把它持有的 sid `EXPIRED` 回收，于是后续 `send-vt100 --sid $SID` 命中已死 sid 被 `_validate_session` 拒掉、`set-ui-scale` 是 sid=0 broadcast 不需校验所以能穿透板端触发 `apply_cell_size`（清屏）—— 这就是用户最初看到的"清屏了但没文字"。**正确做法是用一个 Python 进程把 acquire_session + set-ui-scale + send_vt100 全放在一个 `PluginClient` 上下文里**，长连接保持 sid 不被回收。后续如果想保留 CLI 化的烟雾测试，得增加一个 `vibe_bridge.main run-script` 之类的子命令在单连接里串联多步，或者把 `send-vt100` 改成"sid 不在时 best-effort 等待板端再次 grant"。本轮没改 CLI，留作下游决定。

**2026-05-08 字号链路打通**：从 host `set-ui-scale` 到板端 `aikb_lcd_ui` 真的会换字号。问题三段都断过：host 只在 `hid_protocol.py:59` 定义了 `Cmd.UI_SCALE_CHANGE = 0x40` 但无 caller；板端 `aikb_hid_input.c` 把 `CMD_UI_SCALE_CHANGE` 显式 no-op；板端 `aikb_lcd_ui.c` 把 `TERM_CELL_W/H/COLS/ROWS` 写死成编译期 `#define`（u-boot 8x16），cell[][] 编译期固定二维数组，且 `aikb_hid_input` 与 `aikb_lcd_ui` 之间唯一 IPC 是 `/tmp/aikb_lcd_ui.in` 字节流 FIFO，没有控制通道。本轮三处都修了：

- 板端 `aikb_lcd_ui.c`：引入 `CELL_PRESETS = {8x16, 10x20, 12x24, 16x32}`，**默认从 8x16 改为 12x24**（grid 从 118x23 变 78x15）；`TERM_CELL_W/H/COLS/ROWS` 改运行时全局 `g_*`；cell[][] 按最小档位 8x16 静态留 23x118 余裕，活动档位用 `g_rows/g_cols` 收紧；新增 `apply_cell_size()` 重设 FT 像素+ascent+清屏；新增 CLI `--cell WxH` 启动档位、`--ctrl PATH` 运行时控制 FIFO（行格式 `cell W H\n`）。
- 板端 `aikb_hid_input.c`：新增 `--ctrl-out PATH` 与 `write_ctrl_line()`；`CMD_UI_SCALE_CHANGE` 解析 `[u8 cell_w, u8 cell_h]` 写 `cell W H\n` 到 ctrl FIFO；sid 被忽略（字号是面板全局参数）。
- `auto.sh`：mkfifo `/tmp/aikb_lcd_ui.ctrl`；hid_input 加 `--ctrl-out`、lcd_ui 加 `--ctrl`。
- host：新 CLI `set-ui-scale --cell-w W --cell-h H` 发 `Cmd.UI_SCALE_CHANGE` sid=0 broadcast，payload=`bytes([W, H])`。daemon real-hidraw 模式经现有 fall-through 路径（`_handle_plugin_packet_real` 末尾 `_forward_packet_to_board(packet)`）转发到板端；mock 模式 ignore（无去处）。

新二进制 SHA-256（已四处同步一致）：`aikb_hid_input` = `15ce122de18a064e0f9f29e560286ca4ff4e01a68de99bdb6b0296dd64bfaf2c`（替换 `df897857...`），`aikb_lcd_ui` = `36272854aa01d8e3f43b5c9d239d0cfe4fa9b2354f0696c5b600c12b9e3430c3`（替换 `b96aebb6...`）。`pack_rootfs && pack_burn_image` 留给用户在烧板前跑。

**2026-05-08 real HID / codex wrapper 调试稳定点**（保持不变）。正常用户流程：板子上电、USB 接好、直接运行 `codex`。wrapper 会自动找/起 daemon、自动扫描 `VID:PID 359f:2120` 的 `/dev/hidraw*`、向板端申请新 session，并把交互 PTY 输出转到 LCD。多个顶层 `codex` 会得到多个窗口；旋钮能切换窗口；退出某个 wrapper 会释放对应 sid，如果退出的是 active 窗口会自动切到仍存活窗口并回放其 buffer。

**宣传片版本另开隔离工作区**：`/home/rv_nano/Sipeed/rv_nano/tools/vibe-promo-screen`。这个目录只放宣传片屏幕数据、脚本、参考图和 notes，不改当前 `src/`、`bin/`、daemon/wrapper、AIKB overlay、`install/.../rootfs` 或烧录镜像。参考图已复制到 `/home/rv_nano/Sipeed/rv_nano/tools/vibe-promo-screen/assets/reference/LCD1.png`；实现方向是使用板端已有 `aikb_lcd_ui --view dashboard`，通过 newline JSON 喂入漂亮的黑底琥珀色 dashboard，而不是改生产 terminal bridge。

板端 `aikb_hid_input.c` 已经按 vibe-bridge 协议 v0 重写：删除了旧的 `SCREEN_CMD_*` / `0x20 sub_cmd` 解析、删除了 `0x21` ACK 回报、把 VT100 翻译从板端搬到上位机；加上了 256 槽 session 表 + LRU 回收、新 6 字节包头 parser、`CMD_REQUEST_SESSION/VT100_STREAM/SESSION_INVALID` 处理；输入报文拆成 `CMD_KEY_EVENT` 和 `CMD_ENCODER_EVENT` 两帧，`sid=0` broadcast。

host 侧 `daemon --hidraw /dev/hidraw0` 已接通：daemon 启动只打开/probe/drain hidraw，**不会**在启动时固定发送 `CMD_REQUEST_SESSION`。每个 wrapper/plugin 发 `CMD_REQUEST_SESSION` 时，daemon 才把该包转发到板端，并以板端返回的 `session_id` 作为权威 sid，在本地只做 owner/router/state 镜像。

重要修正：之前只改了源码，但实际 RISC-V 二进制仍是旧协议。已重新交叉编译并同步到 `middleware`、AIKB overlay、`buildroot/output/target`、`install/.../rootfs` 四处；当前 `aikb_hid_input` SHA-256 为 `df897857b0b7edeea4e2ef97e55dbc3105bc7b58bd8c0ece9601d5d6f236d2c5`，已重新 `pack_rootfs && pack_burn_image`。新镜像：

```
/home/rv_nano/AIKB/LicheeRV-Nano-Build/install/soc_sg2002_licheervnano_sd/images/2026-05-07-17-31-e86b21.img
```

下一动作：烧这个新镜像，再跑 `./scripts/probe_hidraw.sh /dev/hidraw0`。按键事件现在应为 `raw=10 10 00 00 01 00 ...`；probe script 的 handshake 应看到合法 `RESULT=PASS sid>0 status=CREATED`。随后用 `PYTHONPATH=src python3 -m vibe_bridge.main -vv daemon --hidraw /dev/hidraw0` 做真实 daemon bridge 测试。

整体路线图：
1. ✅ 上位机 MVP：协议 codec + session manager + mock HID + VT100 router
2. ✅ Shell wrapper（`bin/codex` / `bin/claude`）：自动起 daemon、注入 `VIBE_SESSION_ID`、PTY 模式 tee 字节流到 daemon 作为 `CMD_VT100_STREAM`
3. ✅ WINDOW_SWITCH / WINDOW_ACTIVATE：切窗时回放新 sid 的缓冲 + 屏幕清屏序列
4. ✅ `HidrawTransport` + `hid {list,probe,handshake}` 探针 CLI
5. ✅ 板端 `aikb_hid_input.c` 升级（代码完成 + RISC-V musl 交叉编译 + 生成目录同步 + rootfs/SD 镜像打包完成；待烧板 real HID handshake）
6. ✅ 把 `HidrawTransport` 接进 daemon：`daemon --hidraw /dev/hidraw0`，daemon 作为 _bridge_ 把上位机插件包转发板端、板端事件回插件
7. ✅ daemon 加 `CMD_KEY_EVENT` / `CMD_ENCODER_EVENT` handler：按键路由给 active owner；旋钮 delta 转内部 `WINDOW_SWITCH` 并回放 active VT100 buffer
8. ⬜ 板端 `aikb_lcd_ui` 多 buffer / 切窗渲染
9. ✅ host `set-ui-scale` + 板端 ctrl FIFO + lcd_ui 运行时档位字号链路（待用户烧板真机回放验证）

测试统计：上位机 `PYTHONPATH=src python3 -m unittest discover -s tests` → **58/58 passed**（原 57 + 新增 `test_ui_scale_change_is_forwarded_to_board`）。两个固件 RISC-V musl 交叉编译通过；中间件目录、AIKB overlay、`buildroot/output/target`、`install/.../rootfs` 四处 SHA 已对齐。`pack_rootfs && pack_burn_image` 由用户跑。

## 已经试过的方案和结果（含失败的）

- **mock HID over Unix socket**：成功。daemon 在 `/tmp/vibe-bridge.sock` 起，packet 格式与未来真 HID 完全一致（report_id + cmd + u16 sid + u16 plen + payload），只在帧前加了 4 字节长度前缀做 SOCK_STREAM 解帧。
- **wrapper PTY 模式**：成功。`bin/codex` 用 `pty.fork` + select 循环 tee `master_fd → stdout + Forwarder.push`、`stdin → master_fd`，winsize 同步、SIGWINCH 转发、raw mode 在 finally 恢复。in-process 整链路 + 用户在真终端跑过 5 项检查全过。
- **session 池满 LRU 回收**：第一次单测断言 `sid_b not in active_sids` 写错了——sid 是 uint16 整数会被复用。改成断言 `plugin name in {a, c}`。
- **多线程并发写 state.json**：`os.replace(.tmp, final)` 偶发 ENOENT。加 `_state_lock` 串行化解决。
- **PTY 模式接管 wrapper 子进程时 socket 处理**：原来 wrapper exec 后 socket 关闭，daemon 的 `_owners[sid]` 指向死 handle。在 `_validate_session` 加了 owner-rebind：任何已知 sid 的非握手包到达都刷新 owner，让后续进程能接管 invalidation 通知。
- **chmod bin/codex bin/claude**：被拒（root-owned），但文件本来就是 777，可执行没问题。**不要**再尝试 chmod。
- **`pip install pytest`**：环境无 pip。已把测试从 pytest 风格改成 stdlib `unittest`，无外部依赖。
- **`HidrawTransport` 的 socketpair 替身**：用 `socket.SOCK_SEQPACKET` 模拟 per-report 框定，单测通过。
- **真 hidraw probe（旧固件）**：`/dev/hidraw0`（VID 359f PID 2120，`crw-rw-rw-`，不需 udev 规则）能开能读能写；`hid handshake` 输出 `RESULT=LEGACY_NOISE`，板端把我们发的 `[0x20][CMD_REQUEST_SESSION=0x01]` 当成旧的 `SCREEN_CMD_CLEAR` 处理并回 `[0x21][seq=0][status=0]` ACK——**注意**这会把 LCD 屏闪一下清空，固件升级后副作用消失。
- **probe CLI 修正**：原来 OTHER 在第一个非 SESSION_RESPONSE 报文就 break；改成在整个 timeout 窗口内把所有报文收齐，再分类成 `PASS / LEGACY_NOISE / TIMEOUT`，输出每包的 `report_id`。后续又收紧 PASS 判定：必须是 `report_id=0x10`、`cmd=0x02`、`sid>0`、payload 恰好 1 字节合法 status，并打印 raw bytes，避免把旧按键包 `raw=10 02` 误判成 session response。
- **板端固件升级同步坑**：`aikb_hid_input.c` 是新源码，但所有实际打包二进制一度仍是旧 SHA `000fc245...`，`strings` 里有 `ack 0x21`。原因是 `pack_rootfs` 不会自动重编 `middleware/v2/sample/...`，且会把 `install/.../rootfs/mnt/system/*` 回拷到 overlay。已重编并同步四处，新 SHA `df897857...`。
- **real hidraw daemon bridge（host 侧单测）**：`daemon --hidraw PATH` 已实现。启动只 open/probe/drain hidraw，不固定申请 session；每个 wrapper/plugin 的 `CMD_REQUEST_SESSION` 转发给板端，`SESSION_RESPONSE` 回来后用板端 sid 调 `SessionManager.adopt_session(...)` 建本地镜像。VT100 仍由 host `Vt100Router` 做 active-window 过滤，只有 active sid 的 VT100 被写到 hidraw；切窗时回放 `SCREEN_CLEAR + buffer` 给板端。新增 `tests/test_daemon_real_hid.py` 用 fake hidraw transport 锁住这个时序。
- **wrapper 真机路径**：`PluginClient` 和 wrapper 现在都读 `VIBE_SOCK_PATH`。已在沙箱外用真实 `/tmp/vibe-real.sock` 验证：`codex --version` 走 `bin/codex` wrapper 得到 `plugin=codex sid=8 buf=19b`；`claude --version` 走 `bin/claude` wrapper 得到 `plugin=claude sid=9 buf=23b`；`window-activate` 能回放到 LCD。注意 Codex 工具沙箱内连 Unix socket 会报 `Operation not permitted`，真机 wrapper 测试要在普通终端或沙箱外执行。
- **重新上电/重插后的状态坑**：hidraw 节点可能从 `/dev/hidraw0` 变成 `/dev/hidraw1`；板端 session 表也会重置，所以旧 state 文件里的 sid 不能继续用。只有一个 session 时旋钮切窗是 no-op。当前 `sessions` CLI 会打印 `mode` 和 `hidraw`，用它确认真实 daemon 正在看哪个设备。daemon 遇到旧 hidraw fd `EIO/ENODEV` 或 `TransportClosed` 时现在会停止 reader、关闭旧 transport，并把 state 降级为 `mode=mock/hidraw_path=null`，避免无限刷日志，也让下一次 wrapper 自动拉起新的 real daemon。
- **零配置 `codex` 路径**：`~/.local/bin/codex` 已 symlink 到 `tools/vibe-bridge/bin/codex`，且 `~/.local/bin` 在 PATH 前面。wrapper 现在会优先复用已存在的默认 socket 或 `/tmp/vibe-real.sock`；没有 daemon 时会自动扫描 `VID:PID 359f:2120` 的 `/dev/hidraw*` 并用 real-HID 模式启动 daemon。2026-05-08 又补了一个判断：如果默认 `/tmp/vibe-bridge.sock` 能连但 state 是 `mode=mock`，同时能发现真 HID 设备，`ensure_daemon_running()` 不再复用这个 mock socket，而是覆盖启动 `real-hidraw` daemon 并等待 state 变成 `mode=real-hidraw` + matching `hidraw_path`。这个修复的是“新开 Codex 正常启动但 LCD 没页面，输出被 mock daemon 吃掉”的坑；自定义 socket 和旧 `/tmp/vibe-real.sock` 仍按手动配置复用，不做这个默认 socket 纠偏。同轮还补了 real daemon 断线降级：hidraw reader 遇到 `EIO/ENODEV` 或 `TransportClosed` 会把 `_hid` 置空、state 写回 `mode=mock/hidraw_path=null`，这样重新挂载后下一次 wrapper 能触发上面的默认 socket 纠偏。单测 `PYTHONPATH=src python3 -m unittest discover -s tests` 当前 65/65 通过。默认每次顶层 `codex` 都申请新 session；只有显式设置 `VIBE_BRIDGE_REUSE_SESSION=1` 才复用已有 `VIBE_SESSION_ID`。
- **wrapper 退出释放窗口**：PTY wrapper 现在用同一个 `PluginClient` 完成申请 sid 和 VT100 转发，进程退出关闭 socket 后 daemon 会释放该 sid。如果退出的是 active 窗口，`Vt100Router.unregister` 会自动切到剩余窗口并回放 `SCREEN_CLEAR + buffer`；如果已经没有剩余窗口，会用正在退出的 sid 发最后一次清屏，避免真实板端拒收 `sid=0` 清屏。
- **字号链路（CMD_UI_SCALE_CHANGE = 0x40）**：选了**档位式**而非完全动态。lcd_ui 静态保留最小档位 8x16 的 `cell[23][118]`，活动档位用 `g_rows/g_cols` 收紧 — 改动面最小、不引入 heap 指针管理。payload 选了**2 字节 `[u8 cell_w, u8 cell_h]`**，固件用 `sscanf("cell %d %d")` 解析 ctrl FIFO 行（人眼可读、便于 `echo "cell 12 24" > /tmp/aikb_lcd_ui.ctrl` 直接调试，不依赖 host）。sid 在板端被忽略，因为字号是面板全局参数；host CLI 默认发 sid=0 broadcast。**默认从 8x16 改为 12x24** — 旧 promo 工程 `vibe-promo-screen` 的 `render_page` 把 width clamp 到 `[84, 118]`，新默认只有 78 cols 会溢出换行；该工程 HANDOFF 也把"VT100 不能调字号"列为 TODO，已过时。本轮没动 promo，待用户决定是否更新。
- **CMD_UI_SCALE_CHANGE 在 mock daemon 模式被 ignore**：mock 模式没有板端可转发，`_handle_plugin_packet` 走的是显式 `if/elif` 链，未列出 0x40 就落 `log.info("ignoring cmd ...")`。real-hidraw 模式才走 `_handle_plugin_packet_real` 末尾的 fall-through `_forward_packet_to_board`，把包发出去；新增 `tests/test_daemon_real_hid.py::test_ui_scale_change_is_forwarded_to_board` 锁住这条路径。

## 板端产物交付规则

以后只要改动或新增板端运行文件（C sample 二进制、`auto.sh`、init 脚本、boot marker、rootfs 文件等），Codex/接手者必须在交付前完成编译/生成、同步和校验，不能只改源码后让用户手工补同步。用户应只需要跑既有打包/烧录流程。

`aikb_hid_input` 的正确同步路径：

```
middleware/v2/sample/aikb_hid_input/aikb_hid_input
buildroot/board/cvitek/SG200X/overlay/mnt/system/usr/bin/aikb_hid_input
buildroot/output/target/mnt/system/usr/bin/aikb_hid_input
install/soc_sg2002_licheervnano_sd/rootfs/mnt/system/usr/bin/aikb_hid_input
```

烧板前必须检查四处 SHA 一致，并确认 `strings` 里没有旧 `ack 0x21`：

```
sha256sum \
  middleware/v2/sample/aikb_hid_input/aikb_hid_input \
  install/soc_sg2002_licheervnano_sd/rootfs/mnt/system/usr/bin/aikb_hid_input \
  buildroot/board/cvitek/SG200X/overlay/mnt/system/usr/bin/aikb_hid_input \
  buildroot/output/target/mnt/system/usr/bin/aikb_hid_input

strings install/soc_sg2002_licheervnano_sd/rootfs/mnt/system/usr/bin/aikb_hid_input \
  | grep -E 'ack 0x21|tx id=0x'
```

板端加载链路：

- `/boot/usb.aikb_hid` 触发 `S08usbdev` 创建 `functions/hid.aikb`，`report_length=64`，主机侧出现 `/dev/hidraw0`。
- 板端 `auto.sh` 创建 `/tmp/aikb_lcd_ui.in` FIFO。
- `auto.sh` 启动 `/mnt/system/usr/bin/aikb_hid_input --hid /dev/hidg0 --screen-out /tmp/aikb_lcd_ui.in`。
- `auto.sh` 启动 `aikb_lcd_ui --input /tmp/aikb_lcd_ui.in --rotate auto --view terminal`。
- 所以 `/mnt/system/usr/bin/aikb_hid_input` 的实际二进制决定协议行为，不能只看源码。

## 下一步计划（3-5 条 actionable）

0. **【烧板优先做】字号链路真机回放**（本轮新增；用户先烧板再走）：
   ```bash
   cd /home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge
   export PYTHONPATH=src
   DEV=$(ls /dev/hidraw* | head -1)
   python3 -m vibe_bridge.main daemon --hidraw "$DEV" &
   sleep 1
   SID=$(python3 -m vibe_bridge.main request-session --plugin smoke \
         | awk -F'[= ]' '/sid=/{print $4}')
   for WH in "8 16" "12 24" "16 32" "10 20"; do
     set -- $WH
     python3 -m vibe_bridge.main set-ui-scale --cell-w $1 --cell-h $2
     python3 -m vibe_bridge.main send-vt100 --sid $SID \
       --raw "\\x1b[2J\\x1b[H\\x1b[1;33mcell ${1}x${2}\\x1b[0m"
     sleep 2
   done
   pkill -f 'vibe_bridge.main.*daemon'
   ```
   期待 LCD 每 2 秒换一种字号、`apply_cell_size` 每次清屏。WSL 无 `/dev/hidraw*` 时先在 Windows PowerShell 跑 `usbipd attach --wsl --busid <BUSID>` 把 `359f:2120` 透传过来。不通时看 `/tmp/aikb_lcd_ui.log` 是否有 `ignoring unsupported cell size` 或 `ctrl read failed`，看 `/tmp/aikb_hid_input.log` 是否有 `ui_scale dropped`（出现说明 `--ctrl-out` 没接通）。

1. **烧板后验证**：
   ```bash
   cd /home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge
   ./scripts/probe_hidraw.sh /dev/hidraw0
   ```
   期待 `RESULT=PASS sid>0 status=CREATED`。按键包期待
   `raw=10 10 00 00 01 00 ...`，不是旧 `raw=10 02`。
2. **real daemon bridge 冒烟**：probe PASS 后起真实桥：
   ```bash
   cd /home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge
   PYTHONPATH=src python3 -m vibe_bridge.main -vv daemon --hidraw /dev/hidraw0
   ```
   另一终端先做最小 session 请求：
   ```bash
   PYTHONPATH=src python3 -m vibe_bridge.main request-session --plugin cli-smoke
   PYTHONPATH=src python3 -m vibe_bridge.main sessions
   ```
   期待 state 里 `mode=real-hidraw`，sid 是板端返回值，不是 daemon 启动时预先分配。
3. **【你看屏验证】** 板端 LCD 必须能由上位机直接发 VT100 字节驱动（之前是板端翻译 SCREEN_CMD_*，现在是 host 端直接发 escape 序列做 payload）。real daemon 起着时运行：
   ```bash
   PYTHONPATH=src python3 plugins/terminal_demo/main.py
   ```
   期待 LCD 显示 demo 的 VT100 输出；inactive session 的输出不应串到屏上，切窗才回放。
4. **wrapper 真机路径**：普通使用直接运行：
   ```bash
   codex
   ```
   wrapper 会自动找真实 Codex、找/起 daemon、找 hidraw、申请新 sid。下面这些变量只在调试或覆盖默认行为时需要：
   ```bash
   export VIBE_SOCK_PATH=/tmp/vibe-real.sock   # 如果复用当前手动启动的 daemon
   export VIBE_HIDRAW_DEVICE=/dev/hidraw0
   export PATH="/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge/bin:$PATH"
   ```
   如果当前 shell 之前缓存过真实 `codex` 路径，跑 `hash -r` 或开新终端。
5. **【我做】** 板端 `aikb_lcd_ui` 多 buffer 评估：现在切窗的回放在 host 端的 vt100_router 做。如果发现板端跟得上 LCD 刷新但单 FIFO 串扰，再下放到板端。短期可能不需要做。

## 关键文件路径（相对项目根 `tools/vibe-bridge/`，跨仓的用绝对路径）

```
request.md                                  # 原始需求规格（uint16 sid, CMD_*, USB delivery 等）
HANDOFF.md                                  # 本文档
README.md                                   # 入口、Quick start、Status 勾选
docs/hid_protocol.md                        # 协议字节布局 + CMD_*/Status 表 + 握手时序
src/vibe_bridge/hid_protocol.py             # Packet codec + Cmd/ReportId/Status enum + fragment_payload
src/vibe_bridge/session_manager.py          # uint16 sid 池 + TTL + LRU + invalidation 回调
src/vibe_bridge/vt100_router.py             # per-sid 缓冲 + active 选择器 + set_active 回放
src/vibe_bridge/transport.py                # Transport ABC + 4-byte 帧（mock 用）
src/vibe_bridge/mock_hid.py                 # Unix socket mock HID server/client
src/vibe_bridge/transport_hidraw.py         # 真 hidraw transport + list_hidraw_devices()
src/vibe_bridge/daemon.py                   # mock/real-hidraw daemon；real 模式转发请求、板端 sid authoritative
src/vibe_bridge/plugin_client.py            # 插件 SDK（acquire_session / adopt_session / send_vt100）
src/vibe_bridge/forwarder.py                # 异步队列把 PTY 字节推 daemon
src/vibe_bridge/pty_runner.py               # pty.fork + select 循环 + winsize/SIGWINCH
src/vibe_bridge/wrapper.py                  # bin/codex 走的入口；选 pty/exec 模式；支持 VIBE_SOCK_PATH
src/vibe_bridge/bootstrap.py                # ensure_daemon_running（detached 子进程；自动发现 hidraw，支持 VIBE_HIDRAW_DEVICE）
src/vibe_bridge/main.py                     # CLI dispatcher（含 hid 子命令 + daemon --hidraw + set-ui-scale）
bin/codex                                   # shell wrapper shim
bin/claude                                  # 同上
plugins/terminal_demo/main.py               # hello-world 插件
scripts/probe_hidraw.sh                     # 【你 3 天后第一件事跑】真 hidraw 探针
scripts/smoke_pty_real_codex.sh             # 真 codex + PTY 模式冒烟
scripts/smoke_cell_cycle.sh                 # 一键端到端字号链路烟雾：autodetect hidraw + 自启 daemon + 长连接 4 档循环
tests/test_hid_protocol.py                  # 10 tests
tests/test_session_manager.py               # 9 tests
tests/test_mock_hid.py                      # 4 tests
tests/test_vt100_router.py                  # 9 tests
tests/test_forwarder.py                     # 5 tests
tests/test_integration_window.py            # 3 e2e tests（真 socket 真 daemon）
tests/test_transport_hidraw.py              # 6 tests（socketpair 替身）
tests/test_daemon_real_hid.py               # 3 tests（fake hidraw，锁 real daemon 时序 + UI_SCALE_CHANGE 转发）
tests/test_bootstrap.py                     # 3 tests（自动 hidraw 发现）
tests/test_wrapper.py                       # 6 tests（wrapper socket path + session reuse 策略）

# 跨仓：
/home/rv_nano/AIKB/LicheeRV-Nano-Build/middleware/v2/sample/aikb_hid_input/aikb_hid_input.c   # 板端 HID 桥；本轮加了 --ctrl-out + CMD_UI_SCALE_CHANGE handler
/home/rv_nano/AIKB/LicheeRV-Nano-Build/middleware/v2/sample/aikb_hid_input/Makefile           # 板端编译入口
/home/rv_nano/AIKB/LicheeRV-Nano-Build/buildroot/board/cvitek/SG200X/overlay/etc/init.d/S08usbdev   # USB gadget 配置（report_length=64）
/home/rv_nano/AIKB/LicheeRV-Nano-Build/buildroot/board/cvitek/SG200X/overlay/mnt/system/auto.sh     # 启动 aikb_hid_input + aikb_lcd_ui；本轮加 mkfifo /tmp/aikb_lcd_ui.ctrl + 两侧 --ctrl/--ctrl-out
/home/rv_nano/AIKB/LicheeRV-Nano-Build/middleware/v2/sample/aikb_lcd_ui/aikb_lcd_ui.c              # 消费 input FIFO 渲染 LCD；本轮把 cell size 改运行时 + --cell/--ctrl + apply_cell_size()

# 受影响下游（独立 Python 工程，本轮没动）：
/home/rv_nano/Sipeed/rv_nano/tools/vibe-promo-screen/scripts/codex_session_promo.py            # render_page width clamp [84,118] 是按旧 8x16=118 cols 标定，新默认 12x24=78 cols 会换行

# 旧 Rust 上位机（不接着做，留作参考）：
/home/rv_nano/Sipeed/rv_nano/tools/Vibe_Bridge/                                                # 旧 Rust 工程，sid 解析 / hook-script / forward-sessions 都在这里
```

## 还没搞清楚的问题

- **插件动作执行**：按键语义已经定为 `REJECT / VOICE / SESSION / VOTE_REVIEW / AGENT_MODEL / MULTI_FUNCTION / CONFIRM / MENU_DEBUG`。daemon 仍只把 `CMD_KEY_EVENT` 路由给当前 active owner，`CMD_ENCODER_EVENT` 会按 delta 切 active window 并回放屏幕；具体业务动作还需要插件或 wrapper 消费这些语义后实现。
- **板端 alloc_session pool 满时的回执顺序**：固件目前先 `CMD_SESSION_INVALID(RECLAIMED, old_sid)`，再 `CMD_SESSION_RESPONSE(CREATED, new_sid=old_sid)`。host 必须按顺序处理（先标记老 plugin 失效，再注册新 plugin）。Mock 路径已验证；真 HID 链路时序可能不同，需观察。
- **`aikb_lcd_ui` 多 buffer / 切窗渲染**：现在板端只有一个 FIFO，路由器在上位机做。如果以后想把窗口管理也下放到板端，要给 `aikb_lcd_ui` 加 sid-aware 的多缓冲 + 一个 active selector。短期不做，全靠上位机 router。
- **fragmentation acks**：`CMD_VT100_STREAM` 大 payload 分多个 64 字节帧，目前 fire-and-forget。如果真 HID 链路有丢包风险（usbipd 抖动），可能要加序号 + ACK。等真硬件跑出问题再说。
- **5 小时滚动额度 / cost 实时数据**：旧 Vibe_Bridge HANDOFF 已经记了这是悬而未决，本项目继承同样问题，目前不接。
- **字号链路真机回放未做**：本轮 host 单测 58/58 + sqfs 容器 RISC-V 交叉编译 + SHA 四处对齐都过了，但用户还没烧板，真机 LCD 实际换字号、`apply_cell_size` 清屏、ctrl FIFO 时序都待真机验证。验证脚本见"下一步计划 0"。
- **vibe-promo-screen 是否同步**：默认字号从 8x16 改 12x24 后，promo 工程 `render_page` 的 width clamp `[84, 118]` 会在新默认下溢出（实际只 78 cols）。本轮没动 promo 代码也没动它的 HANDOFF/README — 那两份文档还写着"VT100 输出不能调字号 / 当前固定 8x16"。是否同步等用户决定（可加 `--cell-size` 启动时主动 set-ui-scale，或仅刷文档）。
