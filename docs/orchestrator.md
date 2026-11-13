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
`open|run|call|shell|status|show|see|logs|unload|pause|resume|hold|close|redeploy|shutdown|daemon|direct|map|route|replay-nav|dump-transitions|reload-nav|nav-stats`.

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
Onglet « Cerveau » de la page Lives (`/life`, `brain.js`, 2026-11-13) :
`GET /api/brain/<compte>` (`src/web_brain.rs`, lecture seule) agrège le
fichier de vie (ambition passée telle quelle si présente, but + jalon
d'origine `goal_origin`, nœuds avec deps, sous-tâches déposées sur
blocage `filed`, méthode et alternatives chiffrées, compteurs), les
souhaits (commande daemon `brain_wishes` = `brain_wishlist::wishlist`),
ordres ouverts (plafond de reprix en mémoire) et deals, le live (niveaux,
combat, pièces, porté), l'historique des buts et les bannissements.
Source en échec ⇒ ligne dans `errors`, jamais une liste vide muette.
`version` = mtime du fichier de vie + empreinte du contenu affiché (hors
comptes à rebours) : le front relit toutes les 5 s et ne redessine le
graphe SVG (colonnes ambition → jalons → but → tâches par plus long
chemin de dépendance) que si elle change. Tests :
`web_brain_tests.rs`, `node orchestration/static/brain.test.js`.
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

Rotation des journaux (2026-11-13, « rotation zippée ») : `log_rotate.rs`.
Un journal de bot (`run/logs/<compte>.log`) qui atteint 100 Mo devient
`<compte>.log.<secondes unix>` puis est compressé en arrière-plan par le
`gzip` du système (`.gz`) ; les 5 archives les plus récentes de chaque
journal sont gardées. `run/orchestrator.log` (stdout du daemon, ouvert en
ajout) est vérifié chaque minute : copie puis troncature sous le verrou
stdout (aucune ligne `logln!` perdue), même compression et même rétention.
Les anciens `*.log.1` sont compressés une fois au démarrage du daemon.
Lire une archive : `zcat run/logs/<compte>.log.<ts>.gz | less`.

Déploiement (2026-11-13) : `restore_jobs` tourne en arrière-plan, après le
hub d'envoi, les chemins, le marché et l'écoute : `status` répond en ~1 s,
la reprise des ~25 workers finit en ~30–60 s (ligne `restore_jobs done in
Ns` du journal). Seul le daemon a changé → ne pas redéployer les workers.
Code worker changé (sdk, scripts, envois) → après `restore_jobs done`,
`redeploy <compte>` par compte de la flotte (liste lue dans `status` une
fois la reprise finie, jamais `run/lives` qui garde d'anciens comptes),
un compte à la fois, jamais en parallèle. `redeploy` = `close` puis `open`
sous un seul verrou de compte (aucun `open` market-absent/brain ne s'y
glisse) ; il attend lui-même le pas d'admission (`admission:open-rate
wait`) et le serveur qui garde encore le joueur (`already logged in`,
relogin retenté 2 s jusqu'à 120 ticks, `TIMEOUT_NO_RESPONSE` = 100).
Code ≠ 0 → lire l'erreur, ne pas boucler.

Propriété des workers (2026-11-13, `worker_owner.rs`, `daemon_close.rs`) :
un seul worker vivant par compte, jamais deux. Bug corrigé : un `open`
opérateur et un `open` market-absent simultanés (ryan_park, 20:17) ont
lancé deux workers ; le second a effacé le socket du premier et écrasé
son entrée `procs` (le `Child` lâché sans être tué) : worker orphelin
connecté, `close` ne tuait que le suivi, tout `open` suivant refusé
`login refused: 5 (already logged in)`. Désormais :
- open/close/redeploy/relance watchdog d'un compte sont exclusifs
  (`LifecycleLocks`) ; le second reçoit `already open <compte>: another
  open/close is in flight` (open) ou `busy <compte>: ...` (close) ;
- la vérité sur les workers vient de `/proc` (`<exe> session <compte>`,
  cwd = racine du dépôt), suivis ou non : `open` répond `already open`
  si le worker répond au ping (adoption `restore_jobs` en cours), sinon
  refuse `live worker pid N for <compte> is not owned by this daemon
  (close <compte> reaps it)` ; `spawn` refuse aussi, jamais d'écrasement
  de `procs` ni de suppression du socket d'un worker vivant ;
- côté worker, `claim_socket` refuse de délier un socket servi ;
- `close` : demande de logout acquittée tout de suite par le worker
  (arrêt du script au prochain pump, déchargement borné, bouton logout,
  fin du processus), attente bornée 20 s, puis SIGTERM / 8 s / SIGKILL du
  worker suivi **et** de tout worker non suivi du compte ; réponse
  `graceful` (déconnexion propre ou terminé), `workers`, `untracked` ;
  `ok` seulement quand plus aucun worker du compte ne vit (`close` sur
  un compte sans job mais avec un orphelin le tue aussi) ;
- `shutdown` ferme les sessions en parallèle. Les envois des
workers sont asynchrones (`push_conn`) : un daemon absent ne gèle plus
leur pump. Build `dev` optimisé (`opt-level`), compilation sur 6 cœurs
(`.cargo/config.toml`).

Erreurs (2026-11-13) : page `/errors` (`orchestration/static/errors.html`,
`errors.js`), API `web_errors.rs` : `/api/errors/summary?hours=1..72&source=
&type=&account=&script=&zone=&signature=` (KPIs, tendance vs fenêtre
précédente, erreurs/heure, groupes par signature, taux d'échec par tâche et
des deals, options de filtres), `/api/errors/list?…&limit=` (lignes : heure
UTC, compte, tâche/nœud/méthode, type, message, signature, x/z/niveau,
zone, détail, dossier), `/api/errors/dossier/<fichier>`. Collecteur
`errors_ingest.rs` (table `error_events`, sources task/nav/deal/brain,
voir `docs/decisions.md`), lignes construites dans `errors_model.rs`,
maths dans `errors_aggregate.rs`. Le spectateur suit `/#<compte>`.
