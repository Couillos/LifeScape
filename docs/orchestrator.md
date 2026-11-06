# Orchestrateur

Un seul daemon supervise tous les bots, un worker de session par compte
(`rs/crates/orchestrator/src/daemon.rs`, `supervise.rs`, `session*.rs`).
Cycle de vie : `open` (connecte, `idle`), `run` (charge, `running`),
`unload` (décharge, session gardée), `close` (logout propre). Un `run` qui
lève rend `idle` + `last_error`, jamais de mort de session. Événement non
résolu (`@@EVENT`) → unload + rescue en session (`session_rescue.rs`) puis
reprise du script interrompu.

CLI (`src/cli.rs`, `src/main.rs`, le daemon démarre seul) :
`open|run|call|shell|status|show|see|logs|unload|pause|resume|hold|close|shutdown|daemon|direct|map|dump-transitions`.
`direct` rejoue un script sans daemon (tests live rapides, sans état ni logs).
`dump-transitions` régénère `gamedata/data/transitions.json` depuis le serveur.
Registre : `src/registry.rs`, source unique `lifescape_scripts::names()`.

État et logs (`src/paths.rs`, racine détectée via le dossier `orchestration/`) :
`run/users/<compte>.json` (état fusionné), `run/users/<compte>.bot.json`
(heartbeat SDK), `run/logs/<compte>.log`, `run/orchestrator.sock`.
Dashboard `http://localhost:8765/` (`src/web.rs`, statiques dans
`orchestration/static/`, port via `LIFESCAPE_WEB_PORT`). Serveur de jeu via
`LIFESCAPE_SERVER_HOST` / `LIFESCAPE_SERVER_PORT` (défaut `127.0.0.1:43594`),
données engine via `LIFESCAPE_ENGINE_DIR` (défaut `../lostcity/engine`).

Ne jamais lancer un bot en direct hors orchestrateur (ni rescue manuel) :
sessions orphelines et états faux. Diagnostics aussi via l'orchestrateur
(`check_transitions`, `check_doors`).
