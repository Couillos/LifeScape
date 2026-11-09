# Navigation — workers sans carte, service unique

Les workers de session ne chargent aucune carte : ni paquet, ni monde
statique, ni tables de planification, ni collision peinte. `run_session`
(`rs/crates/orchestrator/src/session.rs`) ne branche que le handle du
service (`bot.plan_client` sur `run/pathfinding.sock`) et épingle la
version snapshot (`bot.plan_data_version`) ; `maps`, `nav_world` et
`nav_tables` restent à `None` pendant tout le run.

Le daemon porte l'unique snapshot (monde statique + `Tables` + version,
`rs/crates/orchestrator/src/plan_service.rs`, socket
`run/pathfinding.sock`) et sert les plans sur 2 threads (`PLAN_THREADS`),
files bornées (`QUEUE_CAP` 16, `ACCOUNT_CAP` 2, `RESULTS_CAP` 256),
cache L1 (1024 entrées, 32 Ko max/entrée), garde RAM (warn/shed),
timeouts `SUBMIT_TIMEOUT` 10 s / `POLL_TIMEOUT` 5 s / garde client
`SERVICE_WAIT_MAX` 30 s (`rs/crates/nav/src/service.rs`).

Ops du service (`dispatch`) : `submit` / `poll` / `cancel` pour les plans,
lectures batch hors queue `door_defs`, `tiles_flags`, `static_locs_near`,
`in_f2p`, `toll_fence`, `wants`, `diff`, `debug`, `transitions_list`,
`probe_targets`, `doors_near`, plus `stats`. Chaque op batch est comptée
et exposée dans `nav-stats`.

Planification 100 % distante (`Bot::plan_travel`,
`rs/crates/sdk/src/client/travel.rs`) : sans monde local, le plan vient
du service ou l'appel échoue en `Error::Travel("nav-down: …")` — down,
refus, timeout, version biaisée après 1 resubmit. Jamais de plan local.
Sans tables locales, les options sont celles par défaut (`f2p_only`, sans
`satisfaits`) ; sans `plan_client`, `walk_costs` rend `None` (ordre
Manhattan conservé). Le replan après échec replanifiable repasse par le
service avec l'arête bannie. `nav-down` ne déclenche ni reconnect ni
rescue : la session reste vivante, les scripts réessaient par leurs
boucles.

Exécution sans pack : le plan porte l'ouverture des portes (`open_op`,
péage, `stand`/`cross_to`) et le verdict des rides ; le worker ne garde
qu'un registre éphémère (portes du plan + vérification live). Le
diagnostic (`suspect_tile`, refus) croise le live (`world.loc_at`) et le
batch `tiles_flags`. `debug_walk` demande au service la partie statique
(flags, plan ou diagnostic `NoRoute`) et complète en live.

Dossier d'échec sous `run/failures/` : `live_locs`, messages récents,
`trail`, plan (100 % live) + `static_live_diff` calculé service-side sur
la fenêtre live envoyée par le worker (rayon 10). `orchestrator
replay-nav` rejoue le dossier et classe A (plan faux) / B (monde ≠ carte)
/ C (exécution) (`rs/crates/orchestrator/src/replaynav.rs`).

Versions et reload : chaque réponse porte la version snapshot ;
`reload-nav` rebâtit le snapshot à côté et l'échange atomiquement
(l'in-flight finit sur l'ancien `Arc`, le bump de version rend les
anciennes réponses irrecevables, 1 resubmit max).

Outils offline (one-shots dev, jamais les workers) gardent leur
chargement local : `route` (+ `--paid`, `--ascii`), `map`,
`replay-nav`, `dump-transitions`, `mapbuild`. `direct` branche lui aussi
`plan_client` (+ version) quand le service répond, sinon il refuse le
run au démarrage au lieu de diverger. Référence moteur : goldens
`rs/crates/nav/tests/` (`routes.txt`, `golden.rs`, `paid_only`,
`one_way`, `abort`, `probe`) — le planificateur `plan` n'a pas changé.
