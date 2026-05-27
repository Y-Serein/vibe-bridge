# AIKB Project Definition

## 1. Product Definition

AIKB is a small AI keyboard companion built around LicheeRV Nano / SG2002. The board provides local keys, a small LCD UI, HID transport, and a lightweight session surface for AI tools such as Codex / Claude through `vibe-bridge`.

The intended product behavior is:

- User installs the host plugin / bridge once.
- AIKB powers on and enters a usable UI without a setup ritual.
- Keys select AI-oriented actions and UI themes.
- Session pages show live agent / terminal state when available.
- Non-session pages can idle into sleep after no local input.
- Normal host workflows remain intact; the bridge must not break ordinary `codex`, `claude`, Windows Terminal, Ubuntu, or WSL usage.

## 2. Repository Boundaries

Board-side work lives under:

```text
/home/rv_nano/AIKB/LicheeRV-Nano-Build
```

Host-side bridge and project context live under:

```text
/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge
```

Project context, reusable notes, and deliverable docs should be placed in:

```text
/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge/C_context
```

Do not put this product definition document in the AIKB board repo unless the user explicitly changes that rule.

## 3. Hardware Definition

Known board target:

- SoC / board family: SG2002 / LicheeRV Nano.
- Build target: `sg2002_licheervnano_sd`.
- Storage target: SD / TF image today; product direction includes smaller TF or SD NAND around 1GB.
- Memory target: 256MB DDR is acceptable and is separate from TF / SD NAND storage size.
- Display: framebuffer LCD driven by board-side `aikb_lcd_ui`.
- Input: local AIKB keys and encoder events bridged through board-side event FIFOs.
- Host transport: USB HID through the bridge stack.

Important distinction:

- Rootfs / image size affects TF / SD NAND capacity.
- AKIM files can dominate storage size, but mmap playback does not mean the full file is permanently resident in DDR.
- Runtime memory stability must be evaluated separately from image size.

## 4. Software Definition

### Board Runtime

Primary board components:

- `buildroot/board/cvitek/SG200X/overlay/mnt/system/auto.sh`
  - Starts board runtime pieces.
  - Wires LCD UI, key events, boot/wait/sleep animation arguments.
- `middleware/v2/sample/aikb_lcd_ui/aikb_lcd_ui.c`
  - Renders pet, session picker, terminal, sleep, and local UI state.
  - Owns UI titles, footer, idle sleep, AKIM playback, and terminal rendering.
- `middleware/v2/sample/aikb_hid_input/aikb_hid_input.c`
  - Bridges local key/session/HID state.
  - Handles host session request / response and emits state lines to UI.
- Overlay binaries:
  - `buildroot/board/cvitek/SG200X/overlay/mnt/system/usr/bin/aikb_lcd_ui`
  - `buildroot/board/cvitek/SG200X/overlay/mnt/system/usr/bin/aikb_hid_input`

### Host Runtime

Host bridge components live in `vibe-bridge`:

- Windows daemon / installer handles product install, daemon lifecycle, HID ownership, WSL shell integration, and terminal capture.
- WSL / Windows Terminal product rules are documented in `C_context/MEMORY.md` and `C_context/skills/vibe-bridge-control-loop/SKILL.md`.

The board should not assume host capture internals. The board consumes session/key/terminal state through the established HID / FIFO paths.

## 5. UI Definition

Current UI principles:

- Key title is user-facing state and should remain stable after key press.
- Internal scene names must not unexpectedly replace user-facing titles.
- `RUN` means an agent task is running; it must not show OTA/update firmware semantics.
- `VOICE` remains voice-facing, not `LISTEN`.
- Session picker / terminal pages are active interaction surfaces and must not be covered by idle sleep.
- Non-session pages may enter sleep after 3 minutes without local key input.
- Boot animation is not part of the product requirement and has been removed.
- Sleep animation uses `vedio_sleep.akim`.

## 6. Resource Definition

Current resource decisions:

- `vedio_start.akim`: removed / not used.
- `vedio_sleep.akim`: used for idle sleep.
- Pet AKIM files remain part of the UI experience.
- Old / unused / uncertain resources should be audited by reference before deletion.

Do not delete resources only because they look unused in one directory. Check:

- `auto.sh`
- board C source
- overlay paths
- Buildroot target rootfs
- generated install rootfs

## 7. Build And Image Definition

User-owned build command:

```bash
apptainer exec --cleanenv host/ubuntu/licheervnano-build-ubuntu.sqfs bash -lc \
'cd /home/rv_nano/AIKB/LicheeRV-Nano-Build && source build/cvisetup.sh && defconfig sg2002_licheervnano_sd && pack_rootfs && pack_burn_image'
```

Agent boundary:

- Do not run `pack_rootfs && pack_burn_image` unless the user explicitly changes the rule.
- It is acceptable to prepare source/config/scripts so the user can run the command.
- It is acceptable to run static checks and syntax checks.
- Be explicit when a new `.img` has not been generated.

Current size target:

- Buildroot ext4 rootfs: `900M`.
- SD partition XML rootfs: `921600KB`.
- Expected burn image scale: about `916 MiB`, usually suitable for decimal 1GB storage with limited margin.

Current post-build pruning removes unused heavy payloads:

- Python 3.11 runtime and binaries.
- Qt libraries and metadata.
- OpenCV libraries.
- ffmpeg / libav stack.
- gdb / gdbserver / vim / debug tools.
- demo model binaries and NN examples.
- udev hwdb, fonts, cursors, icons, pixmaps, misc desktop payload.

Any future feature that depends on these must update the prune script.

## 8. Validation Definition

Minimum local checks before handing back:

```bash
sh -n buildroot/board/cvitek/SG200X/aikb_post_build_prune.sh
git diff --check
```

Preferred user-side board validation after pack/burn:

```sh
df -h
tail -n 120 /tmp/aikb_lcd_ui.log
```

Visual validation:

- No boot animation after power-on.
- Sleep animation appears after 3 minutes idle on non-session pages.
- Session picker and terminal are not covered by sleep.
- Pressed key title persists and does not auto-change to internal scene title.
- `RUN` page no longer displays OTA/update/progress semantics.
- Terminal/session display still works after rootfs pruning.

## 9. Known Risks

- 900M rootfs is sized for approximately 1GB storage, but actual SD NAND usable capacity must be checked against the vendor part.
- Post-build pruning is intentionally aggressive; future Python/Qt/OpenCV/ffmpeg features will fail unless the prune list is updated.
- UI and overlay binaries can drift from source if only one side is rebuilt or copied.
- A successful static check does not mean the image has been packed, burned, or verified on hardware.
- Storage reduction does not automatically prove lower DDR runtime usage.

## 10. Next Product Questions

- Final storage medium: TF card, SD NAND, or both?
- Minimum safe capacity target: decimal 1GB, binary 1GiB, or smaller?
- Whether sleep should dim/backlight-off in addition to animation.
- Whether boot should show a static splash instead of no animation.
- Whether AI chat-to-key-function definition belongs on host, board, or both.
