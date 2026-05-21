use std::env;
use std::path::Path;
use std::time::Duration;

use vb_core::{AgentActivity, AgentSession, TerminalWindow};
use vb_host::{
    active_agent_processes, agent_source_roots, discover_agent_session_candidates,
    discover_agent_sessions, discover_terminal_titles, discover_terminal_windows, focus_window,
    AgentSourcePoller,
};

fn main() {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("discover") => match discover_terminal_windows() {
            Ok(windows) => {
                print_windows(&windows);
            }
            Err(err) => {
                eprintln!("discover failed: {err}");
                std::process::exit(1);
            }
        },
        Some("tabs") => match discover_terminal_titles() {
            Ok(titles) => {
                println!("terminal titles: {}", titles.len());
                for title in titles {
                    println!("title={}", compact(&title));
                }
            }
            Err(err) => {
                eprintln!("tabs failed: {err}");
                std::process::exit(1);
            }
        },
        Some("agents") => {
            let include_inactive = args.any(|arg| arg == "--all");
            let result = if include_inactive {
                discover_agent_session_candidates()
            } else {
                discover_agent_sessions()
            };
            match result {
                Ok(sessions) => {
                    print_agents(&sessions);
                }
                Err(err) => {
                    eprintln!("agents failed: {err}");
                    std::process::exit(1);
                }
            }
        }
        Some("sources") => {
            print_sources();
        }
        Some("processes") => {
            print_processes();
        }
        Some("monitor") => {
            let monitor_args = args.collect::<Vec<_>>();
            let once = monitor_args.iter().any(|arg| arg == "--once");
            let include_inactive = monitor_args.iter().any(|arg| arg == "--all");
            let mut poller = AgentSourcePoller::new_with_inactive(include_inactive);
            loop {
                match poller.poll_once() {
                    Ok(snapshot) => {
                        print_agents(&snapshot.sessions);
                        print_activities(&snapshot.activities);
                    }
                    Err(err) => {
                        eprintln!("monitor failed: {err}");
                        std::process::exit(1);
                    }
                }
                if once {
                    break;
                }
                std::thread::sleep(Duration::from_millis(700));
            }
        }
        Some("focus") => {
            let Some(raw_hwnd) = args.next() else {
                eprintln!("focus requires an HWND, e.g. 0x123456");
                std::process::exit(2);
            };
            let hwnd = match parse_hwnd(&raw_hwnd) {
                Some(hwnd) => hwnd,
                None => {
                    eprintln!("invalid HWND: {raw_hwnd}");
                    std::process::exit(2);
                }
            };
            if let Err(err) = focus_window(hwnd) {
                eprintln!("focus failed: {err}");
                std::process::exit(1);
            }
        }
        Some("help") | Some("--help") | Some("-h") | None => {
            print_help();
        }
        Some(other) => {
            eprintln!("unknown command: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!("vb-host");
    println!();
    println!("Commands:");
    println!("  discover    enumerate visible Windows terminal windows");
    println!("  tabs        enumerate Windows Terminal tab titles through UI Automation");
    println!("  agents      enumerate active AI agent sessions discovered from hooks/transcripts");
    println!("  agents --all enumerate passive transcript candidates, including inactive history");
    println!("  sources     print agent transcript source roots and existence checks");
    println!("  processes   print running agent processes used for active session matching");
    println!("  monitor     poll active transcript sources and print sanitized activity events");
    println!("  focus HWND  bring a discovered terminal window to the foreground");
}

fn print_windows(windows: &[TerminalWindow]) {
    println!("terminal windows: {}", windows.len());
    for w in windows {
        println!(
            "hwnd=0x{:x} pid={} kind={} title={} exe={}",
            w.hwnd,
            w.pid,
            w.kind.as_str(),
            compact(&w.title),
            compact(w.process_basename())
        );
    }
}

fn compact(value: &str) -> String {
    value.replace('\r', " ").replace('\n', " ")
}

fn print_agents(sessions: &[AgentSession]) {
    println!("agent sessions: {}", sessions.len());
    for session in sessions {
        let hwnd = session
            .terminal_hwnd
            .map(|hwnd| format!("0x{hwnd:x}"))
            .unwrap_or_else(|| "unbound".to_string());
        println!(
            "kind={} status={} id={} name={} hwnd={} cwd={} transcript={}",
            session.kind.as_str(),
            session.status.as_str(),
            compact(&session.agent_id),
            compact(&session.name),
            hwnd,
            compact(&session.cwd),
            compact(&session.transcript_path)
        );
    }
}

fn print_activities(activities: &[AgentActivity]) {
    println!("agent activities: {}", activities.len());
    for activity in activities {
        println!(
            "kind={} activity={} status={} id={} transcript={}",
            activity.kind.as_str(),
            activity.activity.as_str(),
            activity.status.as_str(),
            compact(&activity.agent_id),
            compact(&activity.transcript_path)
        );
    }
}

fn print_sources() {
    let roots = agent_source_roots();
    println!("agent homes: {}", roots.homes.len());
    for home in roots.homes {
        print_path_check("home", &home);
    }
    println!("claude roots: {}", roots.claude_roots.len());
    for root in roots.claude_roots {
        print_path_check("claude", &root);
    }
    println!("codex roots: {}", roots.codex_roots.len());
    for root in roots.codex_roots {
        print_path_check("codex", &root);
    }
}

fn print_processes() {
    let processes = active_agent_processes();
    println!("agent processes: {}", processes.len());
    for process in processes {
        println!(
            "kind={} cwd={}",
            process.kind.as_str(),
            compact(&process.cwd)
        );
    }
}

fn print_path_check(label: &str, path: &Path) {
    let exists = if path.exists() { "exists" } else { "missing" };
    println!("{label} [{exists}] {}", compact(&path.to_string_lossy()));
}

fn parse_hwnd(raw: &str) -> Option<usize> {
    let trimmed = raw.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        usize::from_str_radix(hex, 16).ok()
    } else {
        trimmed.parse::<usize>().ok()
    }
}
