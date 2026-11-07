# Pathfinding économe CPU — étude (doc seule, aucun code)

Objectif : réduire le CPU du planificateur `lifescape-nav` sans toucher au protocole,
aux workers live ni aux garanties (plan pur, déterministe, `plan.rs:228`).

## 1. Constat chiffré

### 1.1 Où part le CPU : un `plan()` = jusqu'à 4 Dijkstra + 4 `prepare_edges`

`plan()` (`rs/crates/nav/src/plan.rs:241-349`) enchaîne jusqu'à 4 recherches Dijkstra
complètes : A (requirements levés), B (péages levés, si `!allow_paid`), C (bornes F2P
levées, si `f2p_only`), D (filtre complet, pour le plus proche atteignable). Chaque
recherche est bornée par `MAX_VISITS = 500_000` (`plan.rs:57`) et appelle
`prepare_edges()` (`plan.rs:528`, via `dijkstra()` `plan.rs:503-513`).

| Poste | Chiffre | Source |
|---|---|---|
| Visites max par `plan()` | 4 × 500 k = 2 M | `plan.rs:57,241-349` |
| Coût par visite | ~2 µs | rapport parent (mesures `route --release`) |
| NoRoute exhaustif (pire cas) | ~3,5 s | rapport parent ; corroboré : `chateau_scell` = 3669 ms release (`docs/decisions.md:13`) |
| Plans réussis (budget golden) | < 300 ms, max observé 140 ms | `rs/crates/nav/tests/golden.rs:169-174`, `docs/decisions.md:13` |
| `prepare_edges()` | ~90 ms à froid, **rejoué à chaque Dijkstra** (jusqu'à 4×/plan) | `plan.rs:503-531` ; itère 367 transitions (`gamedata/data/transitions.json`) avec test `reach` par stand |
| Sans yield, 238 k visites → Disconnected (1 cas de test) | prouve l'épuisement avant `MAX_VISITS` | `rs/crates/sdk/src/client/travel.rs:849-854` |
| Monde | N ≈ 1,3 M tuiles marchables après fix ciel | rapport parent ; pack = 1932 carrés (`maps-server.zip`) |
| Durées live | **non loggées** (seul `PLAN_DEBUG=1` affiche visites/ms sur stderr) | `plan.rs:529-531,575-580` |

Mémoire transitoire par recherche : `dist`/`prev` (`HashMap<u64,…>`, jusqu'à 500 k
entrées) + `BinaryHeap` — dizaines de Mo alloués puis jetés, 4× par `plan()` raté.

### 1.2 Redondance intra-worker : chaque plan est recalculé de zéro

- Aucun cache, aucune mémoïsation dans `lifescape-nav` (grep `cache|memo|LRU` : rien).
- `travel_to` planifie, puis **replanifie** une fois à l'échec (`rs/crates/sdk/src/client/travel.rs:180-236`,
  replan `travel.rs:212`), et `debug_walk` replanifie pour le diagnostic
  (`rs/crates/sdk/src/client/movement.rs:236`). Chaque appel = `plan()` complet.
- `Search` (`plan.rs:388-398`) reconstruit ses 3 `HashMap` (`base_flags`, `open_flags`,
  `doors`) à chaque Dijkstra ; `world.get()` prend le verrou **écriture** du monde
  à chaque tuile (`rs/crates/nav/src/static_world.rs:73-75`).

### 1.3 Redondance inter-workers : N process refont le même Dijkstra

- 1 process OS par session (`rs/crates/orchestrator/src/daemon.rs:2`, spawn via
  `current_exe` `daemon.rs:215-232`). `Arc<StaticWorld>` n'est partagé **qu'entre
  threads d'un même process** (`static_world.rs:7,27-30`) : chaque session recharge
  monde + tables (`rs/crates/orchestrator/src/session.rs:698-728`) → monde dupliqué en RAM.
