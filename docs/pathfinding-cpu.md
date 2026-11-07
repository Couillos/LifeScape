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
