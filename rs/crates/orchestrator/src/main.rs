#![forbid(unsafe_code)]
//! Orchestrator binary: CLI client plus daemon socket serving.
//! Daemon supervision lives in daemon.rs, the CLI in cli.rs.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use lifescape_orchestrator::{json, paths, web};
use lifescape_orchestrator::cli::cli;
use lifescape_orchestrator::daemon::Orchestrator;
use lifescape_orchestrator::json::{Value, object, str_value};

fn serve_client(orch: &Arc<Orchestrator>, stream: UnixStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let request = if line.trim().is_empty() {
        object(vec![])
    } else {
        json::parse(&line).unwrap_or_else(|error| {
            object(vec![("ok", Value::Bool(false)), ("error", str_value(&error.to_string()))])
        })
    };
    let reply = if request.as_obj().is_some() && request.get("ok").is_null() {
        dispatch_open(orch, &request)
    } else {
        // A parse error already carries ok:false; never dispatch it.
        request
    };
    let mut buf = json::stringify(&reply);
    buf.push('\n');
    let _ = (&stream).write_all(buf.as_bytes());
}

// "open" spawns supervisors, so it needs the Arc; every other command
// runs on &Orchestrator.
fn dispatch_open(orch: &Arc<Orchestrator>, request: &Value) -> Value {
    if request.get("cmd").as_str() == Some("open") {
        let account = request.get("account").as_str().unwrap_or("").to_string();
        return orch.open_job(&account, false);
    }
    orch.dispatch(request)
}

fn daemon_main() -> i32 {
    let orch = Arc::new(Orchestrator::new());
    if paths::socket_path().exists() {
        match UnixStream::connect(paths::socket_path()) {
            Ok(_) => {
                eprintln!("daemon already running, refusing to start");
                return 1;
            }
            Err(_) => {
                let _ = std::fs::remove_file(paths::socket_path());
            }
        }
    }
    let server = match UnixListener::bind(paths::socket_path()) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("bind failed: {error}");
            return 1;
        }
    };
    if std::fs::write(paths::pid_path(), std::process::id().to_string()).is_err() {
        eprintln!("pid write failed");
        return 1;
    }
    let dispatch: web::Dispatch = {
        let inner = orch.clone();
        Arc::new(move |request: Value| dispatch_open(&inner, &request))
    };
    let port: u16 = std::env::var("LIFESCAPE_WEB_PORT")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(8765);
    web::start_web(dispatch, port);
    orch.restore_jobs();
    println!("orchestrator listening on {}", paths::socket_path().display());
    for stream in server.incoming() {
        if !orch.running.load(Ordering::SeqCst) {
            break;
        }
        match stream {
            Ok(stream) => {
                let inner = orch.clone();
                std::thread::spawn(move || serve_client(&inner, stream));
            }
            Err(_) => {
                if !orch.running.load(Ordering::SeqCst) {
                    break;
                }
            }
        }
    }
    let _ = std::fs::remove_file(paths::socket_path());
    0
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "daemon" {
        return std::process::ExitCode::from(daemon_main() as u8);
    }
    if args.len() > 1 && args[1] == "session" {
        let account = args.get(2).cloned().unwrap_or_default();
        return std::process::ExitCode::from(lifescape_orchestrator::session::run_session(&account) as u8);
    }
    std::process::ExitCode::from(cli(&args) as u8)
}
