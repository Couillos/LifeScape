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

## État de départ reproductible (W3)

Setup par `admin` sur compte `hold` (`hold` coupe brain, market, rescue et
watchdog : le run reste en `held`, vérifier `held: true` dans `show`) :
`open admin`, `open <tNN>`, `hold <tNN>`, puis `call admin cheat "<cmd>"`.
`tele <lvl,mx,mz,lx,lz>` (staff≥2) place admin ; `teleother <tNN>` amène le
testé sur la case d'admin (aucun message de succès : vérifier par `show`) ;
`giveother <tNN> <objet> <n>` et `setvarother <tNN> <var> <v>` (+ lecture
`getvarother`) fixent inventaire et variables — noms d'objets = debug names
serveur minuscules (`coins`, `raw_shrimp`). `giveother`/`setvarother`/
`teleother` sont staff≥3 **sans gate production** (`ClientCheatHandler.ts`) ;
`~procs`/`speed` exigent staff≥4 hors prod (non testés : side effects).
Après un cheat, attendre ~8–10 s avant le `show` de contrôle (snapshot en
retard possible). Niveau remis à 0 après usage (`setvarother tNN qp 0`).

Restauration `.sav` (validée 2026-11-11, t40) : `close <tNN>` (compte fermé :
le serveur lit au login, écrit au logout), `cp
data/players/main/<tNN>.sav <ref>`, muter, `close`, `cp <ref> …/<tNN>.sav`,
`open` → position, inventaire et HP identiques (md5 égaux). Référence
pristine restaurée après usage.

Preuve 2026-11-11 : `walkto` Lumbridge (3220,3217)→(3223,3217) rejoué 5× sur
t40 `held`, setup `teleother` avant chaque run : 5 verdicts `ok code=0`,
même départ et même arrivée (0,3222,3216) au log, 0 marqueur
rescue/market/brain, 1 appel LLM par run (`run --wait`), mur < 4 s.
