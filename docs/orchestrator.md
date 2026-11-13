# Orchestrateur

Un seul daemon supervise tous les bots, un worker de session par compte
(`rs/crates/orchestrator/src/daemon.rs`, `supervise.rs`, `session*.rs`).
Cycle de vie : `open` (connecte, `idle`), `run` (charge, `running`),
`unload` (décharge, session gardée), `close` (logout propre). Un `run` qui
lève rend `idle` + `last_error`, jamais de mort de session. Événement non
résolu (`@@EVENT`) → unload + rescue en session (`session_rescue.rs`) puis
reprise du script interrompu.

Le daemon héberge aussi le service de navigation : un snapshot
(monde + tables + version) chargé au démarrage (`plan_service.rs`),
servi sur `run/pathfinding.sock`. Les workers ne portent que le handle
(`plan_client`) et la version épinglée — aucune carte en worker.

CLI (`src/cli.rs`, `src/main.rs`, le daemon démarre seul) :
`open|run|call|shell|status|show|see|logs|unload|pause|resume|hold|close|shutdown|daemon|direct|map|route|replay-nav|dump-transitions|reload-nav|nav-stats`.

`verify` (W2) : runner de scénarios déclaratifs (`scenarios/*.json`, lus avec
`lifescape-json`). Setup série via `admin` (cheats + `bank_all`, effets
vérifiés), runs parallèles sur comptes distincts (N clampé à 20 D38, tête
d'admission), verdicts `show` vs baseline (position, inventaire pack+banque,
delta, XP, message par tick, PNJ). Dossier <40 lignes en échec uniquement.
Modules : `verify_cmd.rs` (runner), `verify_scenario.rs` (format),
`verify_assert.rs` (asserts).
`direct` rejoue un script sans daemon : il branche le handle du service
de navigation comme une session et refuse le run au démarrage si le
service ne répond pas (jamais de divergence silencieuse). `route`
planifie offline (`--paid`, `--ascii`), `replay-nav` classe un dossier
d'échec (A/B/C), `dump-transitions` régénère
`gamedata/data/transitions.json` depuis le serveur.
`reload-nav` échange le snapshot du service sans redémarrer ;
`nav-stats` imprime la jauge (cache L1, queue, refus cap/full/stale,
RAM warn/shed, RSS daemon, carrés peints) et le JSON complet expose en
plus un compteur par op batch (`door_defs`, `tiles_flags`,
`static_locs_near`, `in_f2p`, `toll_fence`, `wants`, `diff`, `debug`,
`transitions_list`, `probe_targets`, `doors_near`).
Registre : `src/registry.rs`, source unique `lifescape_scripts::names()`.

État et logs (`src/paths.rs`, racine détectée via le dossier `orchestration/`) :
`run/users/<compte>.json` (état fusionné), `run/users/<compte>.bot.json`
(heartbeat SDK), `run/logs/<compte>.log`, `run/orchestrator.sock`,
`run/pathfinding.sock`. Dashboard `http://localhost:8765/` (`src/web.rs`,
statiques dans `orchestration/static/`, port via `LIFESCAPE_WEB_PORT`).
Serveur de jeu via `LIFESCAPE_SERVER_HOST` / `LIFESCAPE_SERVER_PORT`
(défaut `127.0.0.1:43594`), données engine via `LIFESCAPE_ENGINE_DIR`
(défaut `../lostcity/engine`).

Ne jamais lancer un bot en direct hors orchestrateur (ni rescue manuel) :
sessions orphelines et états faux. Diagnostics aussi via l'orchestrateur
(`check_transitions`, `check_doors`).

Redémarrage (2026-11-12) : `shutdown` ferme les sessions puis termine
vraiment le daemon (la boucle `accept` lit `running`). Un daemon tué
(SIGTERM) laisse les workers connectés ; le suivant les reprend
(`restore_jobs`, statut retenté 3 s puis 10 s) sans jamais supprimer le
socket d'un worker vivant (seul un socket sans écoute est effacé : plus
de worker orphelin). La pause `hold` est portée par le worker et relue à
la reprise. `run/orchestrator.log` (sortie du daemon) est horodaté ; les
erreurs de script y figurent avec leur type et leur message.

Déploiement (2026-11-13) : `restore_jobs` tourne en arrière-plan, après le
hub d'envoi, les chemins, le marché et l'écoute : `status` répond en ~1 s,
la reprise des ~25 workers finit en ~30–60 s (ligne `restore_jobs done in
Ns` du journal). Seul le daemon a changé → ne pas redéployer les workers.
Code worker changé (sdk, scripts, envois) → après `restore_jobs done`,
`close` puis `open` par compte de la flotte (liste lue dans `status` une
fois la reprise finie, jamais `run/lives` qui garde d'anciens comptes) ;
l'admission limite les ouvertures (`admission:open-rate wait`) : relancer
après ~0,8 s, jamais en parallèle (~40 s pour 25 comptes). Les envois des
workers sont asynchrones (`push_conn`) : un daemon absent ne gèle plus
leur pump. Build `dev` optimisé (`opt-level`), compilation sur 6 cœurs
(`.cargo/config.toml`).
