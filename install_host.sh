#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
WRAPPER_BIN="${ROOT}/bin"
USER_BIN="${VIBE_BRIDGE_USER_BIN:-${HOME}/.local/bin}"
REAL_BIN="${VIBE_BRIDGE_REAL_BIN:-${HOME}/.local/share/vibe-bridge/real-bin}"

MARKER_BEGIN="# >>> vibe-bridge host wrappers >>>"
MARKER_END="# <<< vibe-bridge host wrappers <<<"

die() {
  echo "error: $*" >&2
  exit 1
}

usage() {
  cat <<EOF
usage: $(basename "$0") [--install|--check|--uninstall]

  --install     install host wrappers (default)
  --check       report wrapper/PATH/real-CLI status without changing files
  --uninstall   remove vibe-bridge wrappers and restore saved CLI entries
EOF
}

need_file() {
  [ -f "$1" ] || die "missing required file: $1"
}

check_python() {
  command -v python3 >/dev/null 2>&1 || die "python3 is required"
  python3 - "$@" <<'PY'
import sys
if sys.version_info < (3, 9):
    raise SystemExit("error: python3 >= 3.9 is required")
PY
}

pick_rc_file() {
  if [ -n "${VIBE_BRIDGE_RC:-}" ]; then
    echo "${VIBE_BRIDGE_RC}"
    return
  fi

  case "${SHELL:-}" in
    */zsh) echo "${HOME}/.zshrc" ;;
    *) echo "${HOME}/.bashrc" ;;
  esac
}

realpath_or_empty() {
  if command -v realpath >/dev/null 2>&1; then
    realpath "$1" 2>/dev/null || true
  else
    readlink -f "$1" 2>/dev/null || true
  fi
}

preserve_existing_entry() {
  local name="$1"
  local target="${USER_BIN}/${name}"
  local wrapper="${WRAPPER_BIN}/${name}"
  local wrapper_real target_real backup

  wrapper_real="$(realpath_or_empty "${wrapper}")"
  target_real="$(realpath_or_empty "${target}")"

  if [ ! -e "${target}" ] && [ ! -L "${target}" ]; then
    return
  fi

  if [ -n "${wrapper_real}" ] && [ "${target_real}" = "${wrapper_real}" ]; then
    return
  fi

  mkdir -p "${REAL_BIN}"

  if [ -L "${target}" ]; then
    if [ -n "${target_real}" ]; then
      ln -sfn "${target_real}" "${REAL_BIN}/${name}"
      echo "saved existing ${name} symlink target: ${REAL_BIN}/${name} -> ${target_real}"
    fi
    return
  fi

  if [ -f "${target}" ]; then
    if [ -e "${REAL_BIN}/${name}" ] || [ -L "${REAL_BIN}/${name}" ]; then
      backup="${REAL_BIN}/${name}.$(date +%Y%m%d%H%M%S)"
      mv "${target}" "${backup}"
      echo "moved existing ${name} to ${backup}"
    else
      mv "${target}" "${REAL_BIN}/${name}"
      chmod +x "${REAL_BIN}/${name}" 2>/dev/null || true
      echo "moved existing ${name} to ${REAL_BIN}/${name}"
    fi
  fi
}

install_wrapper() {
  local name="$1"
  local wrapper="${WRAPPER_BIN}/${name}"

  need_file "${wrapper}"
  preserve_existing_entry "${name}"
  mkdir -p "${USER_BIN}"
  ln -sfn "${wrapper}" "${USER_BIN}/${name}"
  echo "installed wrapper: ${USER_BIN}/${name} -> ${wrapper}"
}

ensure_path_block() {
  local rc="$1"
  local block

  mkdir -p "$(dirname "${rc}")"
  touch "${rc}"

  if grep -qF "${MARKER_BEGIN}" "${rc}"; then
    echo "PATH block already present in ${rc}"
    return
  fi

  block="${MARKER_BEGIN}
export PATH=\"${USER_BIN}:${REAL_BIN}:\$PATH\"
${MARKER_END}"

  {
    echo
    echo "${block}"
  } >> "${rc}"

  echo "added PATH block to ${rc}"
}

remove_path_block() {
  local rc="$1"
  local tmp

  if [ ! -f "${rc}" ]; then
    echo "rc file not found: ${rc}"
    return
  fi
  if ! grep -qF "${MARKER_BEGIN}" "${rc}"; then
    echo "PATH block not present in ${rc}"
    return
  fi

  tmp="$(mktemp)"
  awk -v begin="${MARKER_BEGIN}" -v end="${MARKER_END}" '
    $0 == begin {skip=1; next}
    $0 == end {skip=0; next}
    !skip {print}
  ' "${rc}" > "${tmp}"
  mv "${tmp}" "${rc}"
  echo "removed PATH block from ${rc}"
}

