use std::env;

use vb_daemon::{run_tcp_registration_server, BridgeDaemon};

#[cfg(windows)]
const DEFAULT_LCD_COLS: i16 = 78;
#[cfg(windows)]
const DEFAULT_LCD_ROWS: i16 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProductAction {
    Install,
    Uninstall,
}

fn main() {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("snapshot") => {
            let mut daemon = BridgeDaemon::new();
            match daemon.poll_once() {
                Ok(snapshot) => {
                    println!(
                        "daemon snapshot: sessions={} activities={} registered={}",
                        snapshot.sessions.len(),
                        snapshot.activities.len(),
                        snapshot.registered_agents
                    );
                    for session in snapshot.sessions {
                        println!(
                            "kind={} status={} id={} name={} hwnd={} cwd={}",
                            session.kind.as_str(),
                            session.status.as_str(),
                            compact(&session.agent_id),
                            compact(&session.name),
                            session
                                .terminal_hwnd
                                .map(|hwnd| format!("0x{hwnd:x}"))
                                .unwrap_or_else(|| "unbound".to_string()),
                            compact(&session.cwd)
                        );
                    }
                }
                Err(err) => {
                    eprintln!("snapshot failed: {err}");
                    std::process::exit(1);
                }
            }
        }
        Some("serve") => {
            let addr = args.next().unwrap_or_else(|| "127.0.0.1:18765".to_string());
            println!("vb-daemon registration ipc: tcp://{addr}");
            if let Err(err) = run_tcp_registration_server(&addr) {
                eprintln!("serve failed: {err}");
                std::process::exit(1);
            }
        }
        Some("serve-hid") => {
            let addr = args.next().unwrap_or_else(|| "127.0.0.1:18765".to_string());
            let device = args.next().unwrap_or_else(|| "auto".to_string());
            if let Err(err) = run_serve_hid(&addr, &device) {
                eprintln!("serve-hid failed: {err}");
                std::process::exit(1);
            }
        }
        Some("launch") => {
            let rest: Vec<String> = args.collect();
            match run_launch(rest) {
                Ok(code) => std::process::exit(code as i32),
                Err(err) => {
                    eprintln!("launch failed: {err}");
                    std::process::exit(1);
                }
            }
        }
        Some("start") => {
            let rest: Vec<String> = args.collect();
            match run_start(rest) {
                Ok(code) => std::process::exit(code as i32),
                Err(err) => {
                    eprintln!("start failed: {err}");
                    std::process::exit(1);
                }
            }
        }
        Some("terminal-shim") | Some("capture-shell") | Some("vibe-terminal") => {
            let rest: Vec<String> = args.collect();
            match run_terminal_shim(rest) {
                Ok(code) => std::process::exit(code as i32),
                Err(err) => {
                    eprintln!("terminal-shim failed: {err}");
                    std::process::exit(1);
                }
            }
        }
        Some("agent-shim") => {
            let rest: Vec<String> = args.collect();
            match run_agent_shim(rest) {
                Ok(code) => std::process::exit(code as i32),
                Err(err) => {
                    eprintln!("agent-shim failed: {err}");
                    std::process::exit(1);
                }
            }
        }
        Some("wsl-shim") => {
            let rest: Vec<String> = args.collect();
            match run_wsl_shim(rest) {
                Ok(code) => std::process::exit(code as i32),
                Err(err) => {
                    eprintln!("wsl-shim failed: {err}");
                    std::process::exit(1);
                }
            }
        }
        Some("install-windows") => {
            let rest: Vec<String> = args.collect();
            if let Err(err) = run_install_windows(rest) {
                eprintln!("install-windows failed: {err}");
                std::process::exit(1);
            }
        }
        Some("install-product") | Some("setup") => {
            let rest: Vec<String> = args.collect();
            if let Err(err) = run_install_product(rest) {
                eprintln!("install-product failed: {err}");
                std::process::exit(1);
            }
        }
        Some("uninstall-windows") => {
            let rest: Vec<String> = args.collect();
            if let Err(err) = run_uninstall_windows(rest) {
                eprintln!("uninstall-windows failed: {err}");
                std::process::exit(1);
            }
        }
        Some("uninstall-product") => {
            let rest: Vec<String> = args.collect();
            if let Err(err) = run_uninstall_product(rest) {
                eprintln!("uninstall-product failed: {err}");
                std::process::exit(1);
            }
        }
        Some("status-windows") => {
            let rest: Vec<String> = args.collect();
            if let Err(err) = run_status_windows(rest) {
                eprintln!("status-windows failed: {err}");
                std::process::exit(1);
            }
        }
        Some("help") | Some("--help") | Some("-h") => print_help(),
        None => {
            if let Some(action) = product_action_from_current_exe_name() {
                let result = match action {
                    ProductAction::Install => run_install_product(Vec::new()),
                    ProductAction::Uninstall => run_uninstall_product(Vec::new()),
                };
                if let Err(err) = result {
                    eprintln!("{} failed: {err}", product_action_name(action));
                    wait_for_enter_after_product_action();
                    std::process::exit(1);
                }
            } else {
                print_help();
            }
        }
        Some(other) => {
            eprintln!("unknown command: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!("vb-daemon");
    println!();
    println!("Commands:");
    println!("  snapshot       print merged passive/registered agent sessions");
    println!("  serve [ADDR]   run JSONL registration IPC, default 127.0.0.1:18765");
    println!("  serve-hid [ADDR] [auto|PATH]");
    println!("  launch [--daemon ADDR] [--kind claude|codex] [--name NAME]");
    println!("                 [--cols N] [--rows N] [--follow-console-size]");
    println!("                 -- COMMAND [ARGS...]");
    println!("                 (Windows only) spawn COMMAND under a ConPTY,");
    println!("                 mirror stdout to the board via the running daemon.");
    println!("  start  [--port N] [--device auto|PATH] [--kind claude|codex]");
    println!("                 [--name NAME] [--cols N] [--rows N] [--follow-console-size]");
    println!("                 -- COMMAND [ARGS...]");
    println!("                 (Windows only) one-process all-in-one: opens HID,");
    println!("                 binds TCP IPC, spawns COMMAND under ConPTY and mirrors");
    println!("                 stdout to the board. Use this for single-window setup.");
    println!("  terminal-shim [--daemon ADDR] [--name NAME] [--cols N] [--rows N]");
    println!("                 [--follow-console-size]");
    println!("                 [--cmdline-b64 B64 | -- COMMAND [ARGS...]]");
    println!("                 (Windows only) capture an entire shell/profile through");
    println!("                 ConPTY without replacing codex/claude binaries.");
    println!("  agent-shim codex|claude [ARGS...]");
    println!("                 (Windows only) user-facing shim entry. Ensures daemon,");
    println!("                 then launches the real agent under ConPTY capture.");
    println!("  wsl-shim [WSL ARGS...]");
    println!("                 (Windows only) transient captured-terminal WSL entry.");
    println!("  install-windows [--addr ADDR] [--device auto|PATH]");
    println!("                 [--wsl] [--wsl-distro NAME] [--no-startup]");
    println!("                 [--terminal-profiles] [--no-wsl-shortcuts]");
    println!("                 [--wsl-shell] [--management-shortcuts]");
    println!("                 [--no-path] [--shim-dir DIR]");
    println!("                 (Windows only) install Startup daemon script and shims.");
    println!("  install-product [same flags as install-windows]");
    println!("                 (Windows only) product install: Startup daemon,");
    println!("                 shims, Start Menu management, and WSL shell hooks.");
    println!("  uninstall-windows [--addr ADDR] [--shim-dir DIR]");
    println!("                 [--no-terminal-profiles] [--purge]");
    println!("                 (Windows only) stop background daemon and remove shims.");
    println!("  uninstall-product [same flags as uninstall-windows]");
    println!("                 (Windows only) product uninstall/restore.");
    println!("  status-windows [--addr ADDR]");
    println!("                 (Windows only) print install paths and daemon health.");
}

fn compact(value: &str) -> String {
    value.replace('\r', " ").replace('\n', " ")
}

#[cfg(windows)]
fn product_step(label: &str) {
    use std::io::Write;

    println!("[vibe-bridge] {label}");
    let _ = std::io::stdout().flush();
}

fn product_action_from_current_exe_name() -> Option<ProductAction> {
    let exe = std::env::current_exe().ok()?;
    let stem = exe.file_stem()?.to_string_lossy();
    product_action_from_exe_name(&stem)
}

fn product_action_from_exe_name(name: &str) -> Option<ProductAction> {
    let lower = name.to_ascii_lowercase();
    if lower.contains("uninstall") {
        Some(ProductAction::Uninstall)
    } else if lower.contains("setup") || lower.contains("installer") {
        Some(ProductAction::Install)
    } else {
        None
    }
}

fn product_action_name(action: ProductAction) -> &'static str {
    match action {
        ProductAction::Install => "install-product",
        ProductAction::Uninstall => "uninstall-product",
    }
}

fn product_install_args(mut raw: Vec<String>) -> Vec<String> {
    let has_terminal_profile_choice = raw
        .iter()
        .any(|arg| arg == "--terminal-profiles" || arg == "--no-terminal-profiles");
    if !has_terminal_profile_choice {
        raw.push("--no-terminal-profiles".to_string());
    }
    let has_management_shortcut_choice = raw
        .iter()
        .any(|arg| arg == "--management-shortcuts" || arg == "--no-management-shortcuts");
    if !has_management_shortcut_choice {
        raw.push("--management-shortcuts".to_string());
    }
    let has_wsl_shell_choice = raw
        .iter()
        .any(|arg| arg == "--wsl-shell" || arg == "--no-wsl-shell");
    if !has_wsl_shell_choice {
        raw.push("--wsl-shell".to_string());
    }
    raw
}

#[cfg(windows)]
fn wait_for_enter_after_product_action() {
    if std::env::var_os("VIBE_BRIDGE_NO_PAUSE").is_some() {
        return;
    }
    println!();
    println!("Setup failed. Press Enter to close this window.");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

#[cfg(not(windows))]
fn wait_for_enter_after_product_action() {}

#[cfg(windows)]
fn run_serve_hid(addr: &str, device: &str) -> Result<(), String> {
    use std::sync::Arc;

    use vb_daemon::run_tcp_hid_daemon;
    use vb_transport::{resolve_win_hid_device, ReopenWinHidTransport};

    if device == "auto" {
        match resolve_win_hid_device().map_err(|err| err.to_string())? {
            Some(path) => println!("vb-daemon HID: {path}"),
            None => eprintln!("vb-daemon HID: auto (waiting for Vibe HID 359f:2120)"),
        }
    } else {
        println!("vb-daemon HID: {device}");
    }
    println!("vb-daemon registration ipc: tcp://{addr}");
    let hid = Arc::new(ReopenWinHidTransport::open_lazy(device));
    run_tcp_hid_daemon(addr, hid)
}

#[cfg(not(windows))]
fn run_serve_hid(_addr: &str, _device: &str) -> Result<(), String> {
    Err("serve-hid is currently only implemented for native Windows".to_string())
}

#[cfg(windows)]
#[derive(Debug, Default)]
struct LaunchOpts {
    daemon_addr: Option<String>,
    kind: Option<String>,
    name: Option<String>,
    cols: i16,
    rows: i16,
    follow_console_size: bool,
    command: Vec<String>,
}

#[cfg(windows)]
fn parse_launch_args(raw: Vec<String>) -> Result<LaunchOpts, String> {
    let mut opts = LaunchOpts {
        cols: DEFAULT_LCD_COLS,
        rows: DEFAULT_LCD_ROWS,
        ..LaunchOpts::default()
    };
    let mut iter = raw.into_iter();
    let mut command_started = false;
    while let Some(arg) = iter.next() {
        if command_started {
            opts.command.push(arg);
            continue;
        }
        match arg.as_str() {
            "--" => command_started = true,
            "--daemon" => opts.daemon_addr = Some(iter.next().ok_or("--daemon needs ADDR")?),
            "--kind" => opts.kind = Some(iter.next().ok_or("--kind needs KIND")?),
            "--name" => opts.name = Some(iter.next().ok_or("--name needs NAME")?),
            "--cols" => {
                opts.cols = iter
                    .next()
                    .ok_or("--cols needs N")?
                    .parse()
                    .map_err(|err| format!("--cols invalid: {err}"))?
            }
            "--rows" => {
                opts.rows = iter
                    .next()
                    .ok_or("--rows needs N")?
                    .parse()
                    .map_err(|err| format!("--rows invalid: {err}"))?
            }
            "--follow-console-size" => opts.follow_console_size = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown launch flag: {other}"));
            }
            _ => {
                // First positional argument is the command, even without `--`.
                opts.command.push(arg);
                command_started = true;
            }
        }
    }
    if opts.command.is_empty() {
        return Err("missing COMMAND to launch (use `-- claude` or `-- codex`)".to_string());
    }
    Ok(opts)
}

#[cfg(windows)]
fn infer_kind(command_argv0: &str) -> &'static str {
    let lower = std::path::Path::new(command_argv0)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    if lower.contains("codex") {
        "codex"
    } else {
        "claude"
    }
}

#[cfg(windows)]
fn effective_conpty_size(
    requested_cols: i16,
    requested_rows: i16,
    follow_console_size: bool,
) -> (i16, i16) {
    if follow_console_size {
        if let Some((cols, rows)) = current_console_window_size() {
            return (cols, rows);
        }
    }
    (requested_cols, requested_rows)
}

#[cfg(windows)]
fn current_console_window_size() -> Option<(i16, i16)> {
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{
        GetConsoleScreenBufferInfo, GetStdHandle, CONSOLE_SCREEN_BUFFER_INFO, STD_OUTPUT_HANDLE,
    };

    fn size_from_handle(handle: HANDLE) -> Option<(i16, i16)> {
        if handle == std::ptr::null_mut() || handle == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
        if unsafe { GetConsoleScreenBufferInfo(handle, &mut info) } == 0 {
            return None;
        }
        let cols = info.srWindow.Right - info.srWindow.Left + 1;
        let rows = info.srWindow.Bottom - info.srWindow.Top + 1;
        if cols >= DEFAULT_LCD_COLS && rows >= DEFAULT_LCD_ROWS {
            Some((cols, rows))
        } else {
            None
        }
    }

    size_from_handle(unsafe { GetStdHandle(STD_OUTPUT_HANDLE) })
}

#[cfg(windows)]
fn start_conpty_resize_watcher(
    session: std::sync::Arc<vb_daemon::conpty::ConPtySession>,
    initial_cols: i16,
    initial_rows: i16,
    enabled: bool,
) -> Option<ConPtyResizeWatcher> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    if !enabled {
        return None;
    }

    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let mut last_size = (initial_cols, initial_rows);
        let mut last_error_size = None;
        while !thread_stop.load(Ordering::Relaxed) {
            if let Some(size) = current_console_window_size() {
                if size != last_size {
                    match session.resize(size.0, size.1) {
                        Ok(()) => {
                            append_terminal_shim_log(format!(
                                "[launch] conpty resized cols={} rows={}",
                                size.0, size.1
                            ));
                            last_size = size;
                            last_error_size = None;
                        }
                        Err(err) if last_error_size != Some(size) => {
                            append_terminal_shim_log(format!(
                                "[launch] conpty resize failed cols={} rows={} err={}",
                                size.0,
                                size.1,
                                terminal_shim_log_value(&err.to_string())
                            ));
                            last_error_size = Some(size);
                        }
                        Err(_) => {}
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    });
    Some(ConPtyResizeWatcher {
        stop,
        handle: Some(handle),
    })
}

#[cfg(windows)]
struct ConPtyResizeWatcher {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

#[cfg(windows)]
impl Drop for ConPtyResizeWatcher {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;

        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(windows)]
struct EnvVarGuard {
    key: String,
    previous: Option<std::ffi::OsString>,
}

#[cfg(windows)]
impl EnvVarGuard {
    fn empty() -> Self {
        Self {
            key: String::new(),
            previous: None,
        }
    }

    fn set(key: &str, value: &str) -> Result<Self, std::env::VarError> {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Ok(Self {
            key: key.to_string(),
            previous,
        })
    }
}

#[cfg(windows)]
impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if self.key.is_empty() {
            return;
        }
        if let Some(previous) = self.previous.take() {
            std::env::set_var(&self.key, previous);
        } else {
            std::env::remove_var(&self.key);
        }
    }
}

#[cfg(windows)]
struct TerminalChildEnvGuard {
    _guards: Vec<EnvVarGuard>,
    runtime_shim_dir: Option<std::path::PathBuf>,
}

#[cfg(windows)]
impl TerminalChildEnvGuard {
    fn empty() -> Self {
        Self {
            _guards: Vec::new(),
            runtime_shim_dir: None,
        }
    }

    fn enter(addr: &str, terminal_agent_id: &str) -> Result<Self, String> {
        let mut guards = Vec::new();
        let mut runtime_shim_dir = None;
        if let Some(shim_dir) = create_captured_terminal_runtime_shims(terminal_agent_id)? {
            let path = std::env::var_os("PATH").unwrap_or_default();
            let mut paths = std::env::split_paths(&path).collect::<Vec<_>>();
            paths.insert(0, shim_dir.clone());
            if let Ok(joined) = std::env::join_paths(paths) {
                append_terminal_shim_log(format!(
                    "[launch] child PATH prepended runtime_shim_dir={}",
                    terminal_shim_log_value(&shim_dir.to_string_lossy())
                ));
                guards.push(
                    EnvVarGuard::set("PATH", &joined.to_string_lossy())
                        .map_err(|err| err.to_string())?,
                );
                runtime_shim_dir = Some(shim_dir);
            }
        }
        if let Some(shim_dir) = current_exe_dir_if_agent_shims_exist() {
            let path = std::env::var_os("PATH").unwrap_or_default();
            let mut paths = std::env::split_paths(&path).collect::<Vec<_>>();
            if !paths.iter().any(|path| same_path(path, &shim_dir)) {
                paths.insert(0, shim_dir.clone());
                if let Ok(joined) = std::env::join_paths(paths) {
                    append_terminal_shim_log(format!(
                        "[launch] child PATH prepended shim_dir={}",
                        terminal_shim_log_value(&shim_dir.to_string_lossy())
                    ));
                    guards.push(
                        EnvVarGuard::set("PATH", &joined.to_string_lossy())
                            .map_err(|err| err.to_string())?,
                    );
                }
            }
        }
        guards.push(EnvVarGuard::set("VIBE_BRIDGE_DAEMON", addr).map_err(|err| err.to_string())?);
        guards.push(
            EnvVarGuard::set("VIBE_BRIDGE_TERMINAL_AGENT_ID", terminal_agent_id)
                .map_err(|err| err.to_string())?,
        );
        guards.push(
            EnvVarGuard::set("VIBE_BRIDGE_TERMINAL_KIND", "terminal")
                .map_err(|err| err.to_string())?,
        );
        Ok(Self {
            _guards: guards,
            runtime_shim_dir,
        })
    }
}