- Mesuré (lecture seule, `ps`, 2026-11-07) : daemon 3190943 RSS 127 Mo (VmHWM 127 Mo),
  session `t01` 3222657 RSS **185 Mo**, session `t05` 3318133 RSS **55 Mo**
  (peinture paresseuse des carrés : le monde grossit avec les déplacements).
- `plan()` est pure et déterministe (`plan.rs:228` : « mêmes entrées, même plan, octet
  pour octet ») : deux bots au même endroit avec les mêmes options refont **le même
  Dijkstra au bit près**, chacun dans son process.

### 1.4 Live : les mêmes paires (from, dest) reviennent en boucle

`run/logs/t01.log` (~16 h, 23:31 → 15:28) et `admin.log`, lecture seule :

| Mesure | t01.log | admin.log |
|---|---|---|
| `nav plan` | 16 420 (~1 026/h) | 8 187 |
| paires (from, dest) distinctes | 5 986 | — |
| **répétitions exactes (from, dest)** | **10 434 soit 63,5 %** | — |
| top paire répétée | 395× (`3117,3503,0 → 3157,3463,0`, boucle bank) | top dest 442× (`→ 3186,3446,0`) |
| `nav fail` → `nav replan` | 694 → 693 (**~4,2 %** des plans, ~43/h) | 480 replan/fail |

Borne haute : la clé réelle inclut aussi `NavOptions` (`allow_paid`, `satisfied`,
`excluded_edges` — `plan.rs:156-163`) ; le taux avec clé complète est ≤ 63,5 %.
À noter : les tables sont chargées **une fois par session** (`session.rs:718-727`),
donc hors `excluded_edges` (déjà dans la clé), un plan en cache reste valide toute
la session — invalidation quasi nulle.

## 2. Options comparées

Contexte : peu de requêtes/s (~0,3 plan/s/worker au pire), monde statique + portes
dynamiques (paramètre, pas topologie), 1 agent par requête.