find_real_cli() {
  local name="$1"
  local lookup_path="${USER_BIN}:${REAL_BIN}:${PATH}"
  local wrapper_real candidate candidate_real dir
  local -a path_dirs

  wrapper_real="$(realpath_or_empty "${WRAPPER_BIN}/${name}")"
  IFS=':' read -r -a path_dirs <<< "${lookup_path}"
  for dir in "${path_dirs[@]}"; do
    [ -n "${dir}" ] || dir="."
    candidate="${dir}/${name}"
    [ -f "${candidate}" ] && [ -x "${candidate}" ] || continue
    candidate_real="$(realpath_or_empty "${candidate}")"
    if [ -n "${candidate_real}" ] && [ "${candidate_real}" != "${wrapper_real}" ]; then
      echo "${candidate}"
      return 0
    fi
  done
  return 1
}

wrapper_status() {
  local name="$1"
  local target="${USER_BIN}/${name}"
  local wrapper="${WRAPPER_BIN}/${name}"
  local wrapper_real target_real found_real

  wrapper_real="$(realpath_or_empty "${wrapper}")"
  target_real="$(realpath_or_empty "${target}")"

  if [ -e "${target}" ] || [ -L "${target}" ]; then
    if [ -n "${wrapper_real}" ] && [ "${target_real}" = "${wrapper_real}" ]; then
      echo "${name}: wrapper installed at ${target}"
    else
      echo "${name}: ${target} exists but is not this wrapper"
    fi
  else
    echo "${name}: wrapper not installed at ${target}"
  fi

  found_real="$(find_real_cli "${name}" || true)"
  if [ -n "${found_real}" ]; then
    echo "${name}: real CLI candidate: ${found_real}"
  else
    echo "${name}: no real CLI found behind wrapper"
  fi
}

check_cli_visibility() {
  local name="$1"
  local wrapper found_real

  wrapper="$(PATH="${USER_BIN}:${REAL_BIN}:${PATH}" command -v "${name}" || true)"
  if [ -z "${wrapper}" ]; then
    echo "warning: ${name} is not currently visible on PATH" >&2
    return
  fi
  echo "${name} resolves to wrapper: ${wrapper}"

  found_real="$(find_real_cli "${name}" || true)"
  if [ -z "${found_real}" ]; then
    echo "warning: no real ${name} found behind the wrapper; install the real ${name} CLI or put it later on PATH" >&2
    return
  fi
  echo "${name} real CLI candidate: ${found_real}"
}

uninstall_wrapper() {
  local name="$1"
  local target="${USER_BIN}/${name}"
  local wrapper="${WRAPPER_BIN}/${name}"
  local saved="${REAL_BIN}/${name}"
  local wrapper_real target_real

  wrapper_real="$(realpath_or_empty "${wrapper}")"
  target_real="$(realpath_or_empty "${target}")"

  if [ -e "${target}" ] || [ -L "${target}" ]; then
    if [ -n "${wrapper_real}" ] && [ "${target_real}" = "${wrapper_real}" ]; then
      rm -f "${target}"
      echo "removed wrapper: ${target}"
    else
      echo "left non-wrapper entry untouched: ${target}"
    fi
  fi

  if [ ! -e "${target}" ] && [ ! -L "${target}" ] && { [ -e "${saved}" ] || [ -L "${saved}" ]; }; then
    mkdir -p "${USER_BIN}"
    mv "${saved}" "${target}"
    echo "restored saved ${name}: ${target}"
  fi
}

check_install() {
  local rc_file

  check_python
  need_file "${ROOT}/src/vibe_bridge/main.py"

  echo "vibe-bridge host wrapper check"
  echo "repo     : ${ROOT}"
  echo "user bin : ${USER_BIN}"
  echo "real bin : ${REAL_BIN}"

  rc_file="$(pick_rc_file)"
  echo "rc file  : ${rc_file}"
  if [ -f "${rc_file}" ] && grep -qF "${MARKER_BEGIN}" "${rc_file}"; then
    echo "PATH block: present"
  else
    echo "PATH block: missing"
  fi

  wrapper_status codex
  wrapper_status claude
}

install_all() {
  check_python
  need_file "${ROOT}/src/vibe_bridge/main.py"

  install_wrapper codex
  install_wrapper claude

  rc_file="$(pick_rc_file)"
  ensure_path_block "${rc_file}"

  check_cli_visibility codex
  check_cli_visibility claude

  echo
  echo "vibe-bridge host wrappers installed."
  echo "Open a new terminal, or run:"
  echo "  source ${rc_file}"
  echo
  echo "Then verify:"
  echo "  command -v codex"
  echo "  command -v claude"
  echo "  PYTHONPATH=${ROOT}/src python3 -m vibe_bridge.main hid list"
}

uninstall_all() {
  local rc_file

  uninstall_wrapper codex
  uninstall_wrapper claude

  rc_file="$(pick_rc_file)"
  remove_path_block "${rc_file}"

  echo
  echo "vibe-bridge host wrappers uninstalled."
  echo "Open a new terminal, or run:"
  echo "  source ${rc_file}"
}

main() {
  local mode="${1:---install}"
  case "${mode}" in
    --install) install_all ;;
    --check) check_install ;;
    --uninstall) uninstall_all ;;
    -h|--help) usage ;;
    *) usage >&2; die "unknown option: ${mode}" ;;
  esac
}

main "$@"