#[cfg(windows)]
impl Drop for TerminalChildEnvGuard {
    fn drop(&mut self) {
        if let Some(dir) = self.runtime_shim_dir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

#[cfg(windows)]
fn current_exe_dir_if_agent_shims_exist() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    if dir.join("codex.cmd").is_file() || dir.join("claude.cmd").is_file() {
        Some(dir)
    } else {
        None
    }
}

#[cfg(windows)]
fn create_captured_terminal_runtime_shims(
    terminal_agent_id: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    let exe = std::env::current_exe().map_err(|err| format!("current_exe: {err}"))?;
    let safe_id = sanitize_runtime_shim_id(terminal_agent_id);
    let dir = std::env::temp_dir()
        .join("vibe-bridge")
        .join("captured-terminal")
        .join(format!("{}-{}", safe_id, std::process::id()));
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("create runtime shim dir {}: {err}", dir.display()))?;
    let content = format!(
        "@echo off\r\n\
         setlocal\r\n\
         \"{}\" wsl-shim %*\r\n\
         exit /b %ERRORLEVEL%\r\n",
        shell_display_path(&exe)
    );
    let path = dir.join("wsl.cmd");
    std::fs::write(&path, content).map_err(|err| format!("write {}: {err}", path.display()))?;
    Ok(Some(dir))
}

#[cfg(windows)]
fn sanitize_runtime_shim_id(value: &str) -> String {
    let mut out = String::with_capacity(value.len().max(1));
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "terminal".to_string()
    } else {
        out
    }
}

#[cfg(windows)]
fn run_launch(raw: Vec<String>) -> Result<u32, String> {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
    use std::sync::{Arc, Mutex};

    use vb_daemon::{conpty::ConPtySession, hex_encode};

    let opts = parse_launch_args(raw)?;
    let addr = opts
        .daemon_addr
        .clone()
        .unwrap_or_else(|| "127.0.0.1:8765".to_string());
    let kind = opts
        .kind
        .clone()
        .unwrap_or_else(|| infer_kind(&opts.command[0]).to_string());
    let pid = std::process::id();
    let agent_id = format!("launch-{pid}");
    let base_name = opts.name.clone().unwrap_or_else(|| {
        std::path::Path::new(&opts.command[0])
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("launch")
            .to_string()
    });
    let name = if kind == "terminal" {
        format!("{base_name} #{pid}")
    } else {
        base_name
    };
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|path| path.to_str().map(String::from))
        .unwrap_or_default();

    append_terminal_shim_log(format!(
        "[launch] daemon={} kind={} agent_id={} name={}",
        terminal_shim_log_value(&addr),
        terminal_shim_log_value(&kind),
        terminal_shim_log_value(&agent_id),
        terminal_shim_log_value(&name)
    ));

    let write_half =
        TcpStream::connect(&addr).map_err(|err| format!("connect daemon at {addr}: {err}"))?;
    write_half
        .set_nodelay(true)
        .map_err(|err| format!("set_nodelay: {err}"))?;
    // Split: writer (mutex-guarded so multiple threads can interleave whole
    // lines safely) and reader (single thread drains ack lines).
    let read_half = write_half
        .try_clone()
        .map_err(|err| format!("clone tcp stream: {err}"))?;
    let writer = Arc::new(Mutex::new(write_half));

    // Background ack drainer: daemon writes one JSON line per request; we do
    // not need the response content, but we must read it so the socket
    // receive buffer never fills up and blocks the daemon writer.
    std::thread::spawn(move || {
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    });

    let follow_console_size = opts.follow_console_size;
    let (conpty_cols, conpty_rows) =
        effective_conpty_size(opts.cols, opts.rows, follow_console_size);
    append_terminal_shim_log(format!(
        "[launch] conpty size cols={} rows={} requested_cols={} requested_rows={} follow_console_size={}",
        conpty_cols, conpty_rows, opts.cols, opts.rows, follow_console_size
    ));

    let command = if kind == "terminal" {
        maybe_wrap_wsl_terminal_command(&opts.command, &addr, &agent_id)
    } else {
        opts.command.clone()
    };
    let argv: Vec<std::ffi::OsString> = command.iter().map(Into::into).collect();
    let _terminal_child_env = if kind == "terminal" {
        TerminalChildEnvGuard::enter(&addr, &agent_id).map_err(|err| err.to_string())?
    } else {
        TerminalChildEnvGuard::empty()
    };
    let _agent_child_env = if kind == "terminal" {
        EnvVarGuard::empty()
    } else {
        EnvVarGuard::set("VIBE_BRIDGE_LAUNCH_AGENT_ID", &agent_id).map_err(|err| err.to_string())?
    };
    let _captured_env =
        EnvVarGuard::set("VIBE_BRIDGE_CAPTURED_TERMINAL", "1").map_err(|err| err.to_string())?;
    let session = ConPtySession::spawn(&argv, conpty_cols, conpty_rows)
        .map_err(|err| format_launch_spawn_error(&opts.command[0], err))?;
    let session = Arc::new(session);
    let resize_watcher = start_conpty_resize_watcher(
        Arc::clone(&session),
        conpty_cols,
        conpty_rows,
        follow_console_size,
    );
    let _console_mode = ConsoleModeGuard::enter_passthrough();
    let console_input_handle = _console_mode.input_read_handle();

    let register_json = serde_json::json!({
        "type": "agent.register",
        "agent": {
            "agentId": &agent_id,
            "kind": &kind,
            "name": &name,
            "cwd": &cwd,
            // Tells the daemon this agent owns its ConPTY here — board
            // terminal-view replay should clear the screen for live VT100
            // repaint instead of dumping turn-text history.
            "fromLaunch": true,
        }
    });
    send_json_line(&writer, &register_json.to_string())?;

    // Stdout pump: ConPTY → (our stdout, daemon TCP, fire-and-forget).
    let stdout_session = Arc::clone(&session);
    let stdout_writer = Arc::clone(&writer);
    let stdout_agent_id = agent_id.clone();
    let stdout_kind = kind.clone();
    let stdout_thread = std::thread::spawn(move || -> Result<(), String> {
        let mut buf = vec![0u8; 4096];
        let mut stdout = std::io::stdout().lock();
        let mut daemon_stream_enabled = true;
        loop {
            let n = stdout_session
                .read_output(&mut buf)
                .map_err(|err| format!("read ConPTY: {err}"))?;
            if n == 0 {
                return Ok(());
            }
            let chunk = &buf[..n];
            let _ = stdout.write_all(chunk);
            let _ = stdout.flush();
            let envelope = serde_json::json!({
                "type": "terminal.stream",
                "stream": {
                    "agentId": &stdout_agent_id,
                    "kind": &stdout_kind,
                    "dataHex": hex_encode(chunk),
                }
            });
            if daemon_stream_enabled {
                if let Err(err) = send_json_line(&stdout_writer, &envelope.to_string()) {
                    append_terminal_shim_log(format!(
                        "[launch] terminal.stream disabled after send failed: {}",
                        terminal_shim_log_value(&err)
                    ));
                    daemon_stream_enabled = false;
                }
            }
        }
    });

    // Stdin pump: our stdin → ConPTY.
    let stdin_session = Arc::clone(&session);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut buf = [0u8; 1024];
        append_terminal_shim_log(format!(
            "[launch] stdin reader source={}",
            if console_input_handle.is_some() {
                "console"
            } else {
                "stdin"
            }
        ));
        loop {
            let n = match console_input_handle {
                Some(handle) => match read_console_input_handle(handle, &mut buf) {
                    Ok(0) => return,
                    Ok(n) => n,
                    Err(err) => {
                        append_terminal_shim_log(format!(
                            "[launch] console stdin read failed: {}",
                            terminal_shim_log_value(&err.to_string())
                        ));
                        return;
                    }
                },
                None => match stdin.lock().read(&mut buf) {
                    Ok(0) => return,
                    Ok(n) => n,
                    Err(err) => {
                        append_terminal_shim_log(format!(
                            "[launch] stdin read failed: {}",
                            terminal_shim_log_value(&err.to_string())
                        ));
                        return;
                    }
                },
            };
            if let Err(err) = stdin_session.write_input(&buf[..n]) {
                append_terminal_shim_log(format!(
                    "[launch] stdin forward failed: {}",
                    terminal_shim_log_value(&err.to_string())
                ));
                return;
            }
        }
    });

    let exit_code = session.wait().map_err(|err| format!("wait child: {err}"))?;
    drop(resize_watcher);
    let _ = stdout_thread.join();

    let abort_json = serde_json::json!({
        "type": "session.abort",
        "abort": { "agentId": &agent_id, "kind": &kind },
    });
    let _ = send_json_line(&writer, &abort_json.to_string());

    Ok(exit_code)
}

#[cfg(not(windows))]
fn run_launch(_raw: Vec<String>) -> Result<u32, String> {
    Err("launch is only implemented on native Windows".to_string())
}

#[cfg(windows)]
struct ConsoleModeGuard {
    input: Option<ConsoleModeRestore>,
    output: Option<ConsoleModeRestore>,
}

#[cfg(windows)]
struct ConsoleModeRestore {
    handle: windows_sys::Win32::Foundation::HANDLE,
    mode: u32,
    close_on_drop: bool,
}

#[cfg(windows)]
impl ConsoleModeGuard {
    fn enter_passthrough() -> Self {
        use windows_sys::Win32::System::Console::{
            ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
            ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_INPUT_HANDLE,
            STD_OUTPUT_HANDLE,
        };

        unsafe {
            let mut guard = Self {
                input: None,
                output: None,
            };

            if let Some(input) =
                set_console_mode_for_std_or_device(STD_INPUT_HANDLE, "CONIN$", |mode| {
                    (mode & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT))
                        | ENABLE_VIRTUAL_TERMINAL_INPUT
                })
            {
                append_terminal_shim_log(format!(
                    "[launch] console input mode old=0x{:08x} new=0x{:08x} source={}",
                    input.mode,
                    input_applied_mode(input.mode),
                    if input.close_on_drop { "CONIN$" } else { "std" }
                ));
                guard.input = Some(input);
            } else {
                append_terminal_shim_log("[launch] console input mode unavailable");
            }

            if let Some(output) =
                set_console_mode_for_std_or_device(STD_OUTPUT_HANDLE, "CONOUT$", |mode| {
                    mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING
                })
            {
                append_terminal_shim_log(format!(
                    "[launch] console output mode old=0x{:08x} new=0x{:08x} source={}",
                    output.mode,
                    output.mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING,
                    if output.close_on_drop {
                        "CONOUT$"
                    } else {
                        "std"
                    }
                ));
                guard.output = Some(output);
            }

            guard
        }
    }

    fn input_read_handle(&self) -> Option<usize> {
        self.input.as_ref().map(|restore| restore.handle as usize)
    }
}

#[cfg(windows)]
fn read_console_input_handle(handle: usize, buf: &mut [u8]) -> std::io::Result<usize> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Storage::FileSystem::ReadFile;

    if buf.is_empty() {
        return Ok(0);
    }

    let mut read = 0u32;
    let ok = unsafe {
        ReadFile(
            handle as windows_sys::Win32::Foundation::HANDLE,
            buf.as_mut_ptr(),
            buf.len() as u32,
            &mut read,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(std::io::Error::from_raw_os_error(unsafe {
            GetLastError() as i32
        }));
    }
    Ok(read as usize)
}

#[cfg(windows)]
fn input_applied_mode(mode: u32) -> u32 {
    use windows_sys::Win32::System::Console::{
        ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT, ENABLE_VIRTUAL_TERMINAL_INPUT,
    };

    (mode & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT))
        | ENABLE_VIRTUAL_TERMINAL_INPUT
}

#[cfg(windows)]
unsafe fn set_console_mode_for_std_or_device(
    std_handle: u32,
    device: &str,
    transform: impl Fn(u32) -> u32,
) -> Option<ConsoleModeRestore> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::GetStdHandle;

    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;

    unsafe fn try_set(
        handle: HANDLE,
        close_on_drop: bool,
        transform: &impl Fn(u32) -> u32,
    ) -> Option<ConsoleModeRestore> {
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Console::{GetConsoleMode, SetConsoleMode};

        if handle == std::ptr::null_mut() || handle == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut mode = 0u32;
        if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
            if close_on_drop {
                unsafe {
                    let _ = CloseHandle(handle);
                }
            }
            return None;
        }
        let new_mode = transform(mode);
        if unsafe { SetConsoleMode(handle, new_mode) } == 0 {
            if close_on_drop {
                unsafe {
                    let _ = CloseHandle(handle);
                }
            }
            return None;
        }
        Some(ConsoleModeRestore {
            handle,
            mode,
            close_on_drop,
        })
    }

    let std = unsafe { GetStdHandle(std_handle) };
    if let Some(restore) = unsafe { try_set(std, false, &transform) } {
        return Some(restore);
    }

    let mut wide: Vec<u16> = device.encode_utf16().collect();
    wide.push(0);
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle == std::ptr::null_mut() || handle == INVALID_HANDLE_VALUE {
        return None;
    }
    unsafe { try_set(handle, true, &transform) }
}

#[cfg(windows)]
impl Drop for ConsoleModeGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Console::SetConsoleMode;

        unsafe {
            if let Some(restore) = self.input.take() {
                let _ = SetConsoleMode(restore.handle, restore.mode);
                if restore.close_on_drop {
                    let _ = CloseHandle(restore.handle);
                }
            }
            if let Some(restore) = self.output.take() {
                let _ = SetConsoleMode(restore.handle, restore.mode);
                if restore.close_on_drop {
                    let _ = CloseHandle(restore.handle);
                }
            }
        }
    }
}

#[cfg(windows)]
fn format_launch_spawn_error(command: &str, err: std::io::Error) -> String {
    let raw = err.to_string();
    if raw.contains("GetLastError=0x00000002") {
        return format!(
            "spawn ConPTY: command not found: {command}\n\
             Windows could not find `{command}` in PATH. If Codex is installed inside WSL, launch it through wsl.exe, for example:\n\
             cargo run -p vb-daemon -- launch --daemon 127.0.0.1:8765 --kind codex -- wsl.exe -d Ubuntu-22.04 -- bash -lc \"cd ~ && codex --sandbox workspace-write --ask-for-approval on-request --add-dir ~/Sipeed\""
        );
    }
    format!("spawn ConPTY: {raw}")
}

#[cfg(windows)]
fn send_json_line(
    writer: &std::sync::Arc<std::sync::Mutex<std::net::TcpStream>>,
    line: &str,
) -> Result<(), String> {
    use std::io::Write;
    let mut guard = writer
        .lock()
        .map_err(|_| "tcp writer mutex poisoned".to_string())?;
    guard
        .write_all(line.as_bytes())
        .map_err(|err| format!("tcp write: {err}"))?;
    guard
        .write_all(b"\n")
        .map_err(|err| format!("tcp newline: {err}"))?;
    guard.flush().map_err(|err| format!("tcp flush: {err}"))
}

#[cfg(windows)]
#[derive(Debug, Default)]
struct StartOpts {
    daemon_addr: Option<String>,
    device: String,
    kind: Option<String>,
    name: Option<String>,
    cols: i16,
    rows: i16,
    follow_console_size: bool,
    command: Vec<String>,
}

#[cfg(windows)]
fn parse_start_args(raw: Vec<String>) -> Result<StartOpts, String> {
    let mut opts = StartOpts {
        device: "auto".to_string(),
        cols: DEFAULT_LCD_COLS,
        rows: DEFAULT_LCD_ROWS,
        ..StartOpts::default()
    };
    let mut iter = raw.into_iter();
    let mut command_started = false;
    while let Some(arg) = iter.next() {
        if command_started {
            opts.command.push(arg);
            continue;
        }
        match arg.as_str() {
            "--" => command_started = true,
            "--daemon" => opts.daemon_addr = Some(iter.next().ok_or("--daemon needs ADDR")?),
            "--port" => {
                let port: u16 = iter
                    .next()
                    .ok_or("--port needs N")?
                    .parse()
                    .map_err(|err| format!("--port invalid: {err}"))?;
                opts.daemon_addr = Some(format!("127.0.0.1:{port}"));
            }
            "--device" => opts.device = iter.next().ok_or("--device needs auto|PATH")?,
            "--kind" => opts.kind = Some(iter.next().ok_or("--kind needs KIND")?),
            "--name" => opts.name = Some(iter.next().ok_or("--name needs NAME")?),
            "--cols" => {
                opts.cols = iter
                    .next()
                    .ok_or("--cols needs N")?
                    .parse()
                    .map_err(|err| format!("--cols invalid: {err}"))?
            }
            "--rows" => {
                opts.rows = iter
                    .next()
                    .ok_or("--rows needs N")?
                    .parse()
                    .map_err(|err| format!("--rows invalid: {err}"))?
            }
            "--follow-console-size" => opts.follow_console_size = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown start flag: {other}"));
            }
            _ => {
                opts.command.push(arg);
                command_started = true;
            }
        }
    }
    if opts.command.is_empty() {
        return Err("missing COMMAND to launch (use `-- claude` or `-- codex`)".to_string());
    }
    Ok(opts)
}

