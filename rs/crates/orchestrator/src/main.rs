#![forbid(unsafe_code)]
//! Orchestrator binary: CLI client plus daemon socket serving.
//! Daemon supervision lives in daemon.rs, the CLI in cli.rs.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::time::Duration;

use lifescape_json::{self as json, Value, object, str_value};
use lifescape_orchestrator::{paths, web};
use lifescape_orchestrator::cli::cli;
use lifescape_orchestrator::daemon::Orchestrator;

fn serve_client(orch: &Arc<Orchestrator>, stream: UnixStream) {
    lifescape_orchestrator::res_stats::record_connection();
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
    let start = std::time::Instant::now();
    let reply = if request.as_obj().is_some() && request.get("ok").is_null() {
        match request.get("type").as_str() {
            // Worker push frames (snapshots/events): no CLI round-trip.
            Some("snapshot") | Some("event") => orch.ingest_push(&request),
            _ => dispatch_open(orch, &request),
        }
    } else {
        // A parse error already carries ok:false; never dispatch it.
        request
    };
    if start.elapsed() > std::time::Duration::from_secs(2) {
        lifescape_orchestrator::logln!("daemon: slow dispatch {:?}s", start.elapsed());
    }
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
    // Precomputed spectator map (SKIP when the pack hash is unchanged):
    // failures never block the daemon, the client shows fallback text.
    match lifescape_orchestrator::mapbuild::ensure_static_map(&lifescape_orchestrator::paths::static_dir()) {
        Ok(report) => println!("{report}"),
        Err(error) => eprintln!("static map unavailable: {error}"),
    }
    web::start_web(dispatch, port);
    // Pathfinding service: one snapshot for every worker. Any load
    // error leaves it down (every plan then fails `nav-down`, never a
    // local plan); the daemon itself never depends on it.
    let _plan_service = match lifescape_orchestrator::plan_service::load_snapshot() {
        Ok(snapshot) => match lifescape_orchestrator::plan_service::start(snapshot) {
            Ok(service) => {
                println!("pathfinding service on {}", lifescape_orchestrator::plan_service::socket_path().display());
                Some(service)
            }
            Err(error) => {
                eprintln!("pathfinding service unavailable ({error}); workers fail nav-down");
                None
            }
        },
        Err(error) => {
            eprintln!("pathfinding service unavailable ({error}); workers fail nav-down");
            None
        }
    };
    lifescape_orchestrator::holdings::start_sampler(orch.clone());
    // Market: catalogue from the dumps, store opened here alone, demo
    // history seeded when empty, socket beside the session sockets.
    match lifescape_orchestrator::market_service::start() {
        Ok(service) => {
            let probe = lifescape_json::object(vec![("op", lifescape_json::str_value("stats"))]);
            println!(
                "market on {} ({} items)",
                lifescape_orchestrator::market_service::socket_path().display(),
                service.handle(&probe).get("items").as_i64().unwrap_or(-1)
            )
        }
        Err(error) => eprintln!("market service unavailable ({error}); market page shows the error"),
    };
    // Handover convocations: sweeps, staggered poses, one delivery per bot.
    // After the market registry exists (the loop exits when it does not).
    orch.start_market_convene();
    // E2 persistent pushes: one stream per worker, one reader thread
    // daemon-side (`push.sock`). The CLI socket below keeps its 1
    // req/1 reply shape on a bounded pool (no thread per message).
    {
        let inner = orch.clone();
        let dispatch: Arc<dyn Fn(Value) + Send + Sync> =
            Arc::new(move |frame: Value| {
                let _ = inner.ingest_push(&frame);
            });
        if let Err(error) = lifescape_orchestrator::push_hub::start_push_hub(dispatch) {
            eprintln!("push hub unavailable ({error}); workers fall back to one-shot pushes");
        }
    }
    // Worker re-adoption runs beside the services, never before them: the
    // restored workers push their state and plan routes at once, so the
    // push hub, the pathfinding service and the market must already
    // listen, and the CLI socket must answer while it runs. Serial
    // adoption first (2026-11-13: ~3–4 min with nothing listening) froze
    // every worker's pump on blocked pushes, cost server disconnects,
    // `nav-down` and `market unreachable` on each restart.
    {
        let restoring = orch.clone();
        std::thread::Builder::new()
            .name("restore-jobs".to_string())
            .spawn(move || {
                let started = std::time::Instant::now();
                restoring.restore_jobs();
                println!("restore_jobs done in {:.1}s", started.elapsed().as_secs_f64());
            })
            .expect("restore thread spawns");
    }
    println!("orchestrator listening on {}", paths::socket_path().display());
    {
        let serve: Arc<dyn Fn(UnixStream) + Send + Sync> = {
            let inner = orch.clone();
            Arc::new(move |stream: UnixStream| serve_client(&inner, stream))
        };
        let keep_running: Arc<dyn Fn() -> bool + Send + Sync> = {
            let inner = orch.clone();
            Arc::new(move || inner.running.load(std::sync::atomic::Ordering::SeqCst))
        };
        lifescape_orchestrator::daemon_pool::serve_pooled(server, serve, keep_running);
    }
    let _ = std::fs::remove_file(paths::socket_path());
    let _ = std::fs::remove_file(lifescape_orchestrator::plan_service::socket_path());
    let _ = std::fs::remove_file(lifescape_orchestrator::market_service::socket_path());
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
