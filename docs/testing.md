# Tests et debug live

Unitaires : `cargo test --manifest-path rs/Cargo.toml` (tout), ou `-p lifescape-sdk`
`-p lifescape-gamedata` `-p lifescape-orchestrator` `-p lifescape-scripts -p lifescape-planner`.
Chaque module gamedata rejoue 2-3 vecteurs connus ; tant qu'un contrôle échoue,
corriger le dump ou le code puis relancer.

Runs légitimes sur `t01` (jamais `bite`) ; diagnostics dev (`debug_walk`, `check_*`, `probe_*`, `give`, `goto`, cheat) sur `admin` uniquement :
`open`, `run`, `call`, `unload`, `close`, `show` ×2 (ticks qui avancent).
Une théorie protocole = un diagnostic live (`debug_walk`, `check_doors`,
`check_transitions`, `check_routes`, `call`/`shell`) avant d'entrer dans le SDK.

Lecture sans script temporaire : `show` (état + `last_error`), `see` (scène
ASCII 25×25 : `@` bot, `N` pnj, `P` joueur, `*` route, `X` cible, `o` objet,
`D` décor actionnable, `#` mur), `logs` (marqueurs `=== session open/run/... ===`).
Erreurs franches : `no session` (ouvrir d'abord), `already open`, `busy`
(script sans `pump`), `unknown method` (hors whitelist), `forbidden` (`cheat` hors `admin`).

Mesures : durées calibrées en live, borne en ticks = mesure + petite marge ;
préférer un état observable (message, inventaire, PNJ visible) à l'attente aveugle.