#[cfg(windows)]
fn run_start(raw: Vec<String>) -> Result<u32, String> {
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    use vb_daemon::run_tcp_hid_daemon;
    use vb_transport::{resolve_win_hid_device, ReopenWinHidTransport};

    let opts = parse_start_args(raw)?;
    let addr = opts
        .daemon_addr
        .clone()
        .unwrap_or_else(|| "127.0.0.1:8765".to_string());

    let device_path = if opts.device == "auto" {
        resolve_win_hid_device()
            .map_err(|err| err.to_string())?
            .ok_or_else(|| "no Vibe HID 359f:2120 device found".to_string())?
    } else {
        opts.device.clone()
    };
    eprintln!("vb-daemon start: HID={device_path}");
    eprintln!("vb-daemon start: IPC=tcp://{addr}");

    let hid = Arc::new(ReopenWinHidTransport::open(&device_path).map_err(|err| err.to_string())?);

    // Background: full HID daemon (TCP listener + HID rx/tx + heartbeat + passive discovery).
    {
        let addr = addr.clone();
        let hid = Arc::clone(&hid);
        thread::spawn(move || {
            if let Err(err) = run_tcp_hid_daemon(addr.as_str(), hid) {
                eprintln!("[vb-daemon start] HID daemon thread exited: {err}");
            }
        });
    }

    // Wait for the TCP listener to actually bind before launching the child,
    // otherwise the launch path's connect() races and 10061s.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if std::net::TcpStream::connect(&addr).is_ok() {
            break;
        }
        if Instant::now() > deadline {
            return Err(format!(
                "timeout waiting for daemon to bind {addr} (HID daemon thread crashed?)"
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
    eprintln!("vb-daemon start: daemon ready, spawning child");

    // Foreground: ConPTY-wrapped child. Reuses the existing launch code path so
    // stdin/stdout pumps and terminal.stream forwarding stay identical.
    let mut launch_args: Vec<String> = vec!["--daemon".to_string(), addr.clone()];
    if let Some(kind) = opts.kind {
        launch_args.push("--kind".to_string());
        launch_args.push(kind);
    }
    if let Some(name) = opts.name {
        launch_args.push("--name".to_string());
        launch_args.push(name);
    }
    launch_args.push("--cols".to_string());
    launch_args.push(opts.cols.to_string());
    launch_args.push("--rows".to_string());
    launch_args.push(opts.rows.to_string());
    if opts.follow_console_size {
        launch_args.push("--follow-console-size".to_string());
    }
    launch_args.push("--".to_string());
    launch_args.extend(opts.command);

    run_launch(launch_args)
}

#[cfg(not(windows))]
fn run_start(_raw: Vec<String>) -> Result<u32, String> {
    Err("start is only implemented on native Windows".to_string())
}

#[cfg(windows)]
#[derive(Debug, Default)]
struct TerminalShimOpts {
    daemon_addr: Option<String>,
    name: Option<String>,
    cols: i16,
    rows: i16,
    follow_console_size: bool,
    cmdline: Option<String>,
    cmdline_b64: Option<String>,
    command: Vec<String>,
}

#[cfg(windows)]
fn parse_terminal_shim_args(raw: Vec<String>) -> Result<TerminalShimOpts, String> {
    let mut opts = TerminalShimOpts {
        cols: DEFAULT_LCD_COLS,
        rows: DEFAULT_LCD_ROWS,
        ..TerminalShimOpts::default()
    };
    let mut iter = raw.into_iter();
    let mut command_started = false;
    while let Some(arg) = iter.next() {
        if command_started {
            opts.command.push(arg);
            continue;
        }
        match arg.as_str() {
            "--" => command_started = true,
            "--daemon" => opts.daemon_addr = Some(iter.next().ok_or("--daemon needs ADDR")?),
            "--name" => opts.name = Some(iter.next().ok_or("--name needs NAME")?),
            "--cols" => {
                opts.cols = iter
                    .next()
                    .ok_or("--cols needs N")?
                    .parse()
                    .map_err(|err| format!("--cols invalid: {err}"))?
            }
            "--rows" => {
                opts.rows = iter
                    .next()
                    .ok_or("--rows needs N")?
                    .parse()
                    .map_err(|err| format!("--rows invalid: {err}"))?
            }
            "--follow-console-size" => opts.follow_console_size = true,
            "--cmdline" => opts.cmdline = Some(iter.next().ok_or("--cmdline needs COMMANDLINE")?),
            "--cmdline-b64" => {
                opts.cmdline_b64 = Some(iter.next().ok_or("--cmdline-b64 needs B64")?)
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown terminal-shim flag: {other}"));
            }
            _ => {
                opts.command.push(arg);
                command_started = true;
            }
        }
    }
    Ok(opts)
}

#[cfg(windows)]
fn run_terminal_shim(raw: Vec<String>) -> Result<u32, String> {
    let pid = std::process::id();
    append_terminal_shim_log(format!(
        "[terminal-shim] start pid={} raw={}",
        pid,
        terminal_shim_log_args(&raw)
    ));

    let opts = match parse_terminal_shim_args(raw) {
        Ok(opts) => opts,
        Err(err) => {
            append_terminal_shim_log(format!("[terminal-shim] parse-failed pid={pid} err={err}"));
            return Err(err);
        }
    };
    let addr = opts
        .daemon_addr
        .clone()
        .unwrap_or_else(|| "127.0.0.1:8765".to_string());
    append_terminal_shim_log(format!(
        "[terminal-shim] parsed pid={} daemon={} name={} cols={} rows={} follow_console_size={} cmdline_b64={} command_args={}",
        pid,
        terminal_shim_log_value(&addr),
        terminal_shim_log_value(opts.name.as_deref().unwrap_or("")),
        opts.cols,
        opts.rows,
        opts.follow_console_size,
        opts.cmdline_b64.is_some(),
        opts.command.len()
    ));

    let command = match terminal_shim_command(&opts) {
        Ok(command) => command,
        Err(err) => {
            append_terminal_shim_log(format!(
                "[terminal-shim] command-failed pid={} err={}",
                pid,
                terminal_shim_log_value(&err)
            ));
            return Err(err);
        }
    };

    if let Err(err) = ensure_daemon_listening(&addr) {
        append_terminal_shim_log(format!(
            "[terminal-shim] daemon-failed pid={} daemon={} action=passthrough err={}",
            pid,
            terminal_shim_log_value(&addr),
            terminal_shim_log_value(&err)
        ));
        return run_terminal_shim_passthrough(pid, &command, &err);
    }
    append_terminal_shim_log(format!(
        "[terminal-shim] daemon-ready pid={} daemon={}",
        pid,
        terminal_shim_log_value(&addr)
    ));

    let name = opts.name.unwrap_or_else(|| "Vibe Terminal".to_string());
    append_terminal_shim_log(format!(
        "[terminal-shim] launch pid={} name={} command={}",
        pid,
        terminal_shim_log_value(&name),
        terminal_shim_log_command(&command)
    ));
    let mut launch_args = vec![
        "--daemon".to_string(),
        addr,
        "--kind".to_string(),
        "terminal".to_string(),
        "--name".to_string(),
        name,
        "--cols".to_string(),
        opts.cols.to_string(),
        "--rows".to_string(),
        opts.rows.to_string(),
    ];
    if opts.follow_console_size {
        launch_args.push("--follow-console-size".to_string());
    }
    launch_args.push("--".to_string());
    launch_args.extend(command.clone());
    let result = run_launch(launch_args);
    match &result {
        Ok(code) => {
            append_terminal_shim_log(format!("[terminal-shim] exit pid={} code={}", pid, code))
        }
        Err(err) => append_terminal_shim_log(format!(
            "[terminal-shim] launch-failed pid={} action=passthrough err={}",
            pid,
            terminal_shim_log_value(err)
        )),
    }
    match result {
        Ok(code) => Ok(code),
        Err(err) => run_terminal_shim_passthrough(pid, &command, &err),
    }
}

#[cfg(windows)]
fn run_terminal_shim_passthrough(
    pid: u32,
    command: &[String],
    reason: &str,
) -> Result<u32, String> {
    let (argv0, argv_rest) = command
        .split_first()
        .ok_or_else(|| "terminal-shim passthrough command is empty".to_string())?;
    append_terminal_shim_log(format!(
        "[terminal-shim] passthrough pid={} reason={} command={}",
        pid,
        terminal_shim_log_value(reason),
        terminal_shim_log_command(command)
    ));
    let status = std::process::Command::new(argv0)
        .args(argv_rest)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|err| format!("passthrough spawn {}: {err}", argv0))?;
    Ok(status.code().unwrap_or(1) as u32)
}

#[cfg(windows)]
fn append_terminal_shim_log(message: impl AsRef<str>) {
    use std::io::Write;

    let Ok(log_dir) = local_appdata_dir().map(|dir| dir.join("vibe-bridge")) else {
        return;
    };
    let _ = std::fs::create_dir_all(&log_dir);
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("vb-daemon.log"))
    else {
        return;
    };
    let _ = writeln!(file, "{}", message.as_ref());
}

#[cfg(not(windows))]
fn append_terminal_shim_log(_message: impl AsRef<str>) {}

#[cfg(windows)]
fn terminal_shim_log_args(args: &[String]) -> String {
    let parts: Vec<String> = args
        .iter()
        .scan(false, |redact_next, arg| {
            let lower = arg.to_ascii_lowercase();
            let sensitive = lower.contains("token")
                || lower.contains("secret")
                || lower.contains("password")
                || lower.contains("passwd")
                || lower.contains("apikey")
                || lower.contains("api-key")
                || lower.contains("credential")
                || lower.contains("cookie");
            let value = if *redact_next || sensitive {
                "<redacted>".to_string()
            } else {
                terminal_shim_log_value(arg)
            };
            *redact_next = matches!(
                lower.as_str(),
                "--cmdline"
                    | "--cmdline-b64"
                    | "--token"
                    | "--secret"
                    | "--password"
                    | "--passwd"
                    | "--api-key"
                    | "--apikey"
                    | "--credential"
                    | "--cookie"
            );
            Some(value)
        })
        .collect();
    format!("[{}]", parts.join(", "))
}

#[cfg(windows)]
fn terminal_shim_log_command(command: &[String]) -> String {
    if command.len() >= 4
        && command[0].eq_ignore_ascii_case("cmd.exe")
        && command[1].eq_ignore_ascii_case("/D")
        && command[2].eq_ignore_ascii_case("/S")
        && command[3].eq_ignore_ascii_case("/C")
    {
        let mut shown = command[..4].to_vec();
        shown.push("<cmdline>".to_string());
        return terminal_shim_log_args(&shown);
    }
    terminal_shim_log_args(command)
}

fn terminal_shim_log_value(value: &str) -> String {
    const MAX_LEN: usize = 240;
    let mut clean = value.replace(['\r', '\n', '\t'], " ");
    if clean.len() > MAX_LEN {
        clean.truncate(MAX_LEN);
        clean.push_str("...");
    }
    clean
}

#[cfg(windows)]
fn terminal_shim_command(opts: &TerminalShimOpts) -> Result<Vec<String>, String> {
    if let Some(encoded) = &opts.cmdline_b64 {
        let bytes = base64_decode_bytes(encoded)?;
        let cmdline = String::from_utf8(bytes)
            .map_err(|err| format!("--cmdline-b64 decoded invalid utf-8: {err}"))?;
        return Ok(vec![
            "cmd.exe".to_string(),
            "/D".to_string(),
            "/S".to_string(),
            "/C".to_string(),
            cmdline,
        ]);
    }
    if let Some(cmdline) = &opts.cmdline {
        return Ok(vec![
            "cmd.exe".to_string(),
            "/D".to_string(),
            "/S".to_string(),
            "/C".to_string(),
            cmdline.clone(),
        ]);
    }
    if !opts.command.is_empty() {
        return Ok(opts.command.clone());
    }
    Ok(vec!["powershell.exe".to_string()])
}

fn maybe_wrap_wsl_terminal_command(
    command: &[String],
    addr: &str,
    parent_agent_id: &str,
) -> Vec<String> {
    let Some(wsl) = parse_simple_wsl_command(command) else {
        if wsl_command_tokens(command).is_some() {
            append_terminal_shim_log(
                "[terminal-shim] WSL profile transient shims skipped reason=non-interactive-or-unsupported",
            );
        }
        return command.to_vec();
    };
    let mut wrapped = vec!["wsl.exe".to_string()];
    let interactive_args = wsl.interactive_args;
    wrapped.extend(interactive_args.iter().cloned());
    if !wsl_args_include_cd(&interactive_args) {
        wrapped.push("--cd".to_string());
        wrapped.push("~".to_string());
    }
    wrapped.extend([
        "--exec".to_string(),
        "bash".to_string(),
        "-lc".to_string(),
        wsl_terminal_entry_script(addr, parent_agent_id),
        "vibe-bridge-terminal".to_string(),
    ]);
    append_terminal_shim_log(format!(
        "[terminal-shim] WSL profile transient shims enabled parent={}",
        terminal_shim_log_value(parent_agent_id)
    ));
    wrapped
}

struct ParsedWslCommand {
    interactive_args: Vec<String>,
}

fn parse_simple_wsl_command(command: &[String]) -> Option<ParsedWslCommand> {
    let tokens = wsl_command_tokens(command)?;
    Some(ParsedWslCommand {
        interactive_args: interactive_wsl_args(&tokens[1..])?,
    })
}

fn wsl_command_tokens(command: &[String]) -> Option<Vec<String>> {
    let tokens = if command.len() == 5
        && command[0].eq_ignore_ascii_case("cmd.exe")
        && command[1].eq_ignore_ascii_case("/D")
        && command[2].eq_ignore_ascii_case("/S")
        && command[3].eq_ignore_ascii_case("/C")
    {
        split_windows_commandline_loosely(&command[4])
    } else {
        command.to_vec()
    };
    let first = tokens.first()?;
    let exe = first
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(first)
        .to_ascii_lowercase();
    if exe != "wsl.exe" && exe != "wsl" {
        return None;
    }
    Some(tokens)
}

fn interactive_wsl_args(args: &[String]) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-d" | "--distribution" | "-u" | "--user" | "--cd" | "--shell-type" => {
                out.push(arg.clone());
                out.push(iter.next()?.clone());
            }
            other
                if other.starts_with("--distribution=")
                    || other.starts_with("--user=")
                    || other.starts_with("--cd=")
                    || other.starts_with("--shell-type=") =>
            {
                out.push(arg.clone());
            }
            "-e" | "--exec" => return None,
            "-l" | "--list" | "-v" | "--verbose" | "--status" | "--help" | "--version"
            | "--shutdown" | "--install" | "--update" | "--unregister" | "--terminate"
            | "--set-default" | "--set-version" | "--import" | "--export" | "--mount"
            | "--unmount" => return None,
            "--" => return None,
            _ if arg.starts_with('-') => return None,
            _ => return None,
        }
    }
    Some(out)
}

fn wsl_args_include_cd(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--cd" || arg.starts_with("--cd="))
}

fn split_windows_commandline_loosely(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut in_quotes = false;
    while let Some(ch) = chars.next() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '\\' if in_quotes && chars.peek() == Some(&'"') => {
                let _ = chars.next();
                current.push('"');
            }
            ch if ch.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn wsl_terminal_entry_script(addr: &str, parent_agent_id: &str) -> String {
    format!(
        r#"set -eu
VB_ADDR={addr}
VB_PARENT={parent}
SHIM_DIR="${{TMPDIR:-/tmp}}/vibe-bridge-shims-${{VB_PARENT}}-$$"
mkdir -p "$SHIM_DIR"

make_shim() {{
  name="$1"
  cat > "$SHIM_DIR/$name" <<'WRAP'
#!/usr/bin/env bash
# vibe-bridge WSL transient agent shim
set -u
cmd="$(basename "$0")"
shim_dir="${{VIBE_BRIDGE_WSL_SHIM_DIR:-}}"
agent_id="launch-wsl-${{cmd}}-$$"
parent="${{VIBE_BRIDGE_TERMINAL_AGENT_ID:-}}"
daemon="${{VIBE_BRIDGE_DAEMON:-127.0.0.1:8765}}"
host="${{daemon%:*}}"
port="${{daemon##*:}}"
shell_integration="$HOME/.local/share/vibe-bridge/shell-integration/bin/$cmd"

if [ -x "$shell_integration" ] && [ "$shell_integration" != "$0" ]; then
  exec "$shell_integration" "$@"
fi

is_vibe_wrapper() {{
  [ -f "$1" ] && grep -q 'vibe-bridge WSL .*agent shim\|vibe-bridge WSL agent wrapper' "$1" 2>/dev/null
}}

find_real() {{
  local old_ifs="$IFS"
  local dir candidate
  IFS=:
  for dir in $PATH; do
    [ -n "$dir" ] || dir=.
    [ -n "$shim_dir" ] && [ "$dir" = "$shim_dir" ] && continue
    candidate="$dir/$cmd"
    if [ -x "$candidate" ] && ! is_vibe_wrapper "$candidate"; then
      printf '%s\n' "$candidate"
      IFS="$old_ifs"
      return 0
    fi
  done
  IFS="$old_ifs"
  return 1
}}

send_daemon() {{
  local json="$1"
  if {{ exec 9<>"/dev/tcp/$host/$port"; }} 2>/dev/null; then
    printf '%s\n' "$json" >&9 || true
    IFS= read -r _ <&9 || true
    exec 9>&- 9<&- || true
  fi
}}

register_agent() {{
  [ -n "$parent" ] || return 0
  send_daemon "$(printf '{{"type":"agent.register","agent":{{"agentId":"%s","kind":"%s","name":"%s","cwd":"","fromLaunch":true,"parentKind":"terminal","parentAgentId":"%s"}}}}' "$agent_id" "$cmd" "$cmd" "$parent")"
}}

abort_agent() {{
  send_daemon "$(printf '{{"type":"session.abort","abort":{{"agentId":"%s","kind":"%s"}}}}' "$agent_id" "$cmd")"
}}

real="$(find_real || true)"
if [ -z "$real" ]; then
  echo "vibe-bridge: real $cmd not found in WSL transient PATH" >&2
  exit 127
fi
register_agent
"$real" "$@"
rc=$?
abort_agent
exit "$rc"
WRAP
  chmod +x "$SHIM_DIR/$name"
}}

make_shim codex
make_shim claude

cat > "$SHIM_DIR/bashrc" <<'BASHRC'
# vibe-bridge WSL transient bashrc
if [ -f "$HOME/.bashrc" ]; then
  . "$HOME/.bashrc"
fi
if [ -n "${{VIBE_BRIDGE_WSL_SHIM_DIR:-}}" ]; then
  old_ifs="$IFS"
  new_path=""
  IFS=:
  for dir in $PATH; do
    [ "$dir" = "$VIBE_BRIDGE_WSL_SHIM_DIR" ] && continue
    if [ -z "$new_path" ]; then
      new_path="$dir"
    else
      new_path="$new_path:$dir"
    fi
  done
  IFS="$old_ifs"
  export PATH="$VIBE_BRIDGE_WSL_SHIM_DIR:$new_path"
fi
BASHRC

export VIBE_BRIDGE_DAEMON="$VB_ADDR"
export VIBE_BRIDGE_TERMINAL_AGENT_ID="$VB_PARENT"
export VIBE_BRIDGE_CAPTURED_TERMINAL=1
export VIBE_BRIDGE_WSL_SHIM_DIR="$SHIM_DIR"
export PATH="$SHIM_DIR:$PATH"
shell="${{SHELL:-/bin/bash}}"
case "$(basename "$shell")" in
  bash) exec "$shell" --rcfile "$SHIM_DIR/bashrc" -i ;;
  *) exec "$shell" -i ;;
esac
"#,
        addr = sh_quote(addr),
        parent = sh_quote(parent_agent_id),
    )
}

#[cfg(not(windows))]
fn run_terminal_shim(_raw: Vec<String>) -> Result<u32, String> {
    Err("terminal-shim is only implemented on native Windows".to_string())
}

#[cfg(windows)]
#[derive(Debug)]
struct InstallWindowsOpts {
    addr: String,
    device: String,
    shim_dir: std::path::PathBuf,
    wsl_distros: Vec<String>,
    install_wsl: bool,
    install_wsl_shell: bool,
    install_startup: bool,
    install_terminal_profiles: bool,
    install_wsl_shortcuts: bool,
    install_management_shortcuts: bool,
    update_path: bool,
}