| Option | Gain CPU estimé | RAM | Complexité | Risques / invalidation |
|---|---|---|---|---|
| **A. Cache LRU par worker, clé (from, dest, options, version tables)** | ~50-60 % de Dijkstra évités (taux répétition §1.4) | ~64-256 plans (~ko chacun) : < 5 Mo | < 100 lignes, 1 fichier | Clé doit inclure `excluded_edges` (replans) ; `satisfied` varie (pièces) → mesurer hit/miss au runtime |
| **B. `prepare_edges()` 1× par `plan()`** (au lieu de 4×) | ~3 × 90 ms économisés à froid par plan multi-recherche | 0 | ~20 lignes | Les rides dépendent de `ignore_requires/allow_paid/ignore_f2p` : 2 variantes max (A/D), pas 4 |
| **C. Bidirectionnel (déjà envisagé)** | ~2× en espace ouvert, **~1× dans donjons/goulots** (les deux fronts doivent passer les mêmes portes) | 2× structures de visite | Moyenne : règle d'arrêt `topf+topr ≥ µ` subtile avec rides asymétriques | Portes orientées (`side_ok`, `plan.rs:615`) et rides non réversibles : front arrière faux ; cf. vPathfinder « Bottlenecks » |
| **D. Graphe grossier portails** (transitions + portes = arêtes, Dijkstra local seul) | ~10× sur longues distances (HPA* : jusqu'à 10×, chemins à 1 % de l'optimum — Botea et al. 2004) | Graphe abstrait (~367 rides + portes, ko) | Élevée : extraction, précalcul intra-cluster, lissage | Portes dynamiques = repondération d'arêtes (OK) ; donjons +6400 = le cas favorable (connectivité contrainte) |
| **E. Flow fields** | Négatif pour 1 agent | Champ = N tuiles (≈ To à l'échelle monde) | Moyenne | **À écarter** : 1 champ par destination (SupCom2, Godot docs) ; rentable à N agents / 1 but (RTS), pas 1 agent / N buts |
| **F. Daemon de plans (1 moteur, IPC)** | Supprime la duplication RAM, **0 gain CPU** (mêmes Dijkstra) | −N mondes | Élevée + viole `std only` (pas de tokio/serde, `AGENTS.md §4.1`) | IPC (socket/JSON maison) > coût d'un plan médian (~70 ms) ; point de défaillance unique |
| **G. Précalcul total** | 100 % | **To de RAM** | Faible | **À écarter** : ~1 an de calcul (rapport parent), monde > RAM |
| **H. A* + heuristique euclidienne** | Variable | 0 | Faible | **À écarter** : offsets de donjons + portails cassent l'admissibilité de Chebyshev (déjà tranché, `plan.rs:3-8`) |

Sources externes : bidirectionnel 2 disques vs 1 (vPathfinder Algorithm Lab, liquiduslabs.com ;
thèse Kmaxii : ~50 % sur grille 1000×1000) ; HPA* 10×/1 % (Botea–Müller–Schaeffer 2004 ;
DHPA* +1,9× moy., SHPA* statique seule — AIIDE 2010) ; flow fields = 1 but partagé
(GameAIPro ch. 23, SupCom2 ; redblobgames 2024) ; cache LRU chemins 10-100× sur requêtes
répétées (defold-graph-pathfinder ; nExBot : LRU 8 entrées + négatif 32 entrées ;
Screeps Overmind : −20-30 % CPU).

## 3. Recommandation unique, en 2 étapes

**Étape 1 — quick win (< 100 lignes, 1 crate, aucun protocole) : cache + setup unique.**

1. LRU borné (ex. 128 entrées) dans le worker, clé = `(from, dest, allow_paid,
   f2p_only, satisfied-trié, excluded_edges-trié, version-tables)` → `Plan` cloné.
   Le `Plan` est petit (segments + tuiles) et `Clone` existe déjà (`plan.rs:146`).
2. Compteurs `hit/miss` dans le log `nav plan` (une ligne) pour valider le §1.4
   avec la vraie clé — si hit < 30 %, réduire la voilure avant l'étape 2.
3. `prepare_edges()` calculé une fois par `plan()` (variantes A/D partagées,
   `plan.rs:503-528`) — ~20 lignes, gain immédiat sur chaque plan multi-recherche.

Gain attendu : ~50 % de Dijkstra évités pour < 5 Mo, zéro invalidation dynamique
(tables figées par session, `session.rs:718-727`).

**Étape 2 — fond (quand l'étape 1 est mesurée) : graphe grossier portails.**

Nœuds = stands de rides + tuiles de portes ; arêtes = coûts précalculés intra-zone ;
requête = Dijkstra abstrait (microsecondes) + Dijkstra local entre portails.
C'est le profil HPA* 1 niveau, adapté aux donjons (+6400 : connectivité contrainte =
cas favorable). Invalidation = incrément de version tables sur `nav_overrides.json`.
Ne pas faire de HPA* multi-niveaux : gains décroissants (Botea et al.), 1 niveau suffit
à ~0,3 req/s.

## 4. Ce qu'il NE faut PAS faire

- **Précalcul total** (toutes paires) : ~1 an de calcul, To de RAM — rapport parent.
- **Flow fields** : un champ par destination, rentable à N agents/1 but ; ici 1 agent/N buts.
- **Daemon/IPC ou mémoire partagée inter-process** : aucun gain CPU (mêmes Dijkstra),
  latence IPC ≥ plan médian, `mmap` = `unsafe` (interdit, `AGENTS.md §4.2`), `std only`.
- **Bidirectionnel seul** : gain plafonné ~2×, nul dans les goulots ; fronts arrière
  invalides sur rides orientés + portes `side_ok`.
- **A* euclidien** : heuristique inadmissible ici (déjà tranché `plan.rs:3-8`).
- **Cache sans `excluded_edges` dans la clé** : un replan banni resservirait l'arête
  interdite → boucle d'échec. La clé complète est une condition de sûreté, pas un détail.

---
*Sources code : `rs/crates/nav/src/plan.rs`, `static_world.rs`, `tests/golden.rs`,
`rs/crates/sdk/src/client/travel.rs:180-236`, `movement.rs:236`,
`rs/crates/orchestrator/src/daemon.rs:2,215-232`, `session.rs:698-728`,
`docs/decisions.md:13`. Mesures live : `run/logs/t01.log`, `admin.log`, `ps`
(2026-11-07, lecture seule, T01 intouchée).*

## 5. Plan async mutualisé

But : sortir `plan()` du thread script sans changer le plan lui-même
(`plan()` reste l'unique moteur, `plan.rs:255-257` ; wrapper, pas fork — §5.6).
Deux gains visés, dans l'ordre : (a) plus de pump affamé (le reap à
100 ticks disparaît par construction) ; (b) ~50 % de Dijkstra évités par le
cache local (§1.4 : 63,5 % de répétitions exactes (from, dest), borne haute).
Aucun chiffre nouveau ci-dessous : tous sont repris des §1-2.

### 5.1 Architecture : un planificateur par worker, pas de service central

**Décision : thread planificateur dédié par process session, pas de service
central inter-process.** Le worker session est déjà 1 process OS par compte
(`daemon.rs:231-256`, spawn via `current_exe` `daemon.rs:215-228`), et le
thread script y détient `bot` verrouillé pendant tout le run
(`session.rs:302-309`, `run_script_thread` `session.rs:337-338`). Le
planificateur est un second thread du même process, qui reçoit des demandes
sur un canal borné et rend des `Plan` clonés (`Plan: Clone`, `plan.rs:161`).
Alternative rejetée : service central inter-process (socket Unix + JSON
maison). Le §2 ligne F tranche déjà : 0 gain CPU (mêmes Dijkstra), latence
IPC ≥ plan médian, point de défaillance unique, et `std only` interdit
tokio/serde (`nav/Cargo.toml:10-11` : seule dépendance `lifescape-json`
maison). Un service central ne se justifierait que par un hit rate
*inter*-workers mesuré — non mesuré à ce jour (le §1.4 ne compte que
l'intra-worker `t01.log`) — donc différé jusqu'à preuve, pas écarté par
principe.

Cycle de vie, calé sur la session existante :
- démarrage après le chargement monde + tables (`session.rs:717-747`),
  avec clones des `Arc` (`nav_world`, `nav_tables`) et de la version
  données (§5.3) ;
- arrêt au `close`/`unload` (`session_cmds.rs:261-277`, `cmd_unload`
  `session_cmds.rs:78-134`) : le thread sort quand la file se vide ou à
  l'abandon explicite, jamais de thread orphelin après `close_job`
  (`daemon.rs:519-563`, kill + suppression du socket).

Propriété des données : le worker possède tout. `StaticWorld` et `Tables`
n'ont **aucune version aujourd'hui** (`static_world.rs:26-30`,
`tables.rs:127-139`) : ajouter un `DataVersion` calculé au chargement
(emprunte fichier `gamedata/data/transitions.json`,
`nav_overrides.json`, `transitions_verified.json`, `free2play.csv` —
contenu ou mtimes, au choix de l'implémentation) et stocké à côté des `Arc`.
Chaque demande de plan embarque la version ; le cache n'est jamais vidé en
aveugle (§5.3). `transitions_verified` (exclusions + pénalité
`UNVERIFIED_PENALTY`, `plan.rs:773-821`), `nav_overrides` (requirements,
`tables.rs:296-401`) et les options `allow_paid` / `f2p_only` / `satisfied`
(`NavOptions`, `plan.rs:169-176`) font partie de la clé, pas d'un flush.

### 5.2 Contrat d'appel : soumission non-bloquante, poll au `pump`

**Décision : `travel_to` soumet et rend la main ; le `pump` existant fait
office de poll.** Forme (pseudo-signature, < 10 lignes) :

```
submit(from, dest, options, version) -> Ticket   // jamais bloquant, file bornée
poll(ticket) -> EnCours | Pret(Plan) | Echec(NoRoute)  // appelé depuis pump/plan_yield
cancel(ticket)                              // unload/stop → abandon
```

- Soumission : `plan_travel` (`travel.rs:74-113`) construit `from`/`options`
  comme aujourd'hui puis `submit` au lieu d'appeler `plan_yielding`
  (`travel.rs:86`) en synchrone. File **bornée à 2** (1 en calcul + 1 en
  attente) : au-delà, refus immédiat → fallback synchrone (§ ci-dessous),
  jamais d'attente bloquante sur le thread script.
- Poll : les points de `yield` existants servent de poll —
  `plan_yield` (`travel.rs:122-134`) et `pump` (`mod.rs:292-321`) vérifient
  le ticket au lieu d'appeler le keepalive depuis le Dijkstra. Le keepalive
  (`KEEPALIVE_TICKS = 50`, `KEEPALIVE_WALL = 3 s`, `mod.rs:41-45`) redevient
  un appel normal sur le thread script entre deux polls : plus de search
  synchrone de 1,5 s qui affame le socket (budget `PLAN_BUDGET`,
  `plan.rs:71` ; reap 100 ticks, `travel.rs:116-117`).
- Timeouts : reprise des bornes existantes, rien de nouveau — abandon côté
  planificateur au `PLAN_BUDGET` (1500 ms, `plan.rs:71`) comme aujourd'hui
  (`plan.rs:586-592`), abandon côté script à `UNLOAD_TICKS = 250 ticks +
  garde-fou 30 s` (`session_cmds.rs:19-21`, `cmd_unload`
  `session_cmds.rs:96-103`).
- Annulation : `stop_requested` (`session.rs:147`) et la sortie du hook
  `on_pump` (`mod.rs:309-313`) annulent le ticket (`cancel`) exactement
  comme le `yield_() → false` annule la recherche aujourd'hui
  (`plan.rs:262-268` → `Aborted`, `travel.rs:87-89` → `Interrupted`).
- Priorité : replan d'exécution (`travel.rs:227-252`) devant plan initial
  (file de 2 : le replan écrase l'attente, jamais le calcul en cours).
- Fallback synchrone : planificateur saturé ou thread tombé → `plan()`
  direct sur le thread script + une ligne de log (`nav plan sync-fallback`,
  même format que `nav timing`, `travel.rs:214-217`). Jamais de panne
  franche : dégrade, logge, le watchdog (`daemon.rs:143-213`, 45 s sans
  heartbeat) reste le filet existant.

### 5.3 Cache : L1 requête exacte, L2 arêtes, invalidation par version

**Décision : deux briques, pas de cache de segments.** Les segments assemblés
(`assemble`, `plan.rs:912-972`) dépendent de l'ordre d'assemblage ; les
cacher, c'est dupliquer `plan()` en pire. Les deux briques utiles :

- **L1 — plans complets**, clé = `(from, dest, allow_paid, f2p_only,
  satisfied-trié, excluded_edges-trié, version-données)` (même clé que la
  recommandation §3.1). LRU ~128 entrées, < 5 Mo (§3.1) ; `Plan` petit,
  `Clone` existant (`plan.rs:161`). Cible : convertir les 63,5 % de
  répétitions (§1.4) en hits — avec la clé complète le taux réel est ≤ 63,5 %
  (§1.4, borne haute), d'où la métrique hit/miss obligatoire avant l'étape 2
  (§3.1 : si hit < 30 %, réduire la voilure).
- **L2 — `prepare_edges` partagé** (`plan.rs:773-821`, ~90 ms à froid rejoué
  jusqu'à 4×/plan, §1.1) : calculé 1× par `(version, ignore_requires,
  allow_paid)` — 2 variantes utiles max (A/D, §2 ligne B), pas 4 — et
  réutilisé par les 4 recherches A/B/C/D (`plan.rs:280-357`). ~20 lignes,
  gain immédiat sur chaque plan multi-recherche (§3.1 point 3), indépendant
  du thread async : faisable seul en lot 1.

Invalidation : aucune clé sans `version-données` ; un bump de version rend
les anciennes entrées illisibles (pas de flush aveugle, éviction LRU
naturelle). `Tables::load` (`tables.rs:145-150`, une fois par session,
`session.rs:737-746`) et `set_f2p`/`set_sky_floors`
(`static_world.rs:157-167`, `session.rs:726-743`) sont les seuls points de
bump. `satisfied` (pièces/inventaire, `travel.rs:139-166`) varie à chaque
appel : il reste dans la clé L1 (pas de normalisation magique), le taux de
hit mesuré dira s'il faut le compacter (ex.ordonner + hacher, jamais
l'ignorer — §4 : clé incomplète = replan banni resservi).

Métriques (une ligne de log par plan, même format que `nav timing`,
`travel.rs:214-217`) : `hit/miss L1`, `taille`, `âge max`, `hits L2`.
Bornes mémoire : L1 ≤ 128 plans, L2 ≤ 2 variantes ; dépassement = éviction
LRU, jamais d'alloc non bornée (les `HashMap`/`BinaryHeap` par recherche,
§1.1, restent transitoires et inchangés).

### 5.4 Concurrence `std only` : threads + `mpsc`, verrous minimaux

**Décision : `std::thread` + `std::sync::mpsc` borné, `Mutex` à grains fins,
jamais de verrou tenu pendant un Dijkstra.** Rappel applicable
(`nav/Cargo.toml:10-11`, `session.rs:1`, `daemon.rs:1` : `forbid(unsafe_code)`
partout, pas de tokio — `AGENTS.md §4.1-4.2`).

- Qui verrouille quoi : file de demandes (`Mutex<VecDeque>` ou `mpsc::sync_channel`,
  producteur = thread script, consommateur = thread plan) ; table des
  résultats (`Mutex<HashMap<Ticket, …>>`, écrit par le planificateur, lu au
  poll) ; cache L1/L2 (`Mutex` dédié, tenu **uniquement** pour lookup/insert,
  jamais pendant `dijkstra`, `plan.rs:518-721`). `StaticWorld::get` prend
  déjà le verrou **écriture** à chaque tuile (`static_world.rs:73-75`) :
  seul le thread plan y touche en masse, le thread script ne fait plus de
  `plan()` synchrone — la contention lecture/écriture actuelle
  (`Search.base/open_flags`, `plan.rs:415-493` + `world.get`) disparaît avec
  elle.
- Ordre des verrous (documenté, total) : `file → résultats → cache →
  monde`. Jamais l'inverse, jamais deux verrous pris sous le verrou monde
  (feuille). Pas de `RwLock` ajouté : un seul écrivain (planificateur) par
  structure, `Mutex` suffit et ne trompe personne sur le partage réel.
- Pas de `unsafe`, pas de `mmap` partagé inter-process (§4 : `mmap` =
  `unsafe`, interdit) : les `Arc<StaticWorld>`/`Arc<Tables>`
  (`static_world.rs:27-30`, `travel.rs:74-80`) sont clonés entre threads du
  même process, jamais entre process.

### 5.5 Lots ordonnés (chacun testable + revertable seul)

| Lot | Contenu | Acceptation mesurable |
|---|---|---|
| 1. L2 seul (sync) | `prepare_edges()` 1×/plan, variantes A/D partagées (`plan.rs:773-821`, appel depuis `dijkstra` `plan.rs:530-549`) | `route --release` golden : `plan` p50/p99 en baisse, zéro changement de plan (goldens `tests/golden.rs:169-174`) ; revert = 1 fonction |
| 2. L1 local (sync) | LRU 128 + clé complète + compteurs `hit/miss` dans `nav plan` (`travel.rs:180-191`) | hit rate live ≥ 30 % (§3.1), sinon stop ; < 5 Mo ; revert = 1 fichier |
| 3. Thread plan + contrat async | submit/poll/cancel (§5.2), fallback sync, annulation sur `stop_requested`/`UNLOAD_TICKS` | aucun reap à 100 ticks sur legs > 1 s (logs `t01`-style), p99 `travel_to` ≤ sync + 5 ms, fallback loggé ; revert = rebrancher `plan_travel` sync |
| 4. Version données | `DataVersion` au chargement (`session.rs:717-747`), dans chaque clé | test : bump `nav_overrides.json` → 0 hit périmé servi (test unitaire, pas de live) ; revert = version constante |
| 5. Métriques + dimensionnement | taille/âge/hit-miss loggés, bornes L1/L2 | décision chiffrée : garder, régler (128 → N), ou arrêter — pas de lot 6 sans ce verdict |

Chaque lot : test d'abord (golden ou unitaire existant en garde-fou),
un seul crate touché par lot (nav → sdk → orchestrator), `main` reste
vert entre lots.

### 5.6 Risques

| Risque | Gravité | Mitigation (déjà conçue ci-dessus) |
|---|---|---|
| Plan périmé servi (invalidation ratée) | Haute (bot marche au mur / péage refusé) | clé = options complètes + `version-données` (§5.3) ; `excluded_edges` dans la clé (§4) ; test de bump (§5.5 lot 4) |
| Thundering herd au cold start (N bots, cache vide) | Moyenne (pic CPU = N Dijkstra, §1.3) | file bornée → fallback sync étalé (§5.2) ; L2 partagé dès le lot 1 (1 seul `prepare_edges` par variante) |
| File qui explose sous replan-storm (échec → replan × N bots, `travel.rs:227-252`) | Moyenne | profondeur 2 + replan prioritaire écrase l'attente (§5.2) ; replan reste ≤ 1 par échec (inchangé, `travel.rs:197-206`) |
| Divergence sync/async (fallback ≠ thread) | Moyenne | même `plan()` underneath, le thread n'est qu'un wrapper d'appel (§5 : pas de fork) ; goldens identiques lots 1-3 |
| Verrou monde pendant Dijkstra (régression contention) | Faible | ordre total file → résultats → cache → monde, monde feuille (§5.4) ; `get` en écriture (`static_world.rs:73-75`) réservé au thread plan |
| Thread orphelin au close/unload | Faible | cycle de vie calé sur `cmd_unload`/`close_job` (§5.1) ; garde-fous existants `UNLOAD_TICKS` 250 + 30 s (`session_cmds.rs:19-21`) |

### 5.7 Ce qu'on NE fait PAS (async)

- **Service central inter-process d'emblée** : §5.1 — pas de hit rate
  inter-workers mesuré, IPC ≥ plan médian (§2 ligne F), SPOF. Réévaluer
  seulement si le lot 5 montre un hit L1 plafonné par le cross-compte.
- **GPU / parallélisation du Dijkstra lui-même** : 1 agent, ~0,3 req/s
  (§2) — le parallélisme utile est entre requêtes (cache + thread), pas
  dans la boucle (`plan.rs:569-707`).
- **Précalcul total** : ~1 an de calcul, To de RAM (§4, §2 ligne G).
- **Cache de segments ou de `dist`/`prev`** : assemblage dépendant de
  l'ordre (`plan.rs:912-972`), structures transitoires par recherche
  (§1.1) — L1 plans + L2 arêtes suffisent.
- **Nouvelle primitive async maison** (future/executor) : `mpsc` borné +
  poll au `pump` existant (`mod.rs:292-321`) couvrent le besoin ; un
  mini-runtime, c'est du code à maintenir pour 0,3 req/s.

---
*Ajout §5 (2026-11-07) : aucun code modifié, aucune mesure live rejouée
(T01 intouchée) — chiffres repris des §1-2, chemins:ligne vérifiés à
l'écriture. Incertitudes ouvertes : hit rate réel avec clé complète
(≤ 63,5 %, §1.4 borne haute — le lot 2 le mesure) ; hit rate inter-workers
(inconnu — conditionne tout service central futur) ; taille réelle d'un
`Plan` cloné en RAM (borne < 5 Mo du §3.1 à confirmer au lot 2).*
