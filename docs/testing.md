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
`giveother` est **interdit** et refusé par le code (AGENTS.md règle 13 :
aucun objet ni argent triché ne quitte `admin` ; `economy_guard.rs` le
refuse sur `call admin cheat` comme en setup `verify`) ;
`setvarother <tNN> <var> <v>` (+ lecture
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

## Scénarios live `verify` (W2)

Un scénario = un JSON dans `scenarios/` (`id`, `accounts` admis, `setup[]`,
`run` script/args/budget en ticks, `assert[]`, `timeout_s` mural) :
`orchestrator verify [ids|--all] [--parallel N]` (N ≤ 20, admission respectée).
Un compte distinct par scénario ; setup en série (un seul `admin` positionne),
runs en parallèle. Tableau id/compte/résultat/ticks/wall/cause, dossier
(<40 lignes) seulement en échec. Sortie 0 si tout ok, 1 sinon, 2 usage.

Setup : `tele_admin` → `teleother`/`setvarother` (via `admin`,
effet re-vérifié par `show` : `give` répond Ok même plein, compte en login
répond Ok dans le vide — invisible = setup en échec, jamais de run condamné)
→ `bank_all` (vide le pack via le devtool `bank_deposit`). Comptes en `hold`.
`cook_shrimp` cuit les crevettes crues que le compte de test a en banque
(le script les retire) : plus aucun `giveother` (règle 13).
Budget : pire cas borné par le pack (28) ; cook = trek ~1100 + 28 cuissons → budget 1700 (<2000).
Porte Port Sarim loc 1530 flaky (« I can't reach that », 2/4, W4) : cook passe
par Al Kharid (banque 3268,3167 + range 3271,3180, ~13 cases, budget 1700) ;
récolte = blé (3220,3335) + banque Varrock-est, budget 1700 ; marche = spawn
Lumbridge, budget 150.

Preuve : `verify --all` série == `--parallel 3` (t40/t41/t42) : 3 ok des deux
côtés ; échec forcé → dossier 17 lignes, cause + position.

## Échec rapide et budgets (W4)

Rejets explicites (`verify_reject.rs`, branché sur `watch_run`) : `can't reach
that`, `inventory space` (dont `too full`/`can't carry`), `UNSET_MAP_FLAG`,
`NoRoute` frais dans `last_error`. Premier rejet arme un compteur 20 ticks
game ; un bot cloué sur sa case échoue avec le rejet en cause (dossier
inchangé), un bot qui bouge réarme (bank, re-pick : récupérations normales).
Fin <20 ticks après le rejet au lieu du budget.

Budgets = 2-3× p95 W5/W2, écrêtés par le smoke cap 2000 :

| Scénario | p95 | Budget | Mur |
| --- | --- | --- | --- |
| walk_lumbridge | ~50t | 150 (3×) | 150 s |
| cook_shrimp | ~1000t | 1700 (pack 28, <2000) | 180 s |
| harvest_grain | ~1500t | 1700 (pack 28, <2000) | 180 s |

Aucun scénario >5 min (~5000 ticks) : pas de réouverture de session (désynchro
PNJ). Porte Port Sarim loc 1530 flaky évitée par les scénarios ; couverte par
le test `stuck_bot_fails_within_20_ticks` (rejet `can't reach` → fail ≤20t).