#[cfg(windows)]
#[derive(Debug)]
struct UninstallWindowsOpts {
    addr: String,
    shim_dir: std::path::PathBuf,
    remove_terminal_profiles: bool,
    purge: bool,
}

#[cfg(windows)]
fn run_install_windows(raw: Vec<String>) -> Result<(), String> {
    if running_inside_captured_terminal()? {
        return Err(
            "install-windows is blocked inside a Vibe-captured terminal profile. \
             Open an unwrapped shell with Win+R -> powershell.exe -NoLogo -NoProfile, \
             then rerun install-windows there."
                .to_string(),
        );
    }
    product_step("starting Windows install or repair");
    let opts = parse_install_windows_args(raw)?;
    let source_exe = std::env::current_exe()
        .map_err(|err| format!("current_exe: {err}"))?
        .canonicalize()
        .map_err(|err| format!("canonicalize current exe: {err}"))?;
    let log_dir = local_appdata_dir()?.join("vibe-bridge");
    product_step("preparing install directories");
    std::fs::create_dir_all(&opts.shim_dir)
        .map_err(|err| format!("create shim dir {}: {err}", opts.shim_dir.display()))?;
    std::fs::create_dir_all(&log_dir)
        .map_err(|err| format!("create log dir {}: {err}", log_dir.display()))?;
    product_step("stopping previous background daemon if present");
    stop_background_daemon_processes_for_addr(&opts.addr)?;
    product_step("copying versioned daemon executable");
    let exe = install_daemon_exe(&source_exe, &opts.shim_dir)?;

    product_step("checking optional WSL integration");
    let selected_wsl_distros = resolve_wsl_distros(&opts)?;
    let default_wsl_distro = if opts.wsl_distros.len() == 1 {
        opts.wsl_distros.first().map(String::as_str)
    } else {
        None
    };

    product_step("writing codex, claude, and wsl command shims");
    write_agent_cmd(&opts.shim_dir, "codex", &exe, default_wsl_distro)?;
    write_agent_cmd(&opts.shim_dir, "claude", &exe, default_wsl_distro)?;
    write_wsl_cmd(&opts.shim_dir, &exe)?;

    if opts.install_startup {
        product_step("installing Startup background daemon entry");
        let startup_dir = startup_dir()?;
        std::fs::create_dir_all(&startup_dir)
            .map_err(|err| format!("create startup dir {}: {err}", startup_dir.display()))?;
        write_startup_cmd(&startup_dir, &exe, &opts.addr, &opts.device, &log_dir)?;
    }

    if opts.update_path {
        product_step("ensuring user PATH points to Vibe Bridge shims");
        ensure_user_path_contains(&opts.shim_dir)?;
    }

    let terminal_profile_summary = if opts.install_terminal_profiles {
        product_step("wrapping Windows Terminal profiles");
        match install_windows_terminal_profiles(&exe, &opts.addr) {
            Ok(summary) => summary,
            Err(err) => {
                eprintln!("warning: Windows Terminal profile install failed: {err}");
                format!("not confirmed ({err})")
            }
        }
    } else {
        product_step("restoring native Windows Terminal profiles");
        match uninstall_windows_terminal_profiles() {
            Ok(summary) => format!("native ({summary})"),
            Err(err) => {
                eprintln!("warning: Windows Terminal profile restore failed: {err}");
                format!("native not confirmed ({err})")
            }
        }
    };

    let wsl_shortcut_summary = if opts.install_wsl_shortcuts {
        product_step("creating WSL Start Menu shortcuts");
        match install_wsl_start_menu_shortcuts(&exe, &opts.addr) {
            Ok(summary) => summary,
            Err(err) => {
                eprintln!("warning: WSL Start Menu shortcut install failed: {err}");
                format!("not confirmed ({err})")
            }
        }
    } else {
        "skipped".to_string()
    };

    let mut installed_wsl_shell = Vec::new();
    if opts.install_wsl_shell {
        product_step("installing WSL shell integration");
        match resolve_wsl_shell_distros(&opts) {
            Ok(distros) => {
                for distro in distros {
                    match install_wsl_shell_integration(&distro, &opts.addr) {
                        Ok(()) => installed_wsl_shell.push(distro),
                        Err(err) => {
                            eprintln!("warning: WSL shell integration {distro} failed: {err}")
                        }
                    }
                }
            }
            Err(err) => eprintln!("warning: WSL shell distro enumeration failed: {err}"),
        }
    }

    let management_shortcut_summary = if opts.install_management_shortcuts {
        product_step("creating Vibe Bridge management shortcuts");
        match install_management_start_menu_shortcuts(&exe, &opts.addr, &opts.device) {
            Ok(summary) => summary,
            Err(err) => {
                eprintln!("warning: management shortcut install failed: {err}");
                format!("not confirmed ({err})")
            }
        }
    } else {
        "skipped".to_string()
    };

    let mut installed_wsl = Vec::new();
    for distro in &selected_wsl_distros {
        product_step(&format!("installing optional WSL wrapper for {distro}"));
        match install_wsl_distro(distro, &exe, &opts.addr) {
            Ok(()) => installed_wsl.push(distro.clone()),
            Err(err) => eprintln!("warning: WSL distro {distro} install failed: {err}"),
        }
    }

    product_step("starting background daemon");
    let daemon_now = match ensure_daemon_listening_with_exe(&opts.addr, &exe, &opts.device) {
        Ok(()) => "running".to_string(),
        Err(err) => {
            eprintln!("warning: daemon not confirmed running: {err}");
            format!("not confirmed ({err})")
        }
    };

    println!("vibe-bridge Windows install complete");
    println!("source exe : {}", source_exe.display());
    println!("daemon exe : {}", exe.display());
    println!("shim dir   : {}", opts.shim_dir.display());
    println!("daemon addr: {}", opts.addr);
    println!("device     : {}", opts.device);
    println!("daemon now : {daemon_now}");
    println!(
        "wsl install: {}",
        if opts.install_wsl {
            if installed_wsl.is_empty() {
                "none".to_string()
            } else {
                installed_wsl.join(", ")
            }
        } else {
            "skipped".to_string()
        }
    );
    println!(
        "startup    : {}",
        if opts.install_startup {
            "installed"
        } else {
            "skipped"
        }
    );
    println!("terminal   : {terminal_profile_summary}");
    println!("wsl links  : {wsl_shortcut_summary}");
    println!(
        "wsl shell  : {}",
        if opts.install_wsl_shell {
            if installed_wsl_shell.is_empty() {
                "none".to_string()
            } else {
                installed_wsl_shell.join(", ")
            }
        } else {
            "skipped".to_string()
        }
    );
    println!("start menu : {management_shortcut_summary}");
    println!(
        "user PATH  : {}",
        if opts.update_path {
            "shim dir ensured; open a new terminal to use it"
        } else {
            "skipped"
        }
    );
    Ok(())
}

#[cfg(not(windows))]
fn run_install_windows(_raw: Vec<String>) -> Result<(), String> {
    Err("install-windows is only implemented on native Windows".to_string())
}

fn run_install_product(raw: Vec<String>) -> Result<(), String> {
    run_install_windows(product_install_args(raw))
}

#[cfg(windows)]
fn run_uninstall_windows(raw: Vec<String>) -> Result<(), String> {
    if running_inside_captured_terminal()? {
        return Err(
            "uninstall-windows is blocked inside a Vibe-captured terminal profile. \
             Open an unwrapped shell with Win+R -> powershell.exe -NoLogo -NoProfile, \
             then rerun uninstall-windows there."
                .to_string(),
        );
    }

    product_step("starting Windows uninstall");
    let opts = parse_uninstall_windows_args(raw)?;
    product_step("stopping background daemon");
    stop_background_daemon_processes_for_addr(&opts.addr)?;
    product_step("removing Startup entry and command shims");
    let startup_removed = remove_startup_cmd()?;
    let shim_removed = remove_agent_cmds(&opts.shim_dir)?;
    product_step("removing WSL Start Menu shortcuts");
    let wsl_shortcuts_removed = remove_wsl_start_menu_shortcuts()?;
    product_step("removing WSL shell integration");
    let removed_wsl_shell = remove_wsl_shell_integrations(&opts.addr)?;
    let terminal_profile_summary = if opts.remove_terminal_profiles {
        product_step("restoring Windows Terminal profiles");
        match uninstall_windows_terminal_profiles() {
            Ok(summary) => summary,
            Err(err) => {
                eprintln!("warning: Windows Terminal profile uninstall failed: {err}");
                format!("not confirmed ({err})")
            }
        }
    } else {
        "skipped".to_string()
    };
    product_step("removing installed daemon executables");
    let (exe_removed, exe_skipped) = cleanup_daemon_exes(&opts.shim_dir)?;
    if opts.purge {
        product_step("purging local logs and status");
        purge_local_state()?;
    } else {
        remove_daemon_status_file();
    }

    println!("vibe-bridge Windows uninstall complete");
    println!("daemon addr: {}", opts.addr);
    println!("shim dir   : {}", opts.shim_dir.display());
    println!(
        "startup    : {}",
        if startup_removed { "removed" } else { "absent" }
    );
    println!("shims      : removed {shim_removed}");
    println!("terminal   : {terminal_profile_summary}");
    println!(
        "wsl links  : {}",
        if wsl_shortcuts_removed {
            "removed"
        } else {
            "absent"
        }
    );
    println!(
        "wsl shell  : {}",
        if removed_wsl_shell.is_empty() {
            "absent".to_string()
        } else {
            removed_wsl_shell.join(", ")
        }
    );
    println!("daemon exe : removed {exe_removed}, skipped {exe_skipped}");
    println!("purge      : {}", if opts.purge { "yes" } else { "no" });
    Ok(())
}

#[cfg(not(windows))]
fn run_uninstall_windows(_raw: Vec<String>) -> Result<(), String> {
    Err("uninstall-windows is only implemented on native Windows".to_string())
}

fn run_uninstall_product(raw: Vec<String>) -> Result<(), String> {
    run_uninstall_windows(raw)
}

#[cfg(windows)]
fn run_status_windows(raw: Vec<String>) -> Result<(), String> {
    use std::net::TcpStream;

    let addr = parse_status_windows_args(raw)?;
    let local_appdata = local_appdata_dir()?;
    let install_dir = local_appdata.join("vibe-bridge");
    let shim_dir = install_dir.join("bin");
    let startup_script = startup_dir()?.join("vibe-bridge-daemon.cmd");
    let status_path = daemon_status_path()?;
    let log_path = install_dir.join("vb-daemon.log");
    let daemon_reachable = TcpStream::connect(&addr).is_ok();

    println!("vibe-bridge Windows status");
    println!("daemon addr : {addr}");
    println!(
        "daemon tcp  : {}",
        if daemon_reachable {
            "listening"
        } else {
            "not reachable"
        }
    );
    println!("install dir : {}", install_dir.display());
    println!(
        "startup     : {}",
        if startup_script.exists() {
            startup_script.display().to_string()
        } else {
            "absent".to_string()
        }
    );
    println!(
        "codex shim  : {}",
        if shim_dir.join("codex.cmd").exists() {
            "present"
        } else {
            "absent"
        }
    );
    println!(
        "claude shim : {}",
        if shim_dir.join("claude.cmd").exists() {
            "present"
        } else {
            "absent"
        }
    );
    println!(
        "wsl shim    : {}",
        if shim_dir.join("wsl.cmd").exists() {
            "present"
        } else {
            "absent"
        }
    );
    println!("status file : {}", status_path.display());
    if let Ok(status) = std::fs::read_to_string(&status_path) {
        for line in status.lines().take(12) {
            println!("  {line}");
        }
    } else {
        println!("  absent");
    }
    println!("log file    : {}", log_path.display());
    Ok(())
}

#[cfg(not(windows))]
fn run_status_windows(_raw: Vec<String>) -> Result<(), String> {
    Err("status-windows is only implemented on native Windows".to_string())
}

#[cfg(windows)]
fn parse_status_windows_args(raw: Vec<String>) -> Result<String, String> {
    let mut addr = "127.0.0.1:8765".to_string();
    let mut iter = raw.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--addr" => addr = iter.next().ok_or("--addr needs ADDR")?,
            "-h" | "--help" => return Err("usage: status-windows [--addr ADDR]".to_string()),
            other => return Err(format!("unknown status-windows flag: {other}")),
        }
    }
    Ok(addr)
}

#[cfg(windows)]
fn parse_install_windows_args(raw: Vec<String>) -> Result<InstallWindowsOpts, String> {
    let mut opts = InstallWindowsOpts {
        addr: "127.0.0.1:8765".to_string(),
        device: "auto".to_string(),
        shim_dir: local_appdata_dir()?.join("vibe-bridge").join("bin"),
        wsl_distros: Vec::new(),
        install_wsl: false,
        install_wsl_shell: false,
        install_startup: true,
        install_terminal_profiles: false,
        install_wsl_shortcuts: true,
        install_management_shortcuts: false,
        update_path: true,
    };
    let mut iter = raw.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--addr" => opts.addr = iter.next().ok_or("--addr needs ADDR")?,
            "--device" => opts.device = iter.next().ok_or("--device needs auto|PATH")?,
            "--shim-dir" => {
                opts.shim_dir = std::path::PathBuf::from(iter.next().ok_or("--shim-dir needs DIR")?)
            }
            "--wsl" => opts.install_wsl = true,
            "--wsl-distro" => {
                opts.install_wsl = true;
                opts.wsl_distros.push(iter.next().ok_or("--wsl-distro needs NAME")?)
            }
            "--no-wsl" => opts.install_wsl = false,
            "--wsl-shell" => opts.install_wsl_shell = true,
            "--no-wsl-shell" => opts.install_wsl_shell = false,
            "--no-startup" => opts.install_startup = false,
            "--terminal-profiles" => opts.install_terminal_profiles = true,
            "--no-terminal-profiles" => opts.install_terminal_profiles = false,
            "--wsl-shortcuts" => opts.install_wsl_shortcuts = true,
            "--no-wsl-shortcuts" => opts.install_wsl_shortcuts = false,
            "--management-shortcuts" => opts.install_management_shortcuts = true,
            "--no-management-shortcuts" => opts.install_management_shortcuts = false,
            "--no-path" => opts.update_path = false,
            "-h" | "--help" => {
                return Err(
                    "usage: install-windows [--addr ADDR] [--device auto|PATH] [--shim-dir DIR] [--wsl] [--wsl-distro NAME] [--wsl-shell] [--terminal-profiles] [--management-shortcuts] [--no-wsl-shortcuts] [--no-startup] [--no-path]"
                        .to_string(),
                )
            }
            other => return Err(format!("unknown install-windows flag: {other}")),
        }
    }
    Ok(opts)
}

#[cfg(windows)]
fn parse_uninstall_windows_args(raw: Vec<String>) -> Result<UninstallWindowsOpts, String> {
    let mut opts = UninstallWindowsOpts {
        addr: "127.0.0.1:8765".to_string(),
        shim_dir: local_appdata_dir()?.join("vibe-bridge").join("bin"),
        remove_terminal_profiles: true,
        purge: false,
    };
    let mut iter = raw.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--addr" => opts.addr = iter.next().ok_or("--addr needs ADDR")?,
            "--shim-dir" => {
                opts.shim_dir = std::path::PathBuf::from(iter.next().ok_or("--shim-dir needs DIR")?)
            }
            "--terminal-profiles" => opts.remove_terminal_profiles = true,
            "--no-terminal-profiles" => opts.remove_terminal_profiles = false,
            "--purge" => opts.purge = true,
            "-h" | "--help" => {
                return Err(
                    "usage: uninstall-windows [--addr ADDR] [--shim-dir DIR] [--terminal-profiles|--no-terminal-profiles] [--purge]"
                        .to_string(),
                )
            }
            other => return Err(format!("unknown uninstall-windows flag: {other}")),
        }
    }
    Ok(opts)
}

#[cfg(windows)]
fn running_inside_captured_terminal() -> Result<bool, String> {
    if std::env::var_os("VIBE_BRIDGE_CAPTURED_TERMINAL").is_some() {
        return Ok(true);
    }

    let current_pid = std::process::id();
    let script = format!(
        r#"
$pidToCheck = {}
$seen = @{{}}
while ($pidToCheck -and -not $seen.ContainsKey([string]$pidToCheck)) {{
  $seen[[string]$pidToCheck] = $true
  $proc = Get-CimInstance Win32_Process -Filter ("ProcessId = " + $pidToCheck) -ErrorAction SilentlyContinue
  if ($null -eq $proc) {{ break }}
  $name = [string]$proc.Name
  $cmdline = [string]$proc.CommandLine
  if ($name -like 'vb-daemon*.exe' -and ($cmdline -match '(^|[\s"]+)(terminal-shim|capture-shell|vibe-terminal|launch)([\s"]+|$)')) {{
    Write-Output 'captured'
    exit 0
  }}
  $pidToCheck = $proc.ParentProcessId
}}
Write-Output 'plain'
"#,
        current_pid
    );
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output()
        .map_err(|err| format!("run powershell to inspect parent process: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "inspect parent process exited with {}; stderr={}",
            output.status,
            decode_windows_command_output(&output.stderr).trim()
        ));
    }
    Ok(decode_windows_command_output(&output.stdout).contains("captured"))
}

#[cfg(windows)]
fn install_daemon_exe(
    source: &std::path::Path,
    shim_dir: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let hash = file_hash_hex(source)?;
    let dest = shim_dir.join(format!("vb-daemon-{hash}.exe"));
    if dest.exists() && same_path(source, &dest) {
        return Ok(dest);
    }

    if dest.exists() {
        return Ok(dest);
    }
    std::fs::copy(source, &dest).map_err(|err| {
        format!(
            "copy daemon exe {} -> {}: {err}",
            source.display(),
            dest.display()
        )
    })?;
    Ok(dest)
}

#[cfg(windows)]
fn file_hash_hex(path: &std::path::Path) -> Result<String, String> {
    use std::io::Read;

    let mut file =
        std::fs::File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|err| format!("read {}: {err}", path.display()))?;
        if n == 0 {
            break;
        }
        for byte in &buf[..n] {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    Ok(format!("{hash:016x}"))
}

#[cfg(windows)]
fn daemon_status_path() -> Result<std::path::PathBuf, String> {
    Ok(local_appdata_dir()?
        .join("vibe-bridge")
        .join("daemon-status.json"))
}

#[cfg(windows)]
fn write_daemon_status(
    addr: &str,
    device: &str,
    exe: &std::path::Path,
    pid: u32,
) -> Result<(), String> {
    let path = daemon_status_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("create status dir {}: {err}", parent.display()))?;
    }
    let status = serde_json::json!({
        "pid": pid,
        "addr": addr,
        "device": device,
        "exe": exe.to_string_lossy(),
        "startedMs": current_time_millis(),
    });
    let mut output = serde_json::to_string_pretty(&status)
        .map_err(|err| format!("serialize daemon status: {err}"))?;
    output.push('\n');
    std::fs::write(&path, output).map_err(|err| format!("write {}: {err}", path.display()))
}

