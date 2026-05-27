---
name: aikb-board-control-loop
description: Use this skill when taking over AIKB board-side work in /home/rv_nano/AIKB/LicheeRV-Nano-Build, especially LCD UI, AKIM resources, sleep/boot behavior, HID/session key flow, Buildroot rootfs size, SD/TF/SD NAND image sizing, or handoff/memory updates for the AIKB product. It enforces a closed-loop embedded workflow, protects the user's pack_rootfs/pack_burn_image boundary, and records board-specific pitfalls.
---

# AIKB Board Control Loop

Use this for AIKB board work, not general host-only `vibe-bridge` installer work.

## Intake

Start read-only:

1. Read:
   - `/home/rv_nano/AIKB/AGENTS.md`
   - `/home/rv_nano/AIKB/CLAUDE.md`
   - `/home/rv_nano/AIKB/HANDOFF.md`
   - `/home/rv_nano/AIKB/C_context/KNOWN_FAILURES.md`
   - `/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge/C_context/MEMORY.md`
2. Run the project preflight specified by the repo rules. If ambiguous, use:
   - `/usr/bin/python3 /home/rv_nano/Sipeed/T_tools/agent_preflight.py --project AIKB`
3. Check:
   - `git -C /home/rv_nano/AIKB/LicheeRV-Nano-Build status --short`
   - relevant source, overlay, and generated binary drift.

## Report Before Editing

Use this shape:

```markdown
### 目标
- User-visible board behavior.

### 状态
- Current repo/config/UI/resource/image evidence.

### 误差
- Difference between target and current behavior.

### 控制动作
- Smallest variables to change.

### 反馈
- Static checks, build result, logs, image size, or user board test.

### 修正
- Next adjustment if feedback disagrees.

### 验证
- Exact command or board observation.

### 沉淀
- HANDOFF / MEMORY / skill update needed or not.
```

## Hard Boundaries

- Do not run `pack_rootfs && pack_burn_image`; the user owns this step.
- Do not claim a new `.img` exists unless the user or command output proves it.
- Do not treat TF / SD NAND storage size as DDR memory size.
- Do not leave temporary screen-test firmware in place after the user says to restore.
- Do not delete broad resources without checking references in `auto.sh`, board C source, overlay, target rootfs, and install rootfs.
- Keep board UI changes minimal and compatible with current behavior unless the user explicitly asks for a redesign.

## Board UI Rules

- Key-facing title should remain stable after a key press. Do not auto-replace `VOICE` with `LISTEN` or `RUN` with `UPDATING`.
- `RUN` must express running agent work, not OTA/update firmware progress.
- Non-session pages may enter sleep after local idle timeout.
- Session picker and terminal pages must stay active and should not be covered by sleep animation.
- Boot animation is optional product dressing; remove it if the user prioritizes size and startup simplicity.
- Sleep animation is a low-risk reuse path when a valid `vedio_sleep.akim` exists.

## Image Size Rules

When shrinking the image, align all three layers:

- Partition XML, e.g. `ROOTFS size_in_kb`.
- Buildroot ext image size, e.g. `BR2_TARGET_ROOTFS_EXT2_SIZE`.
- Actual target rootfs content via prune script or package config.

For approximately 1GB storage, prefer `900M` rootfs over `960M`; decimal 1GB media has less room than many people assume after boot partition and image overhead.

Post-build pruning is acceptable for clearly unused AIKB payloads, but document the contract. Current common candidates:

- Python runtime.
- Qt.
- OpenCV.
- ffmpeg / libav.
- gdb / vim / debug tools.
- demo model files.
- desktop fonts/icons/cursors/misc payload.

## Validation Ladder

Use the smallest check that proves the claim:

- Shell script edit:
  - `sh -n <script>`
- Whitespace / patch sanity:
  - `git diff --check`
- Rootfs sizing claim:
  - inspect partition XML and Buildroot ext size;
  - compare target rootfs size if available;
  - state that final image is not generated if pack was not run.
- Board behavior:
  - user burn/test required;
  - ask for `/tmp/aikb_lcd_ui.log`, `df -h`, and exact visual result.

## Handoff And Memory

At session end, update the top of:

- `/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge/HANDOFF.md`
- `/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge/C_context/MEMORY.md`

Include:

- goal, status, error, control actions, validation, next user command;
- what was not verified on hardware;
- the user's explicit boundaries;
- do-not-repeat pitfalls.
