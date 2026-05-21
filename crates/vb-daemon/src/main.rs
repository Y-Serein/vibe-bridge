use std::env;

use vb_daemon::{run_tcp_registration_server, BridgeDaemon};

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
        Some("help") | Some("--help") | Some("-h") | None => print_help(),
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
}

fn compact(value: &str) -> String {
    value.replace('\r', " ").replace('\n', " ")
}