#[cfg(windows)]
fn remove_daemon_status_file() {
    if let Ok(path) = daemon_status_path() {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(windows)]
fn current_time_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0)
}

#[cfg(windows)]
fn stop_background_daemon_processes_for_addr(addr: &str) -> Result<(), String> {
    let current_pid = std::process::id();
    let status_path = daemon_status_path()?
        .to_string_lossy()
        .to_string()
        .replace('\'', "''");
    let script = format!(
        r#"
$addr = '{}'
$current = {}
$statusPath = '{}'
$candidateIds = @()
if (Test-Path -LiteralPath $statusPath) {{
  try {{
    $status = Get-Content -LiteralPath $statusPath -Raw | ConvertFrom-Json
    if ($null -ne $status -and [string]$status.addr -ieq $addr -and $null -ne $status.pid) {{
      $candidateIds += [int]$status.pid
    }}
  }} catch {{}}
}}
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {{ $_.Name -like 'vb-daemon*.exe' }} | ForEach-Object {{
  $cmdline = [string]$_.CommandLine
  $hasAddr = ($cmdline.IndexOf($addr, [StringComparison]::OrdinalIgnoreCase) -ge 0)
  $isBackgroundDaemon = ($cmdline -match '(^|[\s"]+)serve-hid([\s"]+|$)')
  if ($_.ProcessId -ne $current -and $hasAddr -and $isBackgroundDaemon) {{
    $candidateIds += [int]$_.ProcessId
  }}
}}
$candidateIds = @($candidateIds | Select-Object -Unique)
foreach ($candidateId in $candidateIds) {{
  $proc = Get-CimInstance Win32_Process -Filter ("ProcessId = " + $candidateId) -ErrorAction SilentlyContinue
  if ($null -eq $proc) {{ continue }}
  $cmdline = [string]$proc.CommandLine
  $name = [string]$proc.Name
  $hasAddr = ($cmdline.IndexOf($addr, [StringComparison]::OrdinalIgnoreCase) -ge 0)
  $isBackgroundDaemon = ($cmdline -match '(^|[\s"]+)serve-hid([\s"]+|$)')
  if ($candidateId -ne $current -and $name -like 'vb-daemon*.exe' -and $hasAddr -and $isBackgroundDaemon) {{
    Stop-Process -Id $candidateId -Force -ErrorAction SilentlyContinue
  }}
}}
$deadline = [DateTime]::UtcNow.AddSeconds(8)
do {{
  Start-Sleep -Milliseconds 150
  $still = @()
  Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {{ $_.Name -like 'vb-daemon*.exe' }} | ForEach-Object {{
    $cmdline = [string]$_.CommandLine
    $hasAddr = ($cmdline.IndexOf($addr, [StringComparison]::OrdinalIgnoreCase) -ge 0)
    $isBackgroundDaemon = ($cmdline -match '(^|[\s"]+)serve-hid([\s"]+|$)')
    if ($_.ProcessId -ne $current -and $hasAddr -and $isBackgroundDaemon) {{
      $still += $_
    }}
  }}
}} while ($still.Count -gt 0 -and [DateTime]::UtcNow -lt $deadline)
if ($still.Count -gt 0) {{
  Write-Error ("background daemon still running for " + $addr + ": " + (($still | ForEach-Object {{ $_.ProcessId }}) -join ','))
  exit 1
}}
if (Test-Path -LiteralPath $statusPath) {{
  try {{
    $status = Get-Content -LiteralPath $statusPath -Raw | ConvertFrom-Json
    if ($null -ne $status -and [string]$status.addr -ieq $addr) {{
      Remove-Item -LiteralPath $statusPath -Force -ErrorAction SilentlyContinue
    }}
  }} catch {{}}
}}
"#,
        addr.replace('\'', "''"),
        current_pid,
        status_path
    );
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output()
        .map_err(|err| format!("run powershell to stop background daemon: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "stop background daemon exited with {}; stderr={}",
            output.status,
            decode_windows_command_output(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn write_agent_cmd(
    shim_dir: &std::path::Path,
    kind: &str,
    exe: &std::path::Path,
    wsl_distro: Option<&str>,
) -> Result<(), String> {
    let distro_line = wsl_distro
        .map(|distro| format!("set \"VIBE_BRIDGE_WSL_DISTRO={}\"\r\n", distro))
        .unwrap_or_default();
    let content = format!(
        "@echo off\r\n\
         setlocal\r\n\
         set \"VIBE_BRIDGE_SHIM_DIR=%~dp0\"\r\n\
         {}\
         \"{}\" agent-shim {} %*\r\n\
         exit /b %ERRORLEVEL%\r\n",
        distro_line,
        shell_display_path(exe),
        kind
    );
    let path = shim_dir.join(format!("{kind}.cmd"));
    std::fs::write(&path, content).map_err(|err| format!("write {}: {err}", path.display()))
}

#[cfg(windows)]
fn write_wsl_cmd(shim_dir: &std::path::Path, exe: &std::path::Path) -> Result<(), String> {
    let content = format!(
        "@echo off\r\n\
         setlocal\r\n\
         \"{}\" wsl-shim %*\r\n\
         exit /b %ERRORLEVEL%\r\n",
        shell_display_path(exe)
    );
    let path = shim_dir.join("wsl.cmd");
    std::fs::write(&path, content).map_err(|err| format!("write {}: {err}", path.display()))
}

#[cfg(windows)]
fn write_startup_cmd(
    startup_dir: &std::path::Path,
    exe: &std::path::Path,
    addr: &str,
    device: &str,
    log_dir: &std::path::Path,
) -> Result<(), String> {
    let log = log_dir.join("vb-daemon.log");
    let content = format!(
        "@echo off\r\n\
         setlocal\r\n\
         if not exist \"{}\" mkdir \"{}\"\r\n\
         start \"\" /min cmd.exe /D /C \"\"{}\" serve-hid {} {} >> \"{}\" 2>&1\"\r\n",
        log_dir.display(),
        log_dir.display(),
        exe.display(),
        addr,
        device,
        log.display()
    );
    let path = startup_dir.join("vibe-bridge-daemon.cmd");
    std::fs::write(&path, content).map_err(|err| format!("write {}: {err}", path.display()))
}

#[cfg(windows)]
fn remove_startup_cmd() -> Result<bool, String> {
    let path = startup_dir()?.join("vibe-bridge-daemon.cmd");
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&path).map_err(|err| format!("remove {}: {err}", path.display()))?;
    Ok(true)
}

#[cfg(windows)]
fn wsl_start_menu_shortcuts_dir() -> Result<std::path::PathBuf, String> {
    start_menu_shortcuts_dir()
}

#[cfg(windows)]
fn start_menu_programs_dir() -> Result<std::path::PathBuf, String> {
    let appdata = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "APPDATA is not set".to_string())?;
    Ok(appdata
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs"))
}

#[cfg(windows)]
fn start_menu_shortcuts_dir() -> Result<std::path::PathBuf, String> {
    Ok(start_menu_programs_dir()?.join("vibe-bridge"))
}

#[cfg(windows)]
fn install_management_start_menu_shortcuts(
    exe: &std::path::Path,
    addr: &str,
    device: &str,
) -> Result<String, String> {
    let dir = start_menu_shortcuts_dir()?;
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("create Start Menu shortcut dir {}: {err}", dir.display()))?;
    write_windows_shortcut(
        &dir.join("Install or Repair Vibe Bridge.lnk"),
        exe,
        &format!(
            "install-product --addr {} --device {}",
            quote_windows_arg(addr),
            quote_windows_arg(device)
        ),
        "Install or repair Vibe Bridge background capture",
    )?;
    write_windows_shortcut(
        &dir.join("Vibe Bridge Status.lnk"),
        exe,
        &format!("status-windows --addr {}", quote_windows_arg(addr)),
        "Show Vibe Bridge background daemon status",
    )?;
    write_windows_shortcut(
        &dir.join("Uninstall Vibe Bridge.lnk"),
        exe,
        &format!("uninstall-product --addr {}", quote_windows_arg(addr)),
        "Uninstall Vibe Bridge and restore Terminal profiles",
    )?;
    Ok("installed 3 management shortcut(s)".to_string())
}

#[cfg(windows)]
fn install_wsl_start_menu_shortcuts(exe: &std::path::Path, addr: &str) -> Result<String, String> {
    let distros = list_wsl_distros()?;
    if distros.is_empty() {
        return Ok("no WSL distros found".to_string());
    }
    let restored_direct = restore_direct_wsl_start_menu_shortcuts()?;
    let dir = wsl_start_menu_shortcuts_dir()?;
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("create WSL shortcut dir {}: {err}", dir.display()))?;
    let mut installed = Vec::new();
    for distro in distros {
        let original = format!("wsl.exe -d {}", quote_windows_arg(&distro));
        let args = format!(
            "terminal-shim --daemon {} --name {} --follow-console-size --cmdline-b64 {}",
            quote_windows_arg(addr),
            quote_windows_arg(&format!("Terminal: {distro}")),
            base64_encode_bytes(original.as_bytes())
        );
        let name = format!(
            "{} (vibe-bridge).lnk",
            sanitize_windows_shortcut_name(&distro)
        );
        write_windows_shortcut(
            &dir.join(name),
            exe,
            &args,
            &format!("Open {distro} through vibe-bridge capture"),
        )?;
        installed.push(distro);
    }
    let (names, count) = limited_profile_name_list(installed);
    let restore_note = if restored_direct {
        "; restored direct Ubuntu/WSL shortcut(s)"
    } else {
        ""
    };
    Ok(format!(
        "installed {count} Start Menu shortcut(s): {names}{restore_note}"
    ))
}

#[cfg(windows)]
fn write_windows_shortcut(
    path: &std::path::Path,
    target: &std::path::Path,
    arguments: &str,
    description: &str,
) -> Result<(), String> {
    let script = format!(
        "$shell = New-Object -ComObject WScript.Shell; \
         $shortcut = $shell.CreateShortcut({}); \
         $shortcut.TargetPath = {}; \
         $shortcut.Arguments = {}; \
         $shortcut.WorkingDirectory = $env:USERPROFILE; \
         $shortcut.Description = {}; \
         $shortcut.Save()",
        ps_quote(&shell_display_path(path)),
        ps_quote(&shell_display_path(target)),
        ps_quote(arguments),
        ps_quote(description)
    );
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output()
        .map_err(|err| format!("run powershell to create shortcut: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "powershell shortcut creation exited with {}; stderr={}",
            output.status,
            decode_windows_command_output(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn shortcut_is_vibe_bridge(path: &std::path::Path) -> bool {
    read_windows_shortcut(path)
        .map(|shortcut| shortcut_summary_is_vibe_bridge(&shortcut))
        .unwrap_or(false)
}

#[cfg(windows)]
fn read_windows_shortcut(path: &std::path::Path) -> Result<String, String> {
    let script = format!(
        "$shell = New-Object -ComObject WScript.Shell; \
         $shortcut = $shell.CreateShortcut({}); \
         Write-Output $shortcut.TargetPath; \
         Write-Output $shortcut.Arguments",
        ps_quote(&shell_display_path(path))
    );
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output()
        .map_err(|err| format!("run powershell to read shortcut: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "powershell shortcut read exited with {}; stderr={}",
            output.status,
            decode_windows_command_output(&output.stderr).trim()
        ));
    }
    Ok(decode_windows_command_output(&output.stdout))
}

fn shortcut_summary_is_vibe_bridge(summary: &str) -> bool {
    let lower = summary.to_ascii_lowercase();
    lower.contains("vibe-bridge") || lower.contains("terminal-shim")
}

#[cfg(windows)]
fn remove_wsl_start_menu_shortcuts() -> Result<bool, String> {
    let direct_changed = restore_direct_wsl_start_menu_shortcuts()?;
    let dir = wsl_start_menu_shortcuts_dir()?;
    if !dir.exists() {
        return Ok(direct_changed);
    }
    std::fs::remove_dir_all(&dir).map_err(|err| format!("remove {}: {err}", dir.display()))?;
    Ok(true)
}

#[cfg(windows)]
fn restore_direct_wsl_start_menu_shortcuts() -> Result<bool, String> {
    let dir = start_menu_programs_dir()?;
    let mut changed = false;
    if dir.exists() {
        for entry in std::fs::read_dir(&dir)
            .map_err(|err| format!("read Start Menu programs dir {}: {err}", dir.display()))?
        {
            let entry = entry.map_err(|err| format!("read Start Menu entry: {err}"))?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !name.ends_with(".lnk.vibe-bridge-backup") {
                continue;
            }
            let original_name = name.trim_end_matches(".vibe-bridge-backup");
            let original = path.with_file_name(original_name);
            let _ = std::fs::remove_file(&original);
            std::fs::rename(&path, &original).map_err(|err| {
                format!(
                    "restore Start Menu shortcut {} -> {}: {err}",
                    path.display(),
                    original.display()
                )
            })?;
            changed = true;
        }
    }

    for distro in list_wsl_distros().unwrap_or_default() {
        let path = dir.join(format!("{}.lnk", sanitize_windows_shortcut_name(&distro)));
        if path.exists() && shortcut_is_vibe_bridge(&path) {
            std::fs::remove_file(&path)
                .map_err(|err| format!("remove {}: {err}", path.display()))?;
            changed = true;
        }
    }
    Ok(changed)
}

#[cfg(windows)]
fn sanitize_windows_shortcut_name(value: &str) -> String {
    let mut out = String::with_capacity(value.len().max(1));
    for ch in value.chars() {
        if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || ch.is_control() {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    let out = out.trim().trim_end_matches('.').to_string();
    if out.is_empty() {
        "WSL".to_string()
    } else {
        out
    }
}

#[cfg(windows)]
fn remove_agent_cmds(shim_dir: &std::path::Path) -> Result<usize, String> {
    let mut removed = 0usize;
    for name in ["codex.cmd", "claude.cmd", "wsl.cmd"] {
        let path = shim_dir.join(name);
        if !path.exists() {
            continue;
        }
        std::fs::remove_file(&path).map_err(|err| format!("remove {}: {err}", path.display()))?;
        removed += 1;
    }
    Ok(removed)
}

#[cfg(windows)]
fn cleanup_daemon_exes(shim_dir: &std::path::Path) -> Result<(usize, usize), String> {
    if !shim_dir.exists() {
        return Ok((0, 0));
    }
    let mut removed = 0usize;
    let mut skipped = 0usize;
    for entry in std::fs::read_dir(shim_dir)
        .map_err(|err| format!("read shim dir {}: {err}", shim_dir.display()))?
    {
        let entry = entry.map_err(|err| format!("read shim dir entry: {err}"))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        if lower == "vb-daemon.exe" || (lower.starts_with("vb-daemon-") && lower.ends_with(".exe"))
        {
            match std::fs::remove_file(&path) {
                Ok(()) => removed += 1,
                Err(_) => skipped += 1,
            }
        }
    }
    Ok((removed, skipped))
}

#[cfg(windows)]
fn purge_local_state() -> Result<(), String> {
    let dir = local_appdata_dir()?.join("vibe-bridge");
    let _ = std::fs::remove_file(dir.join("daemon-status.json"));
    let _ = std::fs::remove_file(dir.join("vb-daemon.log"));
    let _ = std::fs::remove_dir(dir.join("bin"));
    let _ = std::fs::remove_dir(dir);
    Ok(())
}

#[cfg(windows)]
fn ensure_user_path_contains(dir: &std::path::Path) -> Result<(), String> {
    let dir = shell_display_path(dir);
    let script = format!(
        "$dir = '{}'; \
         $path = [Environment]::GetEnvironmentVariable('Path', 'User'); \
         if ([string]::IsNullOrEmpty($path)) {{ $parts = @() }} else {{ $parts = $path -split ';' }}; \
         if (-not ($parts | Where-Object {{ $_ -ieq $dir }})) {{ \
           $newPath = if ([string]::IsNullOrEmpty($path)) {{ $dir }} else {{ $dir + ';' + $path }}; \
           [Environment]::SetEnvironmentVariable('Path', $newPath, 'User') \
         }}",
        dir.replace('\'', "''")
    );
    let status = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .status()
        .map_err(|err| format!("run powershell to update user PATH: {err}"))?;
    if !status.success() {
        return Err(format!("powershell PATH update exited with {status}"));
    }
    Ok(())
}

#[cfg(windows)]
fn shell_display_path(path: &std::path::Path) -> String {
    let raw = path.to_string_lossy();
    strip_windows_verbatim_prefix(&raw).to_string()
}

fn strip_windows_verbatim_prefix(path: &str) -> &str {
    path.strip_prefix(r"\\?\")
        .or_else(|| path.strip_prefix(r"\??\"))
        .unwrap_or(path)
}

#[cfg(windows)]
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
fn install_windows_terminal_profiles(exe: &std::path::Path, addr: &str) -> Result<String, String> {
    let mut touched_files = 0usize;
    let mut touched_profiles = 0usize;
    let mut touched_profile_names = Vec::new();
    let mut supported_profile_names = Vec::new();
    let mut seen_settings = 0usize;
    for path in windows_terminal_settings_paths()? {
        if !path.exists() {
            continue;
        }
        seen_settings += 1;
        let input = std::fs::read_to_string(&path)
            .map_err(|err| format!("read {}: {err}", path.display()))?;
        let result = wrap_windows_terminal_settings_json(&input, exe, addr)?;
        supported_profile_names.extend(result.supported_profiles);
        let Some(output) = result.output else {
            continue;
        };
        let backup = terminal_settings_backup_path(&path);
        std::fs::copy(&path, &backup).map_err(|err| {
            format!(
                "backup Windows Terminal settings {} -> {}: {err}",
                path.display(),
                backup.display()
            )
        })?;
        std::fs::write(&path, output)
            .map_err(|err| format!("write Windows Terminal settings {}: {err}", path.display()))?;
        touched_files += 1;
        touched_profiles += result.changed_profiles.len();
        touched_profile_names.extend(result.changed_profiles);
    }

    if seen_settings == 0 {
        Ok("no Windows Terminal settings found".to_string())
    } else if touched_profiles == 0 {
        if supported_profile_names.is_empty() {
            Ok("no supported profiles".to_string())
        } else {
            let (names, count) = limited_profile_name_list(supported_profile_names);
            Ok(format!(
                "already installed {count} supported profile(s): {names}"
            ))
        }
    } else {
        let (names, _) = limited_profile_name_list(touched_profile_names);
        Ok(format!(
            "wrapped {touched_profiles} profile(s) in {touched_files} settings file(s): {names}"
        ))
    }
}

#[cfg(windows)]
fn uninstall_windows_terminal_profiles() -> Result<String, String> {
    let mut touched_files = 0usize;
    let mut touched_profiles = 0usize;
    let mut touched_profile_names = Vec::new();
    let mut seen_settings = 0usize;
    for path in windows_terminal_settings_paths()? {
        if !path.exists() {
            continue;
        }
        seen_settings += 1;
        let input = std::fs::read_to_string(&path)
            .map_err(|err| format!("read {}: {err}", path.display()))?;
        let result = unwrap_windows_terminal_settings_json(&input)?;
        let Some(output) = result.output else {
            continue;
        };
        let backup = terminal_settings_backup_path(&path);
        std::fs::copy(&path, &backup).map_err(|err| {
            format!(
                "backup Windows Terminal settings {} -> {}: {err}",
                path.display(),
                backup.display()
            )
        })?;
        std::fs::write(&path, output)
            .map_err(|err| format!("write Windows Terminal settings {}: {err}", path.display()))?;
        touched_files += 1;
        touched_profiles += result.changed_profiles.len();
        touched_profile_names.extend(result.changed_profiles);
    }

    if seen_settings == 0 {
        Ok("no Windows Terminal settings found".to_string())
    } else if touched_profiles == 0 {
        Ok("no wrapped profiles".to_string())
    } else {
        let (names, _) = limited_profile_name_list(touched_profile_names);
        Ok(format!(
            "unwrapped {touched_profiles} profile(s) in {touched_files} settings file(s): {names}"
        ))
    }
}

#[cfg(windows)]
fn windows_terminal_settings_paths() -> Result<Vec<std::path::PathBuf>, String> {
    let local = local_appdata_dir()?;
    Ok(vec![
        local
            .join("Packages")
            .join("Microsoft.WindowsTerminal_8wekyb3d8bbwe")
            .join("LocalState")
            .join("settings.json"),
        local
            .join("Packages")
            .join("Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe")
            .join("LocalState")
            .join("settings.json"),
        local
            .join("Microsoft")
            .join("Windows Terminal")
            .join("settings.json"),
    ])
}

#[cfg(windows)]
fn terminal_settings_backup_path(path: &std::path::Path) -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("settings.json");
    path.with_file_name(format!("{file_name}.vibe-bridge-backup.{stamp}"))
}

struct TerminalSettingsWrapResult {
    output: Option<String>,
    changed_profiles: Vec<String>,
    supported_profiles: Vec<String>,
}

struct TerminalSettingsUnwrapResult {
    output: Option<String>,
    changed_profiles: Vec<String>,
}

fn wrap_windows_terminal_settings_json(
    input: &str,
    exe: &std::path::Path,
    addr: &str,
) -> Result<TerminalSettingsWrapResult, String> {
    let stripped = strip_jsonc_comments(input);
    let mut root: serde_json::Value =
        serde_json::from_str(&stripped).map_err(|err| format!("parse settings json: {err}"))?;
    let profiles = root
        .get_mut("profiles")
        .and_then(|value| value.get_mut("list"))
        .and_then(|value| value.as_array_mut())
        .ok_or_else(|| "Windows Terminal settings missing profiles.list".to_string())?;

    let mut changed_profiles = Vec::new();
    let mut supported_profiles = Vec::new();
    for profile in profiles {
        let Some(profile_obj) = profile.as_object_mut() else {
            continue;
        };
        if profile_obj
            .get("hidden")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let name = profile_obj
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("Windows Terminal")
            .to_string();
        let Some(profile_cmdline) = terminal_profile_original_commandline(profile_obj) else {
            continue;
        };
        supported_profiles.push(name.clone());
        let original =
            extract_wrapped_terminal_cmdline(&profile_cmdline).unwrap_or(profile_cmdline);
        let capture_name = format!("Terminal: {name}");
        let wrapped = build_terminal_profile_commandline(exe, addr, &capture_name, &original);
        if profile_obj
            .get("commandline")
            .and_then(|value| value.as_str())
            == Some(wrapped.as_str())
        {
            continue;
        }
        profile_obj.insert(
            "commandline".to_string(),
            serde_json::Value::String(wrapped),
        );
        changed_profiles.push(name);
    }

    let output = if changed_profiles.is_empty() {
        None
    } else {
        let mut output = serde_json::to_string_pretty(&root)
            .map_err(|err| format!("serialize settings json: {err}"))?;
        output.push('\n');
        Some(output)
    };
    Ok(TerminalSettingsWrapResult {
        output,
        changed_profiles,
        supported_profiles,
    })
}

fn unwrap_windows_terminal_settings_json(
    input: &str,
) -> Result<TerminalSettingsUnwrapResult, String> {
    let stripped = strip_jsonc_comments(input);
    let mut root: serde_json::Value =
        serde_json::from_str(&stripped).map_err(|err| format!("parse settings json: {err}"))?;
    let profiles = root
        .get_mut("profiles")
        .and_then(|value| value.get_mut("list"))
        .and_then(|value| value.as_array_mut())
        .ok_or_else(|| "Windows Terminal settings missing profiles.list".to_string())?;

    let mut changed_profiles = Vec::new();
    for profile in profiles {
        let Some(profile_obj) = profile.as_object_mut() else {
            continue;
        };
        let Some(commandline) = profile_obj
            .get("commandline")
            .and_then(|value| value.as_str())
            .map(str::to_string)
        else {
            continue;
        };
        let Some(original) = extract_wrapped_terminal_cmdline(&commandline) else {
            continue;
        };
        let name = profile_obj
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("Windows Terminal")
            .to_string();
        profile_obj.insert(
            "commandline".to_string(),
            serde_json::Value::String(original),
        );
        changed_profiles.push(name);
    }

    let output = if changed_profiles.is_empty() {
        None
    } else {
        let mut output = serde_json::to_string_pretty(&root)
            .map_err(|err| format!("serialize settings json: {err}"))?;
        output.push('\n');
        Some(output)
    };
    Ok(TerminalSettingsUnwrapResult {
        output,
        changed_profiles,
    })
}

#[cfg(windows)]
fn limited_profile_name_list(names: Vec<String>) -> (String, usize) {
    let count = names.len();
    let mut shown = names.into_iter().take(8).collect::<Vec<_>>();
    if count > shown.len() {
        shown.push(format!("... {} more", count - shown.len()));
    }
    (shown.join(", "), count)
}

fn terminal_profile_original_commandline(
    profile: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    if let Some(commandline) = profile
        .get("commandline")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(commandline.to_string());
    }

    let name = profile
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let source = profile
        .get("source")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let lower_name = name.to_ascii_lowercase();
    let lower_source = source.to_ascii_lowercase();

    if lower_source.contains("wsl")
        || lower_name.contains("ubuntu")
        || lower_name.contains("debian")
    {
        if name.trim().is_empty() {
            return Some("wsl.exe".to_string());
        }
        return Some(format!("wsl.exe -d {}", quote_windows_arg(name)));
    }
    if lower_source.contains("powershellcore")
        || lower_name == "powershell"
        || lower_name.contains("powershell 7")
    {
        return Some("pwsh.exe".to_string());
    }
    if lower_source.contains("windowspowershell") || lower_name.contains("windows powershell") {
        return Some("powershell.exe".to_string());
    }
    if lower_source.contains("visualstudio") && lower_name.contains("powershell") {
        return Some("powershell.exe".to_string());
    }
    if lower_source.contains("cmd") || lower_name.contains("command prompt") {
        return Some("cmd.exe".to_string());
    }
    if lower_name.contains("powershell") {
        return Some("powershell.exe".to_string());
    }

    None
}

fn extract_wrapped_terminal_cmdline(commandline: &str) -> Option<String> {
    if !commandline.contains("terminal-shim")
        && !commandline.contains("capture-shell")
        && !commandline.contains("vibe-terminal")
    {
        return None;
    }
    let mut parts = commandline.split_whitespace();
    while let Some(part) = parts.next() {
        if part == "--cmdline-b64" {
            let encoded = parts.next()?.trim_matches('"');
            let bytes = base64_decode_bytes(encoded).ok()?;
            return String::from_utf8(bytes).ok();
        }
    }
    None
}

fn build_terminal_profile_commandline(
    exe: &std::path::Path,
    addr: &str,
    name: &str,
    original_cmdline: &str,
) -> String {
    format!(
        "{} terminal-shim --daemon {} --name {} --follow-console-size --cmdline-b64 {}",
        quote_windows_arg(&exe.to_string_lossy()),
        quote_windows_arg(addr),
        quote_windows_arg(name),
        base64_encode_bytes(original_cmdline.as_bytes())
    )
}

fn quote_windows_arg(value: &str) -> String {
    if !value.is_empty()
        && !value
            .chars()
            .any(|ch| ch.is_whitespace() || ch == '"' || ch == '\\')
    {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for ch in value.chars() {
        if ch == '\\' {
            backslashes += 1;
            continue;
        }
        if ch == '"' {
            out.push_str(&"\\".repeat(backslashes * 2 + 1));
            out.push('"');
        } else {
            out.push_str(&"\\".repeat(backslashes));
            out.push(ch);
        }
        backslashes = 0;
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

fn strip_jsonc_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }

        if ch == '/' {
            match chars.peek().copied() {
                Some('/') => {
                    let _ = chars.next();
                    for next in chars.by_ref() {
                        if next == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
                    let _ = chars.next();
                    let mut prev = '\0';
                    for next in chars.by_ref() {
                        if prev == '*' && next == '/' {
                            break;
                        }
                        prev = next;
                    }
                    continue;
                }
                _ => {}
            }
        }

        out.push(ch);
    }
    out
}

#[cfg(windows)]
fn resolve_wsl_distros(opts: &InstallWindowsOpts) -> Result<Vec<String>, String> {
    if !opts.install_wsl {
        return Ok(Vec::new());
    }
    if !opts.wsl_distros.is_empty() {
        return Ok(opts.wsl_distros.clone());
    }
    match list_wsl_distros() {
        Ok(distros) => Ok(distros),
        Err(err) => {
            eprintln!("warning: WSL distro enumeration failed: {err}");
            Ok(Vec::new())
        }
    }
}

#[cfg(windows)]
fn resolve_wsl_shell_distros(opts: &InstallWindowsOpts) -> Result<Vec<String>, String> {
    if !opts.wsl_distros.is_empty() {
        return Ok(opts.wsl_distros.clone());
    }
    list_wsl_distros()
}

#[cfg(windows)]
fn list_wsl_distros() -> Result<Vec<String>, String> {
    let output = std::process::Command::new("wsl.exe")
        .args(["-l", "-q"])
        .output()
        .map_err(|err| format!("run wsl.exe -l -q: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "wsl.exe -l -q exited with {}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = decode_windows_command_output(&output.stdout);
    let distros = text
        .lines()
        .map(|line| line.trim_matches(|ch: char| ch == '\0' || ch.is_whitespace()))
        .filter(|line| !line.is_empty())
        .filter(|line| !line.eq_ignore_ascii_case("docker-desktop"))
        .filter(|line| !line.eq_ignore_ascii_case("docker-desktop-data"))
        .map(ToOwned::to_owned)
        .collect();
    Ok(distros)
}

#[cfg(windows)]
fn decode_windows_command_output(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && (bytes[0] == 0xff && bytes[1] == 0xfe || bytes[1] == 0) {
        let start = if bytes.starts_with(&[0xff, 0xfe]) {
            2
        } else {
            0
        };
        let units: Vec<u16> = bytes[start..]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

#[cfg(windows)]
fn install_wsl_distro(
    distro: &str,
    daemon_exe: &std::path::Path,
    addr: &str,
) -> Result<(), String> {
    let hook_js = include_str!("../../../adapters/claude-code-hook/index.js");
    let script = build_wsl_install_script(distro, daemon_exe, addr, hook_js);
    run_wsl_script(distro, &script)
}

#[cfg(windows)]
fn install_wsl_shell_integration(distro: &str, addr: &str) -> Result<(), String> {
    let script = build_wsl_shell_integration_script(addr);
    run_wsl_script(distro, &script)
}

#[cfg(windows)]
fn remove_wsl_shell_integrations(addr: &str) -> Result<Vec<String>, String> {
    let mut removed = Vec::new();
    for distro in list_wsl_distros()? {
        let script = build_wsl_shell_uninstall_script(addr);
        match run_wsl_script(&distro, &script) {
            Ok(()) => removed.push(distro),
            Err(err) => {
                eprintln!("warning: WSL shell integration uninstall {distro} failed: {err}")
            }
        }
    }
    Ok(removed)
}

fn build_wsl_shell_integration_script(addr: &str) -> String {
    format!(
        r#"set -eu
VB_ADDR={addr}
VB_DIR="$HOME/.local/share/vibe-bridge/shell-integration"
SHIM_DIR="$VB_DIR/bin"
mkdir -p "$SHIM_DIR"

cat > "$SHIM_DIR/codex" <<'WRAP'
#!/usr/bin/env bash
# vibe-bridge WSL shell integration agent shim
set -u
cmd="$(basename "$0")"
shim_dir="${{VIBE_BRIDGE_WSL_SHELL_SHIM_DIR:-$HOME/.local/share/vibe-bridge/shell-integration/bin}}"
daemon="${{VIBE_BRIDGE_DAEMON:-127.0.0.1:8765}}"
host="${{daemon%:*}}"
port="${{daemon##*:}}"
agent_id="wsl-${{cmd}}-$$"
cwd="$(pwd 2>/dev/null || true)"

is_vibe_wrapper() {{
  [ -f "$1" ] && grep -q 'vibe-bridge WSL .*agent shim\|vibe-bridge WSL agent wrapper\|vibe-bridge WSL shell integration agent shim' "$1" 2>/dev/null
}}

find_real() {{
  local old_ifs="$IFS"
  local dir candidate
  IFS=:
  for dir in $PATH; do
    [ -n "$dir" ] || dir=.
    [ "$dir" = "$shim_dir" ] && continue
    candidate="$dir/$cmd"
    if [ -x "$candidate" ] && ! is_vibe_wrapper "$candidate"; then
      printf '%s\n' "$candidate"
      IFS="$old_ifs"
      return 0
    fi
  done
  IFS="$old_ifs"
  for candidate in "$HOME/.local/bin/$cmd" "/usr/local/bin/$cmd" "/usr/bin/$cmd"; do
    if [ -x "$candidate" ] && ! is_vibe_wrapper "$candidate"; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}}

json_escape() {{
  local s="$1"
  s="${{s//\\/\\\\}}"
  s="${{s//\"/\\\"}}"
  s="${{s//$'\r'/ }}"
  s="${{s//$'\n'/ }}"
  printf '%s' "$s"
}}

send_daemon() {{
  local json="$1"
  if {{ exec 9<>"/dev/tcp/$host/$port"; }} 2>/dev/null; then
    printf '%s\n' "$json" >&9 || true
    IFS= read -r _ <&9 || true
    exec 9>&- 9<&- || true
  fi
}}

register_agent() {{
  local cwd_json
  cwd_json="$(json_escape "$cwd")"
  send_daemon "$(printf '{{"type":"agent.register","agent":{{"agentId":"%s","kind":"%s","name":"%s","cwd":"%s","fromLaunch":true}}}}' "$agent_id" "$cmd" "$cmd" "$cwd_json")"
}}

abort_agent() {{
  send_daemon "$(printf '{{"type":"session.abort","abort":{{"agentId":"%s","kind":"%s"}}}}' "$agent_id" "$cmd")"
}}

stream_chunk() {{
  local chunk="$1"
  local hex
  hex="$(printf '%s' "$chunk" | od -An -tx1 -v | tr -d ' \n')"
  [ -n "$hex" ] || return 0
  send_daemon "$(printf '{{"type":"terminal.stream","stream":{{"agentId":"%s","kind":"%s","dataHex":"%s"}}}}' "$agent_id" "$cmd" "$hex")"
}}

real="$(find_real || true)"
if [ -z "$real" ]; then
  echo "vibe-bridge: real $cmd not found in WSL PATH" >&2
  exit 127
fi

if ! command -v script >/dev/null 2>&1; then
  echo "vibe-bridge: util-linux script(1) not found; running $cmd without terminal capture" >&2
  exec "$real" "$@"
fi

register_agent
tmp="${{TMPDIR:-/tmp}}/vibe-bridge-wsl-pty-${{cmd}}-$$"
fifo="$tmp/out"
mkdir -p "$tmp"
mkfifo "$fifo"

stream_loop() {{
  local chunk status
  while true; do
    chunk=""
    IFS= read -r -t 0.03 -N 4096 chunk
    status=$?
    if [ -n "$chunk" ]; then
      printf '%s' "$chunk"
      stream_chunk "$chunk"
    fi
    [ "$status" -eq 142 ] && continue
    [ "$status" -eq 0 ] && continue
    break
  done < "$fifo"
}}

stream_loop &
stream_pid=$!
cmdline=""
printf -v cmdline '%q ' "$real" "$@"
script -qfec "$cmdline" /dev/null > "$fifo"
rc=$?
wait "$stream_pid" 2>/dev/null || true
rm -rf "$tmp"
abort_agent
exit "$rc"
WRAP

cp "$SHIM_DIR/codex" "$SHIM_DIR/claude"
chmod +x "$SHIM_DIR/codex" "$SHIM_DIR/claude"

install_rc_block() {{
  rc="$1"
  touch "$rc"
  remove_rc_block "$rc"
  cat >> "$rc" <<RC

# >>> vibe-bridge WSL shell integration >>>
export VIBE_BRIDGE_DAEMON="\${{VIBE_BRIDGE_DAEMON:-$VB_ADDR}}"
export VIBE_BRIDGE_WSL_SHELL_SHIM_DIR="\$HOME/.local/share/vibe-bridge/shell-integration/bin"
if [ -d "\$VIBE_BRIDGE_WSL_SHELL_SHIM_DIR" ]; then
  _vb_old_ifs="\$IFS"
  _vb_new_path=""
  IFS=:
  for _vb_dir in \$PATH; do
    [ "\$_vb_dir" = "\$VIBE_BRIDGE_WSL_SHELL_SHIM_DIR" ] && continue
    if [ -z "\$_vb_new_path" ]; then
      _vb_new_path="\$_vb_dir"
    else
      _vb_new_path="\$_vb_new_path:\$_vb_dir"
    fi
  done
  IFS="\$_vb_old_ifs"
  if [ -n "\$_vb_new_path" ]; then
    export PATH="\$VIBE_BRIDGE_WSL_SHELL_SHIM_DIR:\$_vb_new_path"
  else
    export PATH="\$VIBE_BRIDGE_WSL_SHELL_SHIM_DIR"
  fi
  unset _vb_old_ifs _vb_new_path _vb_dir
fi
# <<< vibe-bridge WSL shell integration <<<
RC
}}

remove_rc_block() {{
  rc="$1"
  [ -f "$rc" ] || return 0
  tmp="$rc.vibe-bridge-tmp.$$"
  awk '
    /# >>> vibe-bridge WSL shell integration >>>/ {{ skip=1; next }}
    /# <<< vibe-bridge WSL shell integration <<</ {{ skip=0; next }}
    skip != 1 {{ print }}
  ' "$rc" > "$tmp"
  mv "$tmp" "$rc"
}}

install_rc_block "$HOME/.bashrc"
[ -e "$HOME/.profile" ] && install_rc_block "$HOME/.profile" || true
[ -e "$HOME/.bash_profile" ] && install_rc_block "$HOME/.bash_profile" || true
[ -e "$HOME/.bash_login" ] && install_rc_block "$HOME/.bash_login" || true
[ -e "$HOME/.zshrc" ] && install_rc_block "$HOME/.zshrc" || true
"#,
        addr = sh_quote(addr),
    )
}

fn build_wsl_shell_uninstall_script(_addr: &str) -> String {
    r#"set -eu
remove_rc_block() {
  rc="$1"
  [ -f "$rc" ] || return 0
  tmp="$rc.vibe-bridge-tmp.$$"
  awk '
    /# >>> vibe-bridge WSL shell integration >>>/ { skip=1; next }
    /# <<< vibe-bridge WSL shell integration <<</ { skip=0; next }
    skip != 1 { print }
  ' "$rc" > "$tmp"
  mv "$tmp" "$rc"
}

remove_rc_block "$HOME/.bashrc"
[ -e "$HOME/.profile" ] && remove_rc_block "$HOME/.profile" || true
[ -e "$HOME/.bash_profile" ] && remove_rc_block "$HOME/.bash_profile" || true
[ -e "$HOME/.bash_login" ] && remove_rc_block "$HOME/.bash_login" || true
[ -e "$HOME/.zshrc" ] && remove_rc_block "$HOME/.zshrc" || true
rm -rf "$HOME/.local/share/vibe-bridge/shell-integration"
"#
    .to_string()
}

#[cfg(windows)]
fn build_wsl_install_script(
    distro: &str,
    daemon_exe: &std::path::Path,
    addr: &str,
    hook_js: &str,
) -> String {
    let daemon_unix = windows_path_to_wsl_path(daemon_exe)
        .unwrap_or_else(|| daemon_exe.to_string_lossy().replace('\\', "/"));
    let (host, port) = addr
        .rsplit_once(':')
        .map(|(host, port)| (host, port))
        .unwrap_or(("127.0.0.1", "8765"));
    format!(
        r#"set -eu
DAEMON_UNIX={daemon_unix}
DISTRO={distro}
HOOK_HOST={host}
HOOK_PORT={port}
VB_DIR="$HOME/.local/share/vibe-bridge"
BIN="$HOME/.local/bin"
REAL="$VB_DIR/real-bin"
HOOK_DIR="$VB_DIR/claude-code-hook"
mkdir -p "$BIN" "$REAL" "$HOOK_DIR" "$HOME/.claude"

install_agent_wrapper() {{
  name="$1"
  wrapper="$BIN/$name"
  real="$REAL/$name"
  current="$(command -v "$name" 2>/dev/null || true)"
  if [ -n "$current" ] && [ "$current" != "$wrapper" ] && [ ! -e "$real" ]; then
    ln -s "$current" "$real" 2>/dev/null || true
  fi
  if [ -e "$wrapper" ] || [ -L "$wrapper" ]; then
    if grep -q 'vibe-bridge WSL agent wrapper' "$wrapper" 2>/dev/null; then
      :
    elif [ ! -e "$real" ]; then
      if [ -L "$wrapper" ]; then
        target="$(readlink -f "$wrapper" 2>/dev/null || true)"
        if [ -n "$target" ]; then
          ln -s "$target" "$real" 2>/dev/null || mv "$wrapper" "$real"
        else
          mv "$wrapper" "$real"
        fi
      else
        mv "$wrapper" "$real"
      fi
    else
      mv "$wrapper" "$wrapper.vibe-bridge-backup.$(date +%s)"
    fi
  fi
  cat > "$wrapper" <<WRAP
#!/bin/sh
# vibe-bridge WSL agent wrapper
export VIBE_BRIDGE_WSL_DISTRO="$DISTRO"
real="$real"
if "$DAEMON_UNIX" --help >/dev/null 2>&1; then
  exec "$DAEMON_UNIX" agent-shim $name "\$@"
fi
echo "vibe-bridge: daemon executable not available at $DAEMON_UNIX; running real $name without capture" >&2
if [ -x "$real" ]; then
  exec "$real" "\$@"
fi
echo "vibe-bridge: real $name not found at $real" >&2
exit 127
WRAP
  chmod +x "$wrapper"
}}

install_agent_wrapper codex
install_agent_wrapper claude

HOOK_B64={hook_b64}
printf '%s' "$HOOK_B64" | base64 -d > "$HOOK_DIR/index.js"
chmod +x "$HOOK_DIR/index.js"

install_path_block() {{
  rc="$1"
  touch "$rc"
  if ! grep -q 'vibe-bridge WSL wrappers' "$rc" 2>/dev/null; then
    cat >> "$rc" <<'RC'

# >>> vibe-bridge WSL wrappers >>>
export PATH="$HOME/.local/bin:$PATH"
# <<< vibe-bridge WSL wrappers <<<
RC
  fi
}}
install_path_block "$HOME/.bashrc"
[ -e "$HOME/.zshrc" ] && install_path_block "$HOME/.zshrc" || true

if command -v node >/dev/null 2>&1; then
  VIBE_BRIDGE_HOOK_PATH="$HOOK_DIR/index.js" VIBE_BRIDGE_HOOK_HOST="$HOOK_HOST" VIBE_BRIDGE_HOOK_PORT="$HOOK_PORT" node <<'NODE'
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const hookPath = process.env.VIBE_BRIDGE_HOOK_PATH;
const host = process.env.VIBE_BRIDGE_HOOK_HOST || '127.0.0.1';
const port = process.env.VIBE_BRIDGE_HOOK_PORT || '8765';
const settingsDir = path.join(os.homedir(), '.claude');
const settingsPath = path.join(settingsDir, 'settings.json');
fs.mkdirSync(settingsDir, {{ recursive: true }});
let settings = {{}};
try {{ settings = JSON.parse(fs.readFileSync(settingsPath, 'utf8')); }} catch {{ settings = {{}}; }}
if (!settings.hooks || typeof settings.hooks !== 'object' || Array.isArray(settings.hooks)) settings.hooks = {{}};
for (const event of ['SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'Stop', 'SessionEnd']) {{
  const command = `VIBE_BRIDGE_HOST=${{host}} VIBE_BRIDGE_PORT=${{port}} node ${{hookPath}} ${{event}}`;
  const entry = {{ matcher: '*', hooks: [{{ type: 'command', command }}] }};
  const current = Array.isArray(settings.hooks[event]) ? settings.hooks[event] : [];
  if (!JSON.stringify(current).includes(hookPath)) current.push(entry);
  settings.hooks[event] = current;
}}
fs.writeFileSync(settingsPath, JSON.stringify(settings, null, 2) + '\n');
NODE
else
  echo 'warning: node not found; Claude Code hook not installed' >&2
fi
"#,
        daemon_unix = sh_quote(&daemon_unix),
        distro = sh_quote(distro),
        host = sh_quote(host),
        port = sh_quote(port),
        hook_b64 = sh_quote(&base64_encode_bytes(hook_js.as_bytes())),
    )
}

#[cfg(windows)]
fn run_wsl_script(distro: &str, script: &str) -> Result<(), String> {
    let encoded = base64_encode_bytes(script.as_bytes());
    let decoder = format!("printf '%s' {} | base64 -d | sh", sh_quote(&encoded));
    let output = std::process::Command::new("wsl.exe")
        .args(["-d", distro, "--", "sh", "-lc", &decoder])
        .output()
        .map_err(|err| format!("run wsl.exe for {distro}: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "wsl install exited with {}; stdout={}; stderr={}",
            output.status,
            decode_windows_command_output(&output.stdout).trim(),
            decode_windows_command_output(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn windows_path_to_wsl_path(path: &std::path::Path) -> Option<String> {
    let raw = path.to_string_lossy();
    let raw = raw.strip_prefix(r#"\\?\"#).unwrap_or(&raw);
    let mut chars = raw.chars();
    let drive = chars.next()?.to_ascii_lowercase();
    if !drive.is_ascii_alphabetic() || chars.next()? != ':' {
        return None;
    }
    let rest: String = chars.collect();
    let rest = rest.replace('\\', "/");
    Some(format!("/mnt/{drive}{rest}"))
}

fn sh_quote(value: &str) -> String {
    let escaped = value.replace("'", "'\"'\"'");
    let mut out = String::with_capacity(escaped.len() + 2);
    out.push_str("'");
    out.push_str(&escaped);
    out.push_str("'");
    out
}

fn base64_encode_bytes(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() >= 2 {
            out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() >= 3 {
            out.push(ALPHABET[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn base64_decode_bytes(input: &str) -> Result<Vec<u8>, String> {
    let mut clean = Vec::new();
    for byte in input.bytes() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        clean.push(byte);
    }
    if clean.len() % 4 != 0 {
        return Err("base64 length is not a multiple of 4".to_string());
    }

    let mut out = Vec::with_capacity(clean.len() / 4 * 3);
    for chunk in clean.chunks(4) {
        let mut values = [0u8; 4];
        let mut padding = 0usize;
        for (idx, byte) in chunk.iter().copied().enumerate() {
            if byte == b'=' {
                padding += 1;
                values[idx] = 0;
            } else if padding > 0 {
                return Err("base64 padding appears before the end".to_string());
            } else {
                values[idx] = match byte {
                    b'A'..=b'Z' => byte - b'A',
                    b'a'..=b'z' => byte - b'a' + 26,
                    b'0'..=b'9' => byte - b'0' + 52,
                    b'+' => 62,
                    b'/' => 63,
                    _ => return Err(format!("invalid base64 byte: 0x{byte:02x}")),
                };
            }
        }
        if padding > 2 {
            return Err("base64 has too much padding".to_string());
        }
        let n = ((values[0] as u32) << 18)
            | ((values[1] as u32) << 12)
            | ((values[2] as u32) << 6)
            | values[3] as u32;
        out.push(((n >> 16) & 0xff) as u8);
        if padding < 2 {
            out.push(((n >> 8) & 0xff) as u8);
        }
        if padding < 1 {
            out.push((n & 0xff) as u8);
        }
    }
    Ok(out)
}

#[cfg(windows)]
fn local_appdata_dir() -> Result<std::path::PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "LOCALAPPDATA is not set".to_string())
}

#[cfg(windows)]
fn startup_dir() -> Result<std::path::PathBuf, String> {
    let appdata = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "APPDATA is not set".to_string())?;
    Ok(appdata
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup"))
}

#[cfg(windows)]
fn run_agent_shim(raw: Vec<String>) -> Result<u32, String> {
    let (kind, agent_args) = raw
        .split_first()
        .ok_or_else(|| "usage: agent-shim codex|claude [ARGS...]".to_string())?;
    if kind != "codex" && kind != "claude" {
        return Err(format!("unsupported agent kind: {kind}"));
    }
    let addr = std::env::var("VIBE_BRIDGE_DAEMON").unwrap_or_else(|_| "127.0.0.1:8765".to_string());
    let parent_terminal_agent_id = std::env::var("VIBE_BRIDGE_TERMINAL_AGENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let captured = std::env::var_os("VIBE_BRIDGE_CAPTURED_TERMINAL").is_some();
    append_terminal_shim_log(format!(
        "[agent-shim] start pid={} kind={} captured={} parent={} args={}",
        std::process::id(),
        terminal_shim_log_value(kind),
        captured,
        terminal_shim_log_value(parent_terminal_agent_id.as_deref().unwrap_or("")),
        agent_args.len()
    ));
    ensure_daemon_listening(&addr)?;
    let command = resolve_agent_command(kind, agent_args)?;

    if captured {
        if let Some(parent_agent_id) = parent_terminal_agent_id {
            return run_attached_agent_shim(kind, &addr, &parent_agent_id, command);
        }
        append_terminal_shim_log(
            "[agent-shim] captured terminal has no parent terminal id; falling back to nested launch",
        );
    }

    let mut launch_args = vec![
        "--daemon".to_string(),
        addr,
        "--kind".to_string(),
        kind.clone(),
        "--name".to_string(),
        kind.clone(),
        "--".to_string(),
    ];
    launch_args.extend(command);
    run_launch(launch_args)
}

#[cfg(not(windows))]
fn run_agent_shim(_raw: Vec<String>) -> Result<u32, String> {
    Err("agent-shim is only implemented on native Windows".to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WslShimPlan {
    command: Vec<String>,
    wrapped: bool,
}

#[cfg(test)]
fn build_wsl_shim_command(raw: &[String], addr: &str, parent_agent_id: &str) -> WslShimPlan {
    let Some(interactive_args) = interactive_wsl_args(raw) else {
        let mut command = vec!["wsl.exe".to_string()];
        command.extend(raw.iter().cloned());
        return WslShimPlan {
            command,
            wrapped: false,
        };
    };

    let mut command = vec!["wsl.exe".to_string()];
    command.extend(interactive_args);
    command.extend([
        "--exec".to_string(),
        "bash".to_string(),
        "-lc".to_string(),
        wsl_terminal_entry_script(addr, parent_agent_id),
        "vibe-bridge-terminal".to_string(),
    ]);
    WslShimPlan {
        command,
        wrapped: true,
    }
}

fn build_wsl_passthrough_command(raw: &[String]) -> WslShimPlan {
    let mut command = vec!["wsl.exe".to_string()];
    command.extend(raw.iter().cloned());
    WslShimPlan {
        command,
        wrapped: false,
    }
}

#[cfg(windows)]
fn run_wsl_shim(raw: Vec<String>) -> Result<u32, String> {
    let parent_agent_id = std::env::var("VIBE_BRIDGE_TERMINAL_AGENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let captured = std::env::var_os("VIBE_BRIDGE_CAPTURED_TERMINAL").is_some();
    append_terminal_shim_log(format!(
        "[wsl-shim] start pid={} captured={} parent={} args={}",
        std::process::id(),
        captured,
        terminal_shim_log_value(parent_agent_id.as_deref().unwrap_or("")),
        terminal_shim_log_args(&raw)
    ));

    append_terminal_shim_log(
        "[wsl-shim] native passthrough; WSL shell integration owns codex/claude capture",
    );
    let plan = build_wsl_passthrough_command(&raw);

    append_terminal_shim_log(format!(
        "[wsl-shim] {} command={}",
        if plan.wrapped {
            "interactive transient shims enabled"
        } else {
            "passthrough"
        },
        terminal_shim_log_command(&plan.command)
    ));

    let (argv0, argv_rest) = plan
        .command
        .split_first()
        .ok_or_else(|| "empty wsl command".to_string())?;
    let status = std::process::Command::new(argv0)
        .args(argv_rest)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|err| format!("spawn wsl.exe: {err}"))?;
    Ok(status.code().unwrap_or(1) as u32)
}

#[cfg(not(windows))]
fn run_wsl_shim(_raw: Vec<String>) -> Result<u32, String> {
    Err("wsl-shim is only implemented on native Windows".to_string())
}

#[cfg(windows)]
fn run_attached_agent_shim(
    kind: &str,
    addr: &str,
    parent_agent_id: &str,
    command: Vec<String>,
) -> Result<u32, String> {
    use std::io::{BufRead, BufReader};
    use std::net::TcpStream;
    use std::sync::{Arc, Mutex};

    let pid = std::process::id();
    let agent_id = format!("launch-{pid}");
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|path| path.to_str().map(String::from))
        .unwrap_or_default();
    append_terminal_shim_log(format!(
        "[agent-shim] attached pid={} kind={} agent_id={} parent={} command0={}",
        pid,
        terminal_shim_log_value(kind),
        terminal_shim_log_value(&agent_id),
        terminal_shim_log_value(parent_agent_id),
        terminal_shim_log_value(command.first().map(String::as_str).unwrap_or(""))
    ));

    let write_half =
        TcpStream::connect(addr).map_err(|err| format!("connect daemon at {addr}: {err}"))?;
    write_half
        .set_nodelay(true)
        .map_err(|err| format!("set_nodelay: {err}"))?;
    let read_half = write_half
        .try_clone()
        .map_err(|err| format!("clone tcp stream: {err}"))?;
    let writer = Arc::new(Mutex::new(write_half));
    std::thread::spawn(move || {
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    });

    let register_json = serde_json::json!({
        "type": "agent.register",
        "agent": {
            "agentId": &agent_id,
            "kind": kind,
            "name": kind,
            "cwd": &cwd,
            "fromLaunch": true,
            "parentKind": "terminal",
            "parentAgentId": parent_agent_id,
        }
    });
    send_json_line(&writer, &register_json.to_string())?;

    let (argv0, argv_rest) = command
        .split_first()
        .ok_or_else(|| format!("no resolved command for {kind}"))?;
    let status = std::process::Command::new(argv0)
        .args(argv_rest)
        .env("VIBE_BRIDGE_DAEMON", addr)
        .env("VIBE_BRIDGE_LAUNCH_AGENT_ID", &agent_id)
        .status()
        .map_err(|err| format!("spawn attached {kind}: {err}"))?;

    let abort_json = serde_json::json!({
        "type": "session.abort",
        "abort": { "agentId": &agent_id, "kind": kind },
    });
    let _ = send_json_line(&writer, &abort_json.to_string());

    let code = status.code().unwrap_or(1) as u32;
    append_terminal_shim_log(format!(
        "[agent-shim] attached exit pid={} kind={} code={}",
        pid,
        terminal_shim_log_value(kind),
        code
    ));
    Ok(code)
}

#[cfg(windows)]
fn ensure_daemon_listening(addr: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| format!("current_exe: {err}"))?;
    ensure_daemon_listening_with_exe(addr, &exe, "auto")
}

#[cfg(windows)]
fn ensure_daemon_listening_with_exe(
    addr: &str,
    exe: &std::path::Path,
    device: &str,
) -> Result<(), String> {
    use std::net::TcpStream;
    use std::os::windows::process::CommandExt;
    use std::time::{Duration, Instant};

    if TcpStream::connect(addr).is_ok() {
        return Ok(());
    }

    let log_dir = local_appdata_dir()?.join("vibe-bridge");
    std::fs::create_dir_all(&log_dir)
        .map_err(|err| format!("create log dir {}: {err}", log_dir.display()))?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("vb-daemon.log"))
        .map_err(|err| format!("open daemon log: {err}"))?;
    let log_err = log
        .try_clone()
        .map_err(|err| format!("clone daemon log: {err}"))?;

    let child = std::process::Command::new(exe)
        .args(["serve-hid", addr, device])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(log_err))
        .creation_flags(0x0800_0000 | 0x0000_0200)
        .spawn()
        .map_err(|err| format!("spawn background serve-hid: {err}"))?;
    write_daemon_status(addr, device, exe, child.id())?;

    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if TcpStream::connect(addr).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "daemon did not start listening at {addr}; see %LOCALAPPDATA%\\vibe-bridge\\vb-daemon.log"
            ));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

#[cfg(windows)]
fn resolve_agent_command(kind: &str, args: &[String]) -> Result<Vec<String>, String> {
    if let Some(real) = find_real_windows_agent(kind) {
        let ext = real
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_default();
        let mut command = if ext == "cmd" || ext == "bat" {
            vec![
                "cmd.exe".to_string(),
                "/C".to_string(),
                real.to_string_lossy().to_string(),
            ]
        } else {
            vec![real.to_string_lossy().to_string()]
        };
        command.extend(args.iter().cloned());
        return Ok(command);
    }

    let mut command = vec!["wsl.exe".to_string()];
    if let Ok(distro) = std::env::var("VIBE_BRIDGE_WSL_DISTRO") {
        if !distro.trim().is_empty() {
            command.push("-d".to_string());
            command.push(distro);
        }
    }
    command.extend(["--cd".to_string(), "~".to_string()]);
    command.extend([
        "--exec".to_string(),
        "bash".to_string(),
        "-lc".to_string(),
        wsl_agent_exec_script(),
        kind.to_string(),
    ]);
    command.extend(args.iter().cloned());
    Ok(command)
}

#[cfg(any(windows, test))]
fn wsl_agent_exec_script() -> String {
    r#"cmd="$0"
is_vibe_wrapper() {
  [ -f "$1" ] && grep -q 'vibe-bridge WSL agent wrapper' "$1" 2>/dev/null
}
try_exec() {
  candidate="$1"
  shift
  if [ -x "$candidate" ]; then
    if is_vibe_wrapper "$candidate"; then
      return 1
    fi
    exec "$candidate" "$@"
  fi
  return 1
}
try_exec "$HOME/.local/share/vibe-bridge/real-bin/$cmd" "$@"
try_exec "$HOME/.local/bin/$cmd" "$@"
try_exec "/usr/local/bin/$cmd" "$@"
try_exec "/usr/bin/$cmd" "$@"
found="$(command -v "$cmd" 2>/dev/null || true)"
if [ -n "$found" ]; then
  try_exec "$found" "$@"
fi
echo "vibe-bridge: real $cmd not found in WSL. Install native Windows $cmd, or install $cmd inside WSL without the old vibe-bridge wrapper." >&2
exit 127
"#
    .to_string()
}

#[cfg(windows)]
fn find_real_windows_agent(kind: &str) -> Option<std::path::PathBuf> {
    let shim_dir = std::env::var_os("VIBE_BRIDGE_SHIM_DIR").map(std::path::PathBuf::from);
    let path = std::env::var_os("PATH")?;
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    let mut names = vec![kind.to_string()];
    for ext in pathext.split(';') {
        let ext = ext.trim();
        if !ext.is_empty() {
            names.push(format!("{kind}{ext}"));
            names.push(format!("{kind}{}", ext.to_ascii_lowercase()));
        }
    }

    for dir in std::env::split_paths(&path) {
        if let Some(shim_dir) = &shim_dir {
            if same_path(&dir, shim_dir) {
                continue;
            }
        }
        for name in &names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    let a = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let b = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn base64_round_trip() {
        let input = b"wsl.exe -d Ubuntu-22.04 -- bash";
        let encoded = base64_encode_bytes(input);
        assert_eq!(base64_decode_bytes(&encoded).unwrap(), input);
    }

    #[test]
    fn setup_exe_name_triggers_product_install() {
        assert_eq!(
            product_action_from_exe_name("VibeBridgeSetup"),
            Some(ProductAction::Install)
        );
        assert_eq!(
            product_action_from_exe_name("vibe-bridge-installer"),
            Some(ProductAction::Install)
        );
        assert_eq!(
            product_action_from_exe_name("VibeBridgeUninstall"),
            Some(ProductAction::Uninstall)
        );
        assert_eq!(product_action_from_exe_name("vb-daemon"), None);
    }

    #[test]
    fn product_install_keeps_native_terminal_profiles_by_default() {
        assert_eq!(
            product_install_args(Vec::new()),
            vec![
                "--no-terminal-profiles".to_string(),
                "--management-shortcuts".to_string(),
                "--wsl-shell".to_string()
            ]
        );
        assert_eq!(
            product_install_args(argv(&["--no-terminal-profiles"])),
            argv(&[
                "--no-terminal-profiles",
                "--management-shortcuts",
                "--wsl-shell"
            ])
        );
        assert_eq!(
            product_install_args(argv(&["--addr", "127.0.0.1:9999"])),
            argv(&[
                "--addr",
                "127.0.0.1:9999",
                "--no-terminal-profiles",
                "--management-shortcuts",
                "--wsl-shell"
            ])
        );
        assert_eq!(
            product_install_args(argv(&["--no-management-shortcuts"])),
            argv(&[
                "--no-management-shortcuts",
                "--no-terminal-profiles",
                "--wsl-shell"
            ])
        );
        assert_eq!(
            product_install_args(argv(&["--no-wsl-shell"])),
            argv(&[
                "--no-wsl-shell",
                "--no-terminal-profiles",
                "--management-shortcuts"
            ])
        );
    }

    #[test]
    fn strip_jsonc_comments_preserves_urls_inside_strings() {
        let input = r#"{
          // comment
          "profiles": {
            "list": [
              {"name": "PowerShell", "commandline": "https://example.test//x"}
            ]
          }
        }"#;
        let stripped = strip_jsonc_comments(input);
        assert!(stripped.contains("https://example.test//x"));
        assert!(!stripped.contains("comment"));
        serde_json::from_str::<serde_json::Value>(&stripped).unwrap();
    }

    #[test]
    fn windows_terminal_profile_wraps_explicit_commandline() {
        let input = r#"{
          "profiles": {
            "list": [
              {"name": "Ubuntu-22.04", "commandline": "wsl.exe -d Ubuntu-22.04"},
              {"name": "Hidden", "hidden": true, "commandline": "cmd.exe"}
            ]
          }
        }"#;
        let exe =
            std::path::PathBuf::from(r"C:\Users\me\AppData\Local\vibe-bridge\bin\vb-daemon.exe");
        let result = wrap_windows_terminal_settings_json(input, &exe, "127.0.0.1:8765").unwrap();
        let output = result.output.unwrap();
        assert_eq!(result.changed_profiles, vec!["Ubuntu-22.04"]);
        assert_eq!(result.supported_profiles, vec!["Ubuntu-22.04"]);
        assert!(output.contains("terminal-shim"));
        assert!(output.contains("--follow-console-size"));
        assert!(output.contains("--cmdline-b64"));
        assert!(output.contains("Hidden"));
        assert!(output.contains("cmd.exe"));
    }

    #[test]
    fn windows_terminal_profile_infers_dynamic_powershell() {
        let input = r#"{
          "profiles": {
            "list": [
              {"name": "PowerShell", "source": "Windows.Terminal.PowershellCore"}
            ]
          }
        }"#;
        let exe = std::path::PathBuf::from(r"C:\vb\vb-daemon.exe");
        let result = wrap_windows_terminal_settings_json(input, &exe, "127.0.0.1:8765").unwrap();
        let output = result.output.unwrap();
        assert_eq!(result.changed_profiles, vec!["PowerShell"]);
        assert_eq!(result.supported_profiles, vec!["PowerShell"]);
        assert!(output.contains("terminal-shim"));
        assert!(output.contains("--follow-console-size"));
    }

    #[test]
    fn windows_terminal_profile_infers_wsl_profile_from_source() {
        let input = r#"{
          "profiles": {
            "list": [
              {"name": "kali-linux", "source": "Microsoft.WSL"}
            ]
          }
        }"#;
        let exe = std::path::PathBuf::from(r"C:\vb\vb-daemon.exe");
        let result = wrap_windows_terminal_settings_json(input, &exe, "127.0.0.1:8765").unwrap();
        let output = result.output.unwrap();
        assert_eq!(result.changed_profiles, vec!["kali-linux"]);
        assert_eq!(result.supported_profiles, vec!["kali-linux"]);
        assert!(output.contains("terminal-shim"));
    }

    #[test]
    fn wsl_terminal_profile_gets_transient_agent_shims() {
        let command = vec![
            "cmd.exe".to_string(),
            "/D".to_string(),
            "/S".to_string(),
            "/C".to_string(),
            "wsl.exe -d Ubuntu-22.04".to_string(),
        ];
        let wrapped = maybe_wrap_wsl_terminal_command(&command, "127.0.0.1:8765", "launch-42");
        assert_eq!(wrapped[0], "wsl.exe");
        assert!(wrapped
            .windows(2)
            .any(|pair| pair == ["-d", "Ubuntu-22.04"]));
        assert!(wrapped.windows(2).any(|pair| pair == ["--exec", "bash"]));
        let script = wrapped
            .iter()
            .find(|arg| arg.contains("vibe-bridge WSL transient agent shim"))
            .unwrap();
        assert!(script.contains("VIBE_BRIDGE_TERMINAL_AGENT_ID"));
        assert!(script.contains("make_shim codex"));
        assert!(script.contains("make_shim claude"));
        assert!(script.contains("/tmp"));
        assert!(script.contains("$HOME/.bashrc"));
        assert!(script.contains("$HOME/.local/share/vibe-bridge/shell-integration/bin/$cmd"));
        assert!(script.contains("exec \"$shell_integration\" \"$@\""));
        assert!(script.contains("export PATH=\"$VIBE_BRIDGE_WSL_SHIM_DIR:$new_path\""));
        assert!(!script.contains("> \"$HOME/.bashrc\""));
        assert!(!script.contains(">> \"$HOME/.bashrc\""));
        assert!(!script.contains("$HOME/.local/bin"));
    }

    #[test]
    fn wsl_interactive_detection_is_conservative() {
        assert_eq!(interactive_wsl_args(&[]), Some(Vec::new()));
        assert_eq!(
            interactive_wsl_args(&argv(&["-d", "Ubuntu-22.04", "--cd", "~"])),
            Some(argv(&["-d", "Ubuntu-22.04", "--cd", "~"]))
        );
        assert_eq!(
            interactive_wsl_args(&argv(&["--distribution=Ubuntu", "--user=root"])),
            Some(argv(&["--distribution=Ubuntu", "--user=root"]))
        );
        assert!(interactive_wsl_args(&argv(&["-l", "-v"])).is_none());
        assert!(interactive_wsl_args(&argv(&["--exec", "bash"])).is_none());
        assert!(interactive_wsl_args(&argv(&["ls", "-la"])).is_none());
        assert!(interactive_wsl_args(&argv(&["--unknown"])).is_none());
    }

    #[test]
    fn captured_wsl_shim_wraps_only_interactive_shell_start() {
        let plan = build_wsl_shim_command(
            &argv(&["-d", "Ubuntu-22.04"]),
            "127.0.0.1:8765",
            "launch-42",
        );
        assert!(plan.wrapped);
        assert_eq!(plan.command[0], "wsl.exe");
        assert!(plan
            .command
            .windows(2)
            .any(|pair| pair == ["-d", "Ubuntu-22.04"]));
        assert!(plan
            .command
            .windows(2)
            .any(|pair| pair == ["--exec", "bash"]));
        assert!(plan
            .command
            .iter()
            .any(|arg| arg.contains("vibe-bridge WSL transient agent shim")));

        let passthrough =
            build_wsl_shim_command(&argv(&["-l", "-v"]), "127.0.0.1:8765", "launch-42");
        assert!(!passthrough.wrapped);
        assert_eq!(passthrough.command, argv(&["wsl.exe", "-l", "-v"]));
    }

    #[test]
    fn wsl_cmd_shim_uses_native_passthrough() {
        let plan = build_wsl_passthrough_command(&argv(&["-d", "Ubuntu-22.04"]));
        assert!(!plan.wrapped);
        assert_eq!(plan.command, argv(&["wsl.exe", "-d", "Ubuntu-22.04"]));

        let list = build_wsl_passthrough_command(&argv(&["-l", "-v"]));
        assert!(!list.wrapped);
        assert_eq!(list.command, argv(&["wsl.exe", "-l", "-v"]));
    }

    #[test]
    fn shortcut_summary_detects_vibe_bridge_shortcuts() {
        assert!(shortcut_summary_is_vibe_bridge(
            r"C:\Users\me\AppData\Local\vibe-bridge\bin\vb-daemon.exe
terminal-shim --daemon 127.0.0.1:8765"
        ));
        assert!(shortcut_summary_is_vibe_bridge(
            r"C:\Users\me\AppData\Local\vibe-bridge\bin\vb-daemon.exe
status-windows"
        ));
        assert!(!shortcut_summary_is_vibe_bridge(
            r"C:\Users\me\AppData\Local\Microsoft\WindowsApps\ubuntu.exe"
        ));
    }

    #[test]
    fn wsl_shell_integration_uses_pty_shims_without_replacing_cli() {
        let script = build_wsl_shell_integration_script("127.0.0.1:8765");
        assert!(script.contains("vibe-bridge WSL shell integration agent shim"));
        assert!(script.contains("script -qfec"));
        assert!(script.contains("terminal.stream"));
        assert!(script.contains("agent.register"));
        assert!(script.contains("VIBE_BRIDGE_WSL_SHELL_SHIM_DIR"));
        assert!(script.contains("$HOME/.local/share/vibe-bridge/shell-integration/bin"));
        assert!(script.contains("install_rc_block \"$HOME/.profile\""));
        assert!(script.contains("remove_rc_block \"$rc\""));
        assert!(script.contains("for _vb_dir in \\$PATH; do"));
        assert!(script.contains("read -r -t 0.03 -N 4096 chunk"));
        assert!(script.contains("[ \"$status\" -eq 142 ] && continue"));
        assert!(script.contains("if { exec 9<>\"/dev/tcp/$host/$port\"; } 2>/dev/null; then"));
        assert!(
            script.contains("export PATH=\"\\$VIBE_BRIDGE_WSL_SHELL_SHIM_DIR:\\$_vb_new_path\"")
        );
        assert!(!script.contains("cat > \"$HOME/.local/bin"));
        assert!(!script.contains("# vibe-bridge WSL agent wrapper"));

        let uninstall = build_wsl_shell_uninstall_script("127.0.0.1:8765");
        assert!(uninstall.contains("remove_rc_block \"$HOME/.bashrc\""));
        assert!(uninstall.contains("remove_rc_block \"$HOME/.profile\""));
        assert!(uninstall.contains("rm -rf \"$HOME/.local/share/vibe-bridge/shell-integration\""));
    }

    #[test]
    fn windows_terminal_profile_infers_visual_studio_powershell() {
        let input = r#"{
          "profiles": {
            "list": [
              {"name": "Developer PowerShell for VS 2022", "source": "Windows.Terminal.VisualStudio"}
            ]
          }
        }"#;
        let exe = std::path::PathBuf::from(r"C:\vb\vb-daemon.exe");
        let result = wrap_windows_terminal_settings_json(input, &exe, "127.0.0.1:8765").unwrap();
        let output = result.output.unwrap();
        assert_eq!(
            result.changed_profiles,
            vec!["Developer PowerShell for VS 2022"]
        );
        assert_eq!(
            result.supported_profiles,
            vec!["Developer PowerShell for VS 2022"]
        );
        assert!(output.contains("terminal-shim"));
        assert!(output.contains("Terminal: Developer PowerShell for VS 2022"));
    }

    #[test]
    fn windows_terminal_profile_refreshes_existing_terminal_shim() {
        let original = "wsl.exe -d Ubuntu-22.04";
        let existing = build_terminal_profile_commandline(
            &std::path::PathBuf::from(r"C:\old\vb-daemon.exe"),
            "127.0.0.1:8765",
            "Ubuntu-22.04",
            original,
        );
        let input = format!(
            r#"{{
              "profiles": {{
                "list": [
                  {{"name": "Ubuntu-22.04", "commandline": "{}"}}
                ]
              }}
            }}"#,
            existing.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let exe = std::path::PathBuf::from(r"C:\new\vb-daemon.exe");
        let result = wrap_windows_terminal_settings_json(&input, &exe, "127.0.0.1:8765").unwrap();
        let output = result.output.unwrap();
        assert_eq!(result.changed_profiles, vec!["Ubuntu-22.04"]);
        assert_eq!(result.supported_profiles, vec!["Ubuntu-22.04"]);
        assert!(output.contains("Terminal: Ubuntu-22.04"));
        assert!(output.contains(r"C:\\new\\vb-daemon.exe"));
        assert!(!output.contains(r"C:\\old\\vb-daemon.exe"));
    }

    #[test]
    fn windows_terminal_profile_unwraps_terminal_shim() {
        let original = "wsl.exe -d Ubuntu-22.04";
        let wrapped = build_terminal_profile_commandline(
            &std::path::PathBuf::from(r"C:\vb\vb-daemon-abc.exe"),
            "127.0.0.1:8765",
            "Terminal: Ubuntu-22.04",
            original,
        );
        let input = format!(
            r#"{{
              "profiles": {{
                "list": [
                  {{"name": "Ubuntu-22.04", "commandline": "{}"}}
                ]
              }}
            }}"#,
            wrapped.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let result = unwrap_windows_terminal_settings_json(&input).unwrap();
        let output = result.output.unwrap();
        assert_eq!(result.changed_profiles, vec!["Ubuntu-22.04"]);
        assert!(output.contains("wsl.exe -d Ubuntu-22.04"));
        assert!(!output.contains("terminal-shim"));
        assert!(!output.contains("vb-daemon-abc.exe"));
    }

    #[test]
    fn wsl_agent_exec_script_is_generic_and_skips_old_wrapper() {
        let script = wsl_agent_exec_script();
        assert!(script.contains("cmd=\"$0\""));
        assert!(!script.contains("cmd=\"$1\""));
        assert!(script.contains("$HOME/.local/share/vibe-bridge/real-bin/$cmd"));
        assert!(script.contains("$HOME/.local/bin/$cmd"));
        assert!(script.contains("vibe-bridge WSL agent wrapper"));
        assert!(script.contains("command -v \"$cmd\""));
        assert!(!script.contains("Administrator"));
        assert!(!script.contains("Serein_Y"));
        assert!(!script.contains("Ubuntu"));
    }

    #[test]
    fn strip_windows_verbatim_prefix_keeps_shell_compatible_path() {
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\C:\Users\me\AppData\Local\vibe-bridge\bin"),
            r"C:\Users\me\AppData\Local\vibe-bridge\bin"
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"\??\C:\Users\me\AppData\Local\vibe-bridge\bin"),
            r"C:\Users\me\AppData\Local\vibe-bridge\bin"
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"C:\Users\me\AppData\Local\vibe-bridge\bin"),
            r"C:\Users\me\AppData\Local\vibe-bridge\bin"
        );
    }
}
