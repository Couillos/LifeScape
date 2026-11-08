# Pathfinding économe CPU — étude (doc seule, aucun code)

Objectif : réduire le CPU du planificateur `lifescape-nav` sans toucher au protocole,
aux workers live ni aux garanties (plan pur, déterministe, `plan.rs:242`).

## 1. Constat chiffré

### 1.1 Où part le CPU : un `plan()` = jusqu'à 4 Dijkstra + 4 `prepare_edges`

`plan()` (`rs/crates/nav/src/plan.rs:255-364`) enchaîne jusqu'à 4 recherches Dijkstra
complètes : A (requirements levés), B (péages levés, si `!allow_paid`), C (bornes F2P
levées, si `f2p_only`), D (filtre complet, pour le plus proche atteignable). Chaque
recherche est bornée par `MAX_VISITS = 500_000` (`plan.rs:57`) et appelle
`prepare_edges()` (`plan.rs:773`, appelé depuis `dijkstra()` `plan.rs:544`).

| Poste | Chiffre | Source |
|---|---|---|
| Visites max par `plan()` | 4 × 500 k = 2 M | `plan.rs:57,255-364` |
| Coût par visite | ~2 µs | rapport parent (mesures `route --release`) |
| NoRoute exhaustif (pire cas) | ~3,5 s | rapport parent ; corroboré : `chateau_scell` = 3669 ms release (`docs/decisions.md:13`) |
| Plans réussis (budget golden) | < 300 ms, max observé 140 ms | `rs/crates/nav/tests/golden.rs:173-175`, `docs/decisions.md:13` |
| `prepare_edges()` | ~90 ms à froid, **rejoué à chaque Dijkstra** (jusqu'à 4×/plan) | `plan.rs:518-549` ; itère 367 transitions (`gamedata/data/transitions.json`) avec test `reach` par stand |
| Sans yield, 238 k visites → Disconnected (1 cas de test) | prouve l'épuisement avant `MAX_VISITS` | `rs/crates/sdk/src/client/travel.rs:879-888` |
| Monde | N ≈ 1,3 M tuiles marchables après fix ciel | rapport parent ; pack = 1932 carrés (`maps-server.zip`) |
| Durées live | **non loggées** (seul `PLAN_DEBUG=1` affiche visites/ms sur stderr) | `plan.rs:547-548,600-604,711-718` |

Mémoire transitoire par recherche : `dist`/`prev` (`HashMap<u64,…>`, jusqu'à 500 k
entrées) + `BinaryHeap` — dizaines de Mo alloués puis jetés, 4× par `plan()` raté.

### 1.2 Redondance intra-worker : chaque plan est recalculé de zéro

- Aucun cache, aucune mémoïsation dans `lifescape-nav` (grep `cache|memo|LRU` : rien).
- `travel_to` planifie, puis **replanifie** une fois à l'échec (`rs/crates/sdk/src/client/travel.rs:180-252`,
  replan `travel.rs:237`), et `debug_walk` refait un plan diagnostique (pas un replan : même
  moteur, autres fins, `rs/crates/sdk/src/client/movement.rs:236`). Chaque appel = `plan()` complet.
- `Search` (`plan.rs:403-413`) reconstruit ses 3 `HashMap` (`base_flags`, `open_flags`,
  `doors`) à chaque Dijkstra ; `world.get()` prend le verrou **écriture** du monde
  à chaque tuile (`rs/crates/nav/src/static_world.rs:73-75`).

### 1.3 Redondance inter-workers : N process refont le même Dijkstra

- 1 process OS par session (`rs/crates/orchestrator/src/daemon.rs:2`, spawn via
  `current_exe` `daemon.rs:220-256`). `Arc<StaticWorld>` n'est partagé **qu'entre
  threads d'un même process** (`static_world.rs:7,27-30`) : chaque session recharge
  monde + tables (`rs/crates/orchestrator/src/session.rs:694-747`) → monde dupliqué en RAM.
- Mesuré (lecture seule, `ps`, 2026-11-07) : daemon 3190943 RSS 127 Mo (VmHWM 127 Mo),
  session `t01` 3222657 RSS **185 Mo**, session `t05` 3318133 RSS **55 Mo**
  (peinture paresseuse des carrés : le monde grossit avec les déplacements).
  Relevé 2026-11-08 (lecture seule) : daemon RSS 158 Mo (VmHWM 360 Mo, debug,
  sans monde), `admin` VmHWM 96 Mo, `t01` VmHWM 128 Mo — mondes partiels,
  peinture non finie : la borne haute est au §5.9 (300 Mo full paint), pas ici.
- `plan()` est pure et déterministe (`plan.rs:242` : « mêmes entrées, même plan, octet
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
`excluded_edges` — `plan.rs:169-176`) ; le taux avec clé complète est ≤ 63,5 %.
À noter : les tables sont chargées **une fois par session** (`session.rs:717-747`),
donc hors `excluded_edges` (déjà dans la clé), un plan en cache reste valide toute
la session — invalidation quasi nulle.
En multi-bots, `satisfied` varie **par bot** (pièces/inventaire/quête lus sur l'état
live, `travel.rs:139-163`) et `allow_paid` aussi (`mod.rs:94`, défaut `false`
`mod.rs:144`) : deux bots au même `(from, dest)` ne partagent un hit que s'ils
partagent aussi ces options — voir §5.8 pour le découpage partageable vs par-bot.

## 2. Options comparées

Contexte : peu de requêtes/s (~0,3 plan/s/worker au pire), monde statique + portes
dynamiques (paramètre, pas topologie), 1 agent par requête.

| Option | Gain CPU estimé | RAM | Complexité | Risques / invalidation |
|---|---|---|---|---|
| **A. Cache LRU local au worker (TRANSITOIRE, plus la cible)** | ~50 % intra-worker (§1.4), 0 inter-bots | < 5 Mo/worker mais N× en RAM | < 100 lignes, 1 fichier | Remplacé par le cache global §5.3 ; ne survit qu'en lot transitoire revertable (§5.5). Clé complète exigée dans tous les cas (§5.8) |
| **B. `prepare_edges()` 1× par `plan()`** (au lieu de 4×) | ~3 × 90 ms économisés à froid par plan multi-recherche | 0 | ~20 lignes | Les rides dépendent de `ignore_requires/allow_paid/ignore_f2p` : 2 variantes max (A/D), pas 4 |
| **C. Bidirectionnel (déjà envisagé)** | ~2× en espace ouvert, **~1× dans donjons/goulots** (les deux fronts doivent passer les mêmes portes) | 2× structures de visite | Moyenne : règle d'arrêt `topf+topr ≥ µ` subtile avec rides asymétriques | Portes orientées (`side_ok`, `plan.rs:640`) et rides non réversibles : front arrière faux ; cf. vPathfinder « Bottlenecks » |
| **D. Graphe grossier portails** (transitions + portes = arêtes, Dijkstra local seul) | ~10× sur longues distances (HPA* : jusqu'à 10×, chemins à 1 % de l'optimum — Botea et al. 2004) | Graphe abstrait (~367 rides + portes, ko) | Élevée : extraction, précalcul intra-cluster, lissage | Portes dynamiques = repondération d'arêtes (OK) ; donjons +6400 = le cas favorable (connectivité contrainte) |
| **E. Flow fields** | Négatif pour 1 agent | Champ = N tuiles (≈ To à l'échelle monde) | Moyenne | **À écarter** : 1 champ par destination (SupCom2, Godot docs) ; rentable à N agents / 1 but (RTS), pas 1 agent / N buts |
| **F. Service central de plans (RETENU — décision produit, §5)** | CPU : ~50-60 % de Dijkstra évités via cache GLOBAL (hubs partagés, §5.8) ; RAM : N mondes → 1 monde | 1 monde chaud (borne §5.9, limite dure 1 Go : total nominal ~790 Mo, marge ~230 Mo) + cache global 1024 plans (≤ ~10 Mo mesurés, §5.3) | Moyen : threads std + IPC JSON maison (pattern `send_worker` existant, `daemon.rs:576-597`) ; std only, pas de tokio/serde | IPC estimé < 2 ms vs Dijkstra médian ~70 ms (< 3 %, §5.1) ; SPOF → fallback sync loggé (§5.2) ; invalidation unique mais globale (§5.6) |
| **G. Précalcul total** | 100 % | **To de RAM** | Faible | **À écarter** : ~1 an de calcul (rapport parent), monde > RAM |
| **H. A* + heuristique euclidienne** | Variable | 0 | Faible | **À écarter** : offsets de donjons + portails cassent l'admissibilité de Chebyshev (déjà tranché, `plan.rs:3-8`) |

Sources externes : bidirectionnel 2 disques vs 1 (vPathfinder Algorithm Lab, liquiduslabs.com ;
thèse Kmaxii : ~50 % sur grille 1000×1000) ; HPA* 10×/1 % (Botea–Müller–Schaeffer 2004 ;
DHPA* +1,9× moy., SHPA* statique seule — AIIDE 2010) ; flow fields = 1 but partagé
(GameAIPro ch. 23, SupCom2 ; redblobgames 2024) ; cache LRU chemins 10-100× sur requêtes
répétées (defold-graph-pathfinder ; nExBot : LRU 8 entrées + négatif 32 entrées ;
Screeps Overmind : −20-30 % CPU).

## 3. Recommandation unique : service central, en 3 lots

**Décision produit (non négociée dans ce doc) : UN service central de
pathfinding, mémoire partagée UNE fois pour tous les bots.** Le reste de ce
§3 et le §5 en sont la déclinaison.

1. **Lot 1 — L2 seul (sync, ~20 lignes, aucun protocole).**
   `prepare_edges()` calculé 1× par `plan()` (variantes A/D partagées,
   `plan.rs:544,773`) au lieu de jusqu'à 4×. Gain immédiat sur chaque plan
   multi-recherche, indépendant de l'archi, revert = 1 fonction.
2. **Lot 2 — service central minimal (monde 1×, IPC, fallback).**
   Le daemon charge `StaticWorld` + `Tables` UNE fois ; threads std ;
   workers en IPC JSON maison (pattern `send_worker`, `daemon.rs:576-597`) ;
   service down → fallback sync local loggé, jamais de panne franche.
   Preuve du transport : p99 overhead IPC ≤ 5 ms mesuré (seuil proposé),
   à comparer au Dijkstra médian ~70 ms (§2 ligne F, repris).
3. **Lot 3 — cache global + fairness + métriques.** Clé complète (§5.3),
   file globale bornée + équité inter-bots (§5.2), compteurs hit/miss.
   Seuil repris : hit rate global ≥ 30 % (cf. §3 ancien), sinon régler
   avant tout lot suivant. Graphe grossier portails (§2 ligne D) seulement
   si le cache global plafonne — pas avant le verdict du lot 3.

Gain attendu : RAM N×(55-185 Mo, §1.3) → 1 monde chaud (borne §5.9,
limite dure 1 Go) dès le lot 2 ; CPU ~50 %+ de Dijkstra évités au lot 3
(répétitions intra §1.4 + hubs partagés §5.8).

## 4. Ce qu'il NE faut PAS faire

- **Précalcul total** (toutes paires) : ~1 an de calcul, To de RAM — rapport parent.
- **Flow fields** : un champ par destination, rentable à N agents/1 but ; ici 1 agent/N buts.
- **Thread-par-worker comme cible définitive** : N files, N caches, N mondes
  dupliqués — le service central (§5) fait mieux avec moins de RAM et un seul
  endroit à invalider. Le thread local ne survit qu'en lot transitoire
  revertable (§5.5), jamais comme architecture finale.
- **Protocole binaire maison pour l'IPC** : le JSON maison (`lifescape-json`,
  seule dép. nav, `nav/Cargo.toml:10-11`) + framing newline — exactement le
  pattern `send_worker` (`daemon.rs:576-597`) — suffit à ~0,3 req/s (§2) ;
  un framing binaire, c'est un nouveau parser à maintenir pour < 1 ms gagné
  (ré-estimé, §5.1) — écarté.
- **Bidirectionnel seul** : gain plafonné ~2×, nul dans les goulots ; fronts arrière
  invalides sur rides orientés + portes `side_ok`.
- **A* euclidien** : heuristique inadmissible ici (déjà tranché `plan.rs:3-8`).
- **Cache sans `excluded_edges` dans la clé** : un replan banni resservirait l'arête
  interdite → boucle d'échec. La clé complète est une condition de sûreté, pas un détail.

---
*Sources code : `rs/crates/nav/src/plan.rs`, `static_world.rs`, `tests/golden.rs`,
`rs/crates/sdk/src/client/travel.rs:180-236`, `movement.rs:236`,
`rs/crates/orchestrator/src/daemon.rs:2,220-256`, `session.rs:694-747`,
`docs/decisions.md:13`. Mesures live : `run/logs/t01.log`, `admin.log`, `ps`
(2026-11-07, lecture seule, T01 intouchée).*

## 5. Service central de pathfinding (architecture retenue)

But : UN moteur `plan()` partagé (`plan.rs:255-257` reste l'unique moteur ;
wrapper IPC, pas fork — §5.6), mémoire chargée UNE fois, cache GLOBAL unique.
Trois gains visés, dans l'ordre : (a) RAM : N mondes dupliqués (§1.3) → 1 monde
chaud ; (b) CPU : ~50 %+ de Dijkstra évités par le cache global (répétitions
intra §1.4 + hubs partagés §5.8) ; (c) pump jamais affamé (le reap à 100 ticks
disparaît par construction : plus de Dijkstra synchrone sur le thread script).
Aucune mesure live nouvelle ci-dessous : taux et budgets CPU repris des §1-2,
bornes (1500 ms, 250 ticks + 30 s, 45 s, file globale 16 + cap 2/compte, cache
global 1024 — bornes proposées sauf mention) reprises du code cité.
Le §5.8 décline le multi-bots F2P (partageable vs par-bot, clé, dimensionnement
global, fairness) : 1 monde, 1 file, 1 cache — plus aucune notion worker.

### 5.1 Architecture : service central dans le process daemon, workers en IPC

**Décision : le moteur vit dans le process daemon (threads std, pas de tokio,
`unsafe` interdit), les workers (1 process/session, `daemon.rs:231-256`) l'appellent
en IPC.** Le daemon charge `StaticWorld` + `Tables` UNE fois au démarrage
(aujourd'hui chargés par session, `session.rs:698-747` : c'est cette duplication
que le service supprime) ; les threads du service se partagent les `Arc`
(`static_world.rs:35-51` : `open` rend déjà un `Arc`) ; chaque requête
embarque la version données (§5.3).

Transport : **socket Unix + JSON maison (`lifescape-json`) + framing newline —
exactement le pattern `send_worker` (`daemon.rs:576-597`).** Pas de binaire
maison (§4 : écarté), pas de tokio/serde (`nav/Cargo.toml:10-11`, `std only`,
`AGENTS.md §4.1`), pas de `mmap` (`unsafe` interdit, `AGENTS.md §4.2`).

Pourquoi l'IPC ne mange pas le gain (chiffres, pas d'affirmation) :
requête ≈ 200-500 octets JSON (`from`/`dest` 3 entiers + options + sets triés) ;
réponse = 1 `Plan` : pire golden 264 tuiles = **9,6 Ko JSON mesuré**
(format dossier `travel_dossier.rs:186-195`, bench jetable `/tmp` nov. 2026 :
médiane goldens 0,7 Ko, min 0,2 Ko — §5.3) ; aller-retour socket Unix locale pour < 10 Ko :
< 1 ms (ré-estimé). Face au Dijkstra médian ~70 ms (§2 ligne F, repris) et au
budget `PLAN_BUDGET` 1500 ms (`plan.rs:71`), l'overhead estimé est < 2 ms soit
< 3 % du médian — deux ordres de grandeur sous le gain visé (~50 % de Dijkstra
évités). Capacité : à 0,3 req/s/bot (§2, repris), 10 bots = 3 req/s face à
~1000/70 ≈ 14 plans/s sur UN thread plan (dérivé du médian) : marge ×4 avant
tout pool. Seuil d'acceptation au lot 2 (§5.5) : p99 overhead IPC ≤ 5 ms
**mesuré** (seuil proposé) — si dépassé, revoir le framing avant le cache.

Cycle de vie, calé sur le daemon existant :
- démarrage : chargement monde + tables UNE fois (index map rapide,
  peinture paresseuse comme en session, `session.rs:696-697`), puis accept
  des workers ; un worker qui `open` après un redémarrage daemon lit la
  version courante à sa première requête (version dans chaque réponse).
- arrêt : au `shutdown` (`daemon.rs:871-879`) le service vide la file puis
  meurt avec le daemon ; jamais de thread orphelin (même discipline que
  `close_job`, `daemon.rs:519-563`, kill + suppression du socket).

**Service down → dégradation, jamais panne franche** : tout `submit` qui
n'obtient pas de réponse (socket absent, timeout) bascule en fallback
synchrone local — `plan()` direct sur le thread script (§5.2) + une ligne de
log (`nav plan sync-fallback`, même format que `nav timing`,
`travel.rs:214-217`) + compteur dans les métriques globales (§5.3). Le
watchdog 45 s (`daemon.rs:27`, `WATCHDOG_STALE_SECS`) reste le filet existant.

### 5.2 Contrat d'appel : soumission IPC non-bloquante, poll au `pump`, fallback sync

**Décision : `travel_to` soumet au service et rend la main ; le `pump` existant
fait office de poll ; service injoignable → fallback sync local.** Forme
(pseudo-signature, < 10 lignes, framing JSON newline comme `send_worker`,
`daemon.rs:576-597`) :

```
submit(account, from, dest, options) -> Ticket   // IPC, jamais bloquant
poll(ticket) -> EnCours | Pret(Plan) | Echec(NoRoute) | Refuse  // depuis pump/plan_yield
cancel(ticket)                              // unload/stop → abandon propagé
```

- Soumission : `plan_travel` (`travel.rs:74-113`) construit `from`/`options`
  comme aujourd'hui puis `submit` au lieu d'appeler `plan_yielding`
  (`travel.rs:86`) en synchrone. **File globale bornée à 16 + cap 2 tickets
  en vol par compte** (bornes proposées) : au-delà (file pleine ou cap
  atteint) → `Refuse` immédiat → fallback sync (§ ci-dessous), jamais
  d'attente bloquante sur le thread script.
- Poll : les points de `yield` existants servent de poll — `plan_yield`
  (`travel.rs:122-134`) et `pump` (`mod.rs:292-321`) vérifient le ticket à
  chaque tick (échelle ~60 ms, consigne produit) au lieu d'appeler le keepalive
  depuis le Dijkstra. Le keepalive (`KEEPALIVE_TICKS = 50`, `KEEPALIVE_WALL = 3 s`,
  `mod.rs:41-45`) redevient un appel normal sur le thread script entre deux
  polls : plus de search synchrone de 1,5 s qui affame le socket (budget
  `PLAN_BUDGET`, `plan.rs:71` ; reap 100 ticks, `travel.rs:116-117`).
- Timeouts : reprise des bornes existantes, rien de nouveau — abandon côté
  service au `PLAN_BUDGET` (1500 ms, `plan.rs:71`) comme aujourd'hui
  (`plan.rs:586-592`) ; le `yield_` du Dijkstra branche en plus le flag
  cancel du ticket (annulation) ; abandon côté script à `UNLOAD_TICKS =
  250 ticks + garde-fou 30 s` (`session_cmds.rs:19-21`, `cmd_unload`
  `session_cmds.rs:96-103`).
- Annulation : `stop_requested` (`session.rs:147`) et la sortie du hook
  `on_pump` (`mod.rs:309-313`) envoient `cancel(ticket)` exactement comme le
  `yield_() → false` annule la recherche aujourd'hui (`dijkstra`
  `plan.rs:574-578` → `Aborted`, remonté en `travel.rs:87-89` →
  `Interrupted`). Ticket en file → retiré ; en calcul → le `yield_`
  l'interrompt au prochain point de yield.
- Priorité : replan d'exécution (`travel.rs:227-252`) devant plan initial
  **du même compte** (le replan écrase l'attente du compte, jamais le calcul
  d'autrui — §5.8, règle de fairness).
- Fallback synchrone : service injoignable, file pleine, ou thread tombé →
  `plan()` direct sur le thread script + une ligne de log (`nav plan
  sync-fallback`, même format que `nav timing`, `travel.rs:214-217`).
  Une seule tentative service par `submit` : le fallback ne resubmit jamais,
  donc pas de boucle fallback-en-panne — il calcule local, logge, compte.
  Jamais de panne franche : dégrade, logge, compte (compteur `sync-fallback`
  global, §5.3), le watchdog (`daemon.rs:143-213`, seuil 45 s
  `WATCHDOG_STALE_SECS`, `daemon.rs:27`) reste le filet existant.

### 5.3 Données partagées UNE fois + cache GLOBAL unique

**Décision : un seul `StaticWorld`, un seul `Tables`, une seule version,
un seul cache — partagés par `Arc` entre les threads du service.**

Données : `StaticWorld` et `Tables` n'ont **aucune version aujourd'hui**
(`static_world.rs:26-30`, `tables.rs:127-139`) : ajouter un `DataVersion`
calculé au chargement (emprunte fichier `gamedata/data/transitions.json`,
`nav_overrides.json`, `transitions_verified.json`, `free2play.csv`, plus le
pack `maps-server.zip` et `gamedata/data/gametypes.json` (définitions de locs
qui conditionnent portes et stands) — contenu ou mtimes, au choix de
l'implémentation), stocké à côté des `Arc` dans un `Snapshot` :
`Snapshot = (Arc<StaticWorld>, Arc<Tables>, DataVersion)`.

Qui recharge, quand, sans couper les requêtes en vol : **le daemon, sur
commande opérateur explicite** (`reload-nav`, proposée — pas de watcher
automatique : du code en plus pour un événement rare), après toute modif
gamedata. Rechargement en **double-buffering** : le nouveau `Snapshot` se
construit à côté (index map rapide, peinture paresseuse — même coût qu'un
`open` session, `session.rs:696-747`), puis swap atomique (`Arc<Snapshot>`
sous `Mutex` tenu le temps du swap seul, jamais pendant un Dijkstra). Les
requêtes en vol gardent l'ancien `Arc` et finissent undisturbed, **mais leur
réponse porte l'ancienne version : le worker la rejette et resubmit une fois
sur le nouveau `Snapshot` (puis fallback sync si le resubmit échoue — jamais
de boucle)** ; la version
bumpée rend en plus les anciennes entrées cache illisibles (pas de flush aveugle,
éviction LRU naturelle). **Un seul endroit à vider**, pas N.

Cache : deux briques, comme avant, mais GLOBALES :
- **L1 — plans complets**, clé = `(from, dest, allow_paid, f2p_only,
  satisfied-trié, excluded_edges-trié, version-données)` (même clé que la
  recommandation §3 ancien). **LRU global 1024, LRU pur — CHIFFRÉ (mesuré
  nov. 2026, bench jetable `/tmp`, format dossier
  `travel_dossier.rs:186-195`)** : petit 0,2 Ko (porte seule), médiane
  goldens 0,7 Ko, max 9,6 Ko (264 tuiles walk, ~36 o/tuile) ; clé type
  ~0,2 Ko ; overhead LRU ~350 o/entrée (clé + `HashMap` + `Arc`) soit
  ~0,35 Mo pour 1024. Pire cas : 1024 × 9,6 Ko ≈ 10 Mo, soit **~1 % de la
  limite dure 1 Go** (§5.9 ; ~100 k plans au pire cas, ~1,4 M au médian) — la borne
  1024 est gardée, pas augmentée sans reboucler le §5.9 : le hit
  plafonne par la distribution des paires (63,5 % intra §1.4, queue
  longue : 5 986 paires distinctes/16 h), pas par la RAM ; le lot 4
  tranche 1024/2048/4096 sur hit mesuré. **Négatifs inclus dans le même
  LRU** (même clé, valeur = motif `NoRoute`, ~0,1 Ko) : un `Disconnected`
  coûte jusqu'au budget 1,5 s (`chateau_scell` = 3,7 s release) — le
  négatif est le hit le plus rentable du cache. Éviction LRU pure :
  segmenté (LFU/SLRU/TTL) écarté — données quasi-statiques + bump de
  version = invalidation exacte, pas de staleness temporelle, complexité
  non justifiée à ces volumes. Cible : convertir les 63,5 % de répétitions
  (§1.4, borne haute — le taux réel avec clé complète est ≤, §1.4) + les
  hits inter-bots sur hubs (§5.8) en Dijkstra évités.
  Réutilisation exacte sans preuve (coût nul) : **troncation** — si la
  nouvelle dest est SUR le plan caché, le préfixe est un plus court
  chemin (sous-structure d'optimalité), servi sans recherche.
  **Extension (préfixe + suffixe au-delà) ÉCARTÉE** : le suffixe +10
  tuiles coûte 2,6 ms warm mesuré (rentable face à un replan 80-97 ms),
  mais la concaténation est valide sans être prouvée optimale — elle
  casserait `plan.rs:242` (octet pour octet) et la preuve d'isolation
  ci-dessous ; même rejet pour le destination-keyed join (jonction
  mid-path, preuve bilatérale). Seule forme exacte d'aide bornée : le
  coût caché comme majorant d'élagage (branch-and-bound) — non retenu,
  complexité vs gain non mesuré.
  **Budget de preuve bornée N = 2000 visites** (tout réusage partiel
  futur) : temps/visite warm **revalidé** 1,6-4,9 µs (19 v → ~45 µs ;
  530 v → 0,8 ms ; 926-1201 v → 3,7-5,8 ms en zones portes/rides ;
  55-59 kv → 123-141 ms ; centre ~2-2,5 µs, le « ~2 µs » est confirmé) ;
  setup warm incompressible 2,2-3,5 ms (froid 83-183 ms, carrés non
  peints jusqu'à ~15 ms) — **supprimé par L2 (lot 1, prérequis)** :
  2000 × 2,5 µs ≈ 5 ms ≈ ordre de l'IPC. Équivalent ≈ 30 tuiles en
  terrain ouvert (visites ≈ 2r²), jusqu'à ~10× plus en couloir unique
  (visites ≈ longueur). Règle : jonction prouvée sous 2000 visites →
  réutilise ; sinon recalcul complet, jamais d'approximation.
- **L2 — `prepare_edges` partagé** (`plan.rs:773-821`, ~90 ms à froid rejoué
  jusqu'à 4×/plan, §1.1) : calculé 1× par `(version, ignore_requires,
  allow_paid)` — 2 variantes utiles max (A/D, §2 ligne B), pas 4 — et
  réutilisé par les 4 recherches A/B/C/D (`plan.rs:280-357`). ~20 lignes,
  gain immédiat sur chaque plan multi-recherche, indépendant du service :
  faisable seul en lot 1 (§5.5).

**Preuve d'isolation (même clé = même plan, aucun plan d'un bot ne peut fuiter
vers un autre).** `plan()` est pure et déterministe (`plan.rs:242` : « mêmes
entrées, même plan, octet pour octet ») : servir un hit L1, c'est servir
exactement le plan que le calcul aurait rendu pour ces entrées. La clé
contient **tous** les champs qui varient par bot — `allow_paid`, `satisfied`
(pièces/inventaire/QP, `travel.rs:139-163`), `excluded_edges` (ban du replan,
`travel.rs:232-234`), `from`/`dest`, `f2p_only`, `version-données` — aucun
chemin de code ne les ignore ni les compacte (§4 : clé incomplète = replan
banni resservi). Donc deux bots ne partagent un hit que s'ils auraient obtenu
le même plan au bit près : la fuite est impossible **par construction**, pas
par discipline. Garde-fou : test d'isolement au lot 2 (§5.5) — deux jeux
d'options ne différant que par l'un de ces champs → 0 hit croisé — plus test
de bump au lot 3.

Métriques GLOBALES (une ligne de log par plan, même format que `nav timing`,
`travel.rs:214-217`) : `hit/miss` L1, taille, âge max, `hits` L2, profondeur
file, refus, `sync-fallback`, **RSS service + carrés peints** (§5.9).
Bornes mémoire : L1 ≤ 1024 plans (négatifs
inclus, ~10 Mo max mesurés + ~0,35 Mo d'overhead), L2 ≤ 2 variantes ;
dépassement L1/L2 = éviction LRU, jamais d'alloc non bornée. Les
`HashMap`/`BinaryHeap` par recherche (§1.1) sont transitoires mais NON
négligeables : c'est le poste dominant du budget (§5.9 : 150 Mo/thread,
2 threads), borné par `MAX_VISITS` + pool fixe — jamais « inchangés
sans borne ».
Seuil conservé : alerte si p99 reply > 16 Ko (max golden mesuré 9,6 Ko —
jamais atteint, le lot 4 le vérifie).

### 5.4 Concurrence `std only` : pool de threads + `mpsc`, verrous minimaux

**Décision : `std::thread` + `std::sync::mpsc` borné, `Mutex` à grains fins,
jamais de verrou tenu pendant un Dijkstra.** Rappel applicable
(`nav/Cargo.toml:10-11`, `session.rs:1`, `daemon.rs:1` : `forbid(unsafe_code)`
partout, pas de tokio — `AGENTS.md §4.1-4.2`). Pool proposé : 2 threads plan
+ 1 accepteur IO (dimensionnement proposé : 1 thread suffit à ~14 plans/s,
§5.1 ; le 2ᵉ absorbe les pics/NoRoute exhaustifs à 1,5 s ; le lot métriques
tranche 1/2/4).

- Qui verrouille quoi : file GLOBALE (`Mutex<VecDeque>` ou `mpsc::sync_channel`,
  producteurs = threads IO des requêtes IPC, consommateurs = threads plan) ;
  table des résultats (`Mutex<HashMap<Ticket, …>>`, écrite par le service, lue
  au poll) ; cache L1/L2 (`Mutex` dédié, tenu **uniquement** pour lookup/insert,
  jamais pendant `dijkstra`, `plan.rs:518-721`) ; `Snapshot`
  (`Mutex` tenu le temps du swap seul, §5.3). `StaticWorld::get` prend
  déjà le verrou **écriture** à chaque tuile (`static_world.rs:73-75`) : avec
  2 threads plan sur le même monde partagé, ce verrou est la seule contention
  résiduelle — lectures courtes + peinture paresseuse une fois par carré ;
  pré-peindre les carrés des hubs au démarrage (§5.8) pour sortir la peinture
  du chemin chaud. Si le lot métriques montre de la contention, sharder ou
  passer `get` en lecture : hors scope, noté en incertitude (§5.7, pied de §5).
- Ordre des verrous (documenté, total) : `file → résultats → cache →
  snapshot/monde`. Jamais l'inverse, jamais deux verrous pris sous le verrou
  monde (feuille). Pas de `RwLock` ajouté : `Mutex` suffit et ne trompe
  personne sur le partage réel.
- Pas de `unsafe`, pas de `mmap` partagé inter-process (§4 : `mmap` =
  `unsafe`, interdit) : les `Arc<StaticWorld>`/`Arc<Tables>` (pattern existant
  `static_world.rs:27-30`, `travel.rs:74-80`) sont clonés entre threads du
  **même process daemon**, jamais entre process — l'inter-process passe par
  l'IPC JSON (§5.1), jamais par de la mémoire partagée.

### 5.5 Lots ordonnés : service d'abord, cache ensuite (chacun testable + revertable seul)

Ordre imposé par les dépendances : le cache GLOBAL vit dans le service, donc
le service (§5.1-5.2) précède le cache (§5.3) ; L2 seul (sync, sans archi)
passe en premier car il dé-risque `prepare_edges` pour tous les lots suivants.
Le thread-par-worker n'est PAS un lot : au plus une ÉTAPE TRANSITOIRE
(L1+L2 sync locaux, revert = 1 fichier) si le service tarde — jamais la cible.

| Lot | Contenu | Acceptation mesurable |
|---|---|---|
| 1. L2 seul (sync) | `prepare_edges()` 1×/plan, variantes A/D partagées (`plan.rs:773-821`, appel depuis `dijkstra` `plan.rs:530-549`) | `route --release` golden : `plan` p50/p99 en baisse, zéro changement de plan (goldens `tests/golden.rs:173-175`) ; revert = 1 fonction |
| 2. Service minimal (sans cache) | monde+tables 1× au daemon, 1-2 threads std, IPC JSON newline (pattern `send_worker`, `daemon.rs:576-597`), `submit/poll/cancel`, fallback sync loggé, annulation sur `stop_requested`/`UNLOAD_TICKS` | p99 overhead IPC ≤ 5 ms mesuré (seuil proposé) sur legs golden (chrono `nav timing`, `travel.rs:214-217`) ; aucun reap à 100 ticks sur legs > 1 s ; fallback loggé+compté ; **test d'isolement** : deux jeux d'options ne différant que par `satisfied`/`allow_paid`/`excluded_edges` → 0 hit croisé (unitaire, §5.8 — la clé est posée dès ce lot même sans L1) ; revert = rebrancher `plan_travel` sync |
| 3. Cache global + version | L1 global 1024 (plans + négatifs, troncation exacte, §5.3) + L2 global (§5.3), `DataVersion` au chargement daemon, swap `reload-nav`, test de bump | hit rate global ≥ 30 % (seuil repris du §3 ancien), sinon régler avant la suite ; p99 reply ≤ 16 Ko vérifié (max golden mesuré 9,6 Ko) ; test : bump `nav_overrides.json` → 0 plan périmé servi (ni hit cache ni vol en cours, §5.3 rejet+resubmit) ; RAM service ≤ budget dur §5.9 (total ≤ 1 Go, marge incluse — pas « 1 monde + cache » mou) ; revert = version constante + L1 off |
| 4. Fairness + métriques + dimensionnement | file globale 16 + cap 2/compte + cancel propagé (§5.2, §5.8), logs `hit/miss`, taille, âge, file, refus, `sync-fallback` | décision chiffrée : garder, régler (threads 1/2/4, L1 1024/2048/4096, file 16±), ou arrêter — pas de lot 5 sans ce verdict ; sous replan-storm simulé (1 compte × N échecs) : 0 famine des autres comptes (refus+fallback du fautif seul) |
| 5. (conditionnel) Graphe grossier portails | §2 ligne D, 1 niveau | seulement si le lot 4 montre un hit L1 plafonné par la distance (pas par les options) ; sinon : ne pas faire |

Chaque lot : test d'abord (golden ou unitaire existant en garde-fou),
`main` reste vert entre lots.

### 5.6 Risques (service central)

| Risque | Gravité | Mitigation (déjà conçue ci-dessus) |
|---|---|---|
| Service down (SPOF) | Moyenne (dégradation, pas panne) | fallback sync local loggé + compté (§5.2) ; le worker ne bloque jamais (file bornée → `Refuse` → sync) ; watchdog 45 s (`daemon.rs:27`) filet existant ; lot 2 sans cache prouve le fallback avant tout cache |
| Plan périmé servi (invalidation ratée GLOBALE — pire que par worker : 1 flush raté empoisonne TOUS les bots) | Haute | clé = options complètes + `version-données` (§5.3) ; `excluded_edges` dans la clé (§4) ; **un seul** point de reload (`reload-nav` opérateur + swap atomique, §5.3) au lieu de N ; réponse à version périmée rejetée + resubmit ×1 (vol en cours, §5.3) ; version servie loggée (écart détectable) ; test de bump (§5.5 lot 3) |
| Plan d'un autre bot servi (fuite inter-bots : requires/autorisations d'autrui) | Haute (péage/quête franchi sans droit, replan banni resservi) | **clé par construction** : `allow_paid` + `satisfied` + `excluded_edges` par bot dans la clé L1 (§5.8, preuve §5.3), jamais normalisés ni ignorés ; test d'isolement (§5.5 lot 2) |
| Thundering herd au cold start (N bots × cache vide) | Moyenne (pic CPU = N Dijkstra d'un coup sur le service) | file globale 16 → `Refuse` → fallback sync étalé (§5.2) : le pic se répartit entre service et workers ; L2 partagé dès le lot 1 (1 seul `prepare_edges` par variante) ; pré-peinture des carrés hubs au démarrage (§5.4, §5.8) ; préchauffage optionnel (rejouer les top paires §1.4 au boot — proposé, non tranché) |
| Taille d'un `Plan` sérialisé sur le fil | Faible (**mesuré**, risque clos) | max golden 9,6 Ko, médiane 0,7 Ko (bench `/tmp` nov. 2026, format dossier) ; seuil d'alerte p99 > 16 Ko conservé au lot 4 ; au-delà : compacter (tuiles en delta) ou refuser le cache des monstres — non tranché, mesurer d'abord |
| File qui explose sous replan-storm (échec → replan × N bots, `travel.rs:227-252`) | Moyenne | cap 2/compte + replan prioritaire sur l'attente du compte (§5.2, §5.8) : le fautif seul est refusé/basculé sync, les autres ne voient rien ; replan reste ≤ 1 par échec (inchangé, `travel.rs:221-237`) |
| Divergence sync/async (fallback ≠ service) | Moyenne | même `plan()` underneath des deux côtés, le service n'est qu'un wrapper d'appel (§5 : pas de fork) ; goldens identiques lots 1-2 |
| Contention verrou monde (2 threads plan, `get` en écriture `static_world.rs:73-75`) | Faible (à mesurer) | ordre total file → résultats → cache → monde, monde feuille (§5.4) ; pré-peinture hubs ; lot 4 tranche (shard/`get` lecture si contention avérée — hors scope) |
| Snapshot orphelin au reload/shutdown | Faible | requêtes en vol gardent l'ancien `Arc` (double-buffering, §5.3) ; au `shutdown` la file se vide puis le service meurt avec le daemon (§5.1) |

### 5.7 Service central : OUI — enterrement de l'ancienne conclusion

L'ancienne conclusion (§5.1/§5.7 avant revirement, §2 ligne F, §4) disait :
*pas de service central — 0 gain CPU, IPC ≥ plan médian, SPOF, hit inter-workers
non mesuré, donc différé*. Elle est **explicitement abandonnée**, pour cinq
raisons — dont trois montrent que ses prémisses étaient des affirmations
sans mesure, pas des faits :

1. **Décision produit** : mémoire partagée UNE fois (fin du N×<5 Mo et
   surtout fin des N mondes dupliqués, §1.3 : 185 Mo vs 55 Mo par session).
   Même à Dijkstra constants, le service supprime de la RAM réelle — ce que
   l'ancien §2 ligne F (« 0 gain CPU ») présentait comme une absence de gain
   en ignorant la RAM.
2. **« IPC ≥ plan médian » était sans mesure.** Le pattern existe
   (`send_worker`, `daemon.rs:576-597`) et les payloads sont des Ko (§5.1 :
   overhead estimé < 3 % du médian ~70 ms, avec seuil mesurable au lot 2,
   §5.5). L'ancienne conclusion comparait un coût mesuré (Dijkstra) à un
   coût imaginé (IPC) — le lot 2 remplace l'imagination par un p99.
3. **Le SPOF a un fallback conçu** (sync local loggé, §5.2 — recyclé de
   l'ancien contrat thread-par-worker) : service down = dégradation
   mesurée, pas panne franche.
4. **Le hit inter-bots a des hubs concrets** (banques 68 booths, spots,
   gobelins — §5.8) et une isolation prouvée par la clé (§5.3) : l'ancien
   « non mesuré donc différé » devient « mesuré au lot 3, avec seuil d'arrêt
   ≥ 30 % ».
5. **`std only` est compatible** : threads std + JSON maison existant
   (`lifescape-json`, `nav/Cargo.toml:10-11`), pas de tokio/serde à
   introduire — l'ancien argument confondait « service » et « tokio ».

Ce qu'on NE fait toujours PAS :
- **GPU / parallélisation du Dijkstra lui-même** : 1 agent, ~0,3 req/s
  (§2) — le parallélisme utile est entre requêtes (pool + cache), pas
  dans la boucle (`plan.rs:569-707`).
- **Précalcul total** : ~1 an de calcul, To de RAM (§4, §2 ligne G).
- **Cache de segments ou de `dist`/`prev`** : assemblage dépendant de
  l'ordre (`plan.rs:912-972`), structures transitoires par recherche
  (§1.1) — L1 plans + L2 arêtes suffisent.
- **Extension de path en cache et destination-keyed join** : tranché §5.3 —
  concaténation valide mais optimalité non prouvée (casse `plan.rs:242`) ;
  seule la troncation (dest sur le plan) est exacte et gratuite.
- **Nouvelle primitive async maison** (future/executor) : `mpsc` borné +
  poll au `pump` existant (`mod.rs:292-321`) couvrent le besoin côté worker ;
  côté service, threads std + file bornée ; un mini-runtime, c'est du code
  à maintenir pour 0,3 req/s/bot.
- **Thread-par-worker comme cible finale** (§5.1 ancien : enterré — voir
  §4 ; transitoire revertable au plus, §5.5).

### 5.8 Multi-bots F2P : 1 monde, 1 file, 1 cache — plus aucune notion worker

Contexte : N workers aux objectifs et paths différents (travel pur, bank
trips pêche/mine/woodcutting, chasse gobelins, tutos, quêtes — cf.
`skill_common.rs:175` `nearest_bank`, `skill_woodcutting_spot.rs:16-24`
yew 175 xp vs willow 67,5, `skill_woodcutting.rs:183-199` chasse gobelins à
l'est de Lumbridge, `tutorial.rs`, `lib.rs:169`), tous F2P. Ce § ne change
ni l'architecture (§5.1 : service central) ni les lots (§5.5) : il fige le
découpage que la clé doit garantir, en GLOBAL.

**F2P partout : une seule variante monde.** `f2p_only: true` est hardcodé
(`travel.rs:165`) — la recherche C (`plan.rs:336-348`) tourne toujours, la
variante P2P n'existe pas en pratique. Conséquence : un seul graphe statique
(+ overrides + verdicts) pour tout le daemon ; **plus de variante F2P/P2P
à cacher ou invalider**. Reste variable **par bot**, donc dans la clé :
`allow_paid` individuel (`mod.rs:94`, défaut `false` `mod.rs:144`),
`satisfied` (pièces/inventaire/QP lus sur l'état live, `travel.rs:139-163` :
deux bots n'ont les mêmes tags que s'ils portent la même bourse),
`excluded_edges` (ban du replan, `travel.rs:232-234`, forcément par bot).

| Partageable entre bots (UN snapshot daemon, mêmes entrées → même sortie) | Par bot (dans la clé L1, jamais normalisé) |
|---|---|
| Graphe statique + `Tables` + verdicts (`transitions_verified`, `nav_overrides`) + `DataVersion` (§5.1/§5.3) | `from` (position), `dest` |
| L2 `prepare_edges` : 2 variantes max (A/D, §2 ligne B) — indépendant du bot | `allow_paid`, `satisfied` (trié), `excluded_edges` (trié) |
| Coûts Dijkstra pour options identiques → hits L1 inter-bots (§5.3) | `version-données` (commune en pratique, mais dans la clé : §5.3) |

**Isolation par construction** — voir la preuve au §5.3 (déterminisme
`plan.rs:242` + clé complète ⇒ aucun plan d'un bot ne peut fuiter vers un
autre). Garde-fou : test d'isolement au lot 2 (§5.5).

**Charge et localité inter-bots (estimations, à mesurer au lot 3).**
Référence intra-worker : ~1000 plans/16 h/worker, 63,5 % de répétitions
exactes `(from, dest)` (§1.4). Localité inter-bots attendue : forte sur les
hubs partagés — banques (`all_banks()`, 68 booths, `banks.rs:18-36`,
`nearest_bank` `banks.rs:39-51` : Varrock ouest `(3268,3169)` en test
`banks.rs:62-63`), spots yew/saules près des banques
(`skill_woodcutting_spot.rs:17-55`), gobelins est-Lumbridge
(`skill_woodcutting.rs:183-199`), trajets banque↔spot des boucles
pêche/mine/woodcutting (`skill_common.rs:535-688` `bank_trip`). Deux bots
au même hub avec les mêmes options partagent le hit — **mais** les
options diffèrent en pratique (`satisfied` = bourse/inventaire du bot) :
le hit inter-bots réel est donc ≤ la redondance apparente des paires
`(from, dest)`, comme en intra (§1.4 borne haute). Estimation prudente
(ré-estimée) : le cache global ajoute +10-20 points de hit au-delà de
l'intra-worker sur N bots aux hubs communs ; le lot 3 le mesure (comparer
paires `(from, dest)` vs clé complète sur logs N bots).

**Dimensionnement GLOBAL : 1 cache, 1 file, 1 monde.** RAM service =
budget dur §5.9 (total nominal ~790 Mo ≤ 1 Go, marge ~230 Mo : base
daemon 180 + monde 300 + transitoire 2×150 + L1/L2/file ~13) — contre
N×(55-185 Mo + LRU) avant. Invalidation unique : un seul
reload (`reload-nav`, §5.3), quasi nulle comme en mono-worker. Aucun réglage
par bot : le lot 4 ajuste (threads 1/2/4, L1 512/1024/2048, file 16±) sur hit
rate mesuré, pas sur présomption — **sans jamais dépasser le §5.9 : toute
hausse d'un seuil (threads, L1, file) reboucle le tableau, sinon refusée**.

**Fairness : règle explicite (plus d'isolation process).** File FIFO globale
bornée à 16 + **cap 2 tickets en vol par compte** (bornes proposées) :
quand un bot sature son cap (replan-storm : échec → replan,
`travel.rs:227-252`), seuls SES submits sont refusés → fallback sync local
loggé (§5.2) — les autres comptes ne voient ni la file ni le CPU plan du
fautif. Pas de priorité inter-comptes, pas de famine possible : le replan
d'exécution n'est prioritaire que sur l'attente **du même compte** (§5.2).
Cold start N bots × cache vide : la file absorbe 16, le reste bascule sync —
le pic s'étale au lieu de saturer (§5.6). Seule contention résiduelle :
CPU/IO machine (service + N workers), hors sujet de ce doc.

**Objectifs différents = mêmes requêtes, même contrat.** Travel pur,
bank trips, hunts, tutos : tous passent par `plan_travel`
(`travel.rs:74-113`) → même `submit`/`poll`/`cancel` (§5.2), même file
globale, même fallback sync. Aucune priorité par type de script, et c'est
suffisant à ~0,3 req/s/bot (§2).

### 5.9 Budget RAM dur : 1 Go LIMITE DURE (décision produit, non négociable)

**1 Go = plafond opposable du process daemon, service inclus, mesuré en
VmHWM/RSS.** Ni ordre de grandeur, ni objectif : tout dépassement = bug
bloquant. Jamais de dépassement « temporaire accepté », jamais d'OOM
silencieux. Les « ~200 Mo » et « ~10 Mo » des §§2-5.8 deviennent les
lignes ci-dessous ; en cas d'écart, ce § fait foi.

| Poste | Borne dure | Mesuré ou estimé | Garde-fou |
|---|---|---|---|
| Base daemon (sans monde) | 180 Mo | MESURÉ debug 2026-11-08 : daemon RSS 158 Mo, VmHWM 360 Mo (binaire debug, sans monde) ; release À MESURER (M3) | lecture RSS à chaque submit, loggé avec `nav timing` |
| Monde + tables chauds (1 snapshot, peinture complète) | 300 Mo | MESURÉ partiel : sessions 55-185 Mo (§1.3) ; 2026-11-08 : admin VmHWM 96 Mo, t01 VmHWM 128 Mo (mondes partiels, peinture paresseuse) ; ESTIMÉ full paint (1932 carrés) ≤ 250 Mo + marge → borne 300 ; À MESURER (M1) | pack fini (1932 carrés) = borne finie ; compteur carrés peints loggé ; pré-peinture hubs (§5.4/§5.8) au démarrage pour sortir la peinture du chemin chaud |
| Cache L1 (1024 plans + négatifs) | 11 Mo | MESURÉ nov. 2026 : 1024 × 9,6 Ko + ~0,35 Mo overhead → ~10 Mo | cap en len strict + LRU ; entrée > 32 Ko servie une fois, non stockée ; p99 reply > 16 Ko = alerte conservée |
| L2 (`prepare_edges`, 2 variantes) | 1 Mo | ESTIMÉ (2 × rides ~367 transitions) | 2 variantes max (A/D), jamais 4 |
| File 16 + table résultats | 1 Mo | ESTIMÉ : 16 × (~4 Ko req + ~10 Ko plan) ≈ 224 Ko → borne 1 Mo | file bornée 16 + cap 2/compte : pleine → `Refuse` immédiat, jamais bloquant (§5.2) ; req > 8 Ko → `Refuse` (borne `satisfied`/`excluded_edges`) |
| Transitoire par thread plan (`dist`+`prev`+`heap`+scratch `base/open/doors`, §1.1) | 150 Mo/thread × 2 = 300 Mo | ESTIMÉ borne haute à `MAX_VISITS` = 500 k (`plan.rs:57-58`) : `dist` ~17 Mo + `prev` ~56 Mo (valeur `PrevEdge` ~32-48 o, `plan.rs:235-240`) + `heap` ~8 Mo + scratch ~3×17 Mo ≈ 130 Mo → borne 150 ; le poste dominant ; À MESURER (M2) | pool fixe 2 threads, 1 recherche/thread ; kill au `PLAN_BUDGET` 1500 ms + cancel propagé (§5.2) ; jamais de 3ᵉ recherche concurrente |
| **Total nominal (hors reload)** | **~790 Mo** | 180 + 300 + 11 + 1 + 1 + 300 = 793 → marge **~230 Mo** sous 1 Go | |
| Reload `reload-nav` (2ᵉ snapshot transitoire) | +300 Mo, EXCLUSIF | ESTIMÉ (même borne que le snapshot) | admission gelée pendant le build (tout `submit` → `Refuse` + fallback sync), pool réduit à 1 thread, L1 évincé de moitié avant swap → pic ≈ 180 + 300 + 300 + 150 + 7 ≈ **940 Mo ≤ 1 Go** ; reload refusé si RSS > 600 Mo (réessai au creux) ; un seul reload à la fois ; requêtes en vol gardent l'ancien `Arc` (§5.3) |

Aucun seuil de lot (L1 1024, file 16, cap 2, pool 2 threads) n'est augmenté
sans reboucler ce tableau : une hausse qui fait dépasser 1 Go même en pic
reload est refusée, même avec un hit mesuré supérieur.

Enforcement — ce qui se passe QUAND on approche de la limite (pas « si ») :
- **80 % (820 Mo)** : log WARN `pathfinding ram 80%` (même format que
  `nav timing`) + L1 évincé de moitié + compteur `ram-warn`. La peinture
  paresseuse continue (bornée par le pack, ligne monde) mais chaque carré
  peint est loggé.
- **90 % (920 Mo)** : admission gelée — tout `submit` → `Refuse` + fallback
  sync local loggé/compté (§5.2) ; plans > 32 Ko servis mais non cachés ;
  compteur `ram-shed`.
- **95 % (970 Mo)** : abandon des recherches en vol les plus grosses
  (flag cancel du ticket, `dijkstra` `plan.rs:574-578` → `Aborted`) +
  fallback sync ; jamais de kill process, jamais de réponse partielle.
- **OOM = bug** : tout shed/éviction est loggé + compté (`ram-warn`,
  `ram-shed`, `sync-fallback`, §5.3) ; le watchdog 45 s (`daemon.rs:27`)
  reste le filet ; pas de panic silencieuse, pas de boucle
  resubmit (une seule tentative service par `submit`, §5.2).
- La file ne bloque jamais : pleine → `Refuse` immédiat → fallback sync
  étalé sur les workers (pic réparti, §5.6). Pas de priorité inter-comptes,
  pas de famine (§5.8 inchangé).

Incertitudes restantes — à MESURER aux lots (protocole, binaire release,
monde warm sauf mention) :
- **M1 (lot 2)** : monde full-paint — `open` release + peindre les 1932
  carrés + delta VmRSS → borne 300 confirmée ou révisée AVANT le lot 3.
- **M2 (lot 2)** : transitoire/thread — bench pire cas (golden 264 tuiles
  + `NoRoute` exhaustif au `MAX_VISITS`) avec `heap_max`/`visits` déjà
  loggés (`plan.rs:565-604`, `PLAN_DEBUG=1`) + pic allocateur par recherche
  → borne 150/thread confirmée ou pool réduit à 1.
- **M3 (lot 2)** : base daemon release sans monde (`ps`/VmHWM) → borne 180
  confirmée (attendue plus basse qu'en debug).
- **M4 (lots 2-4, repris)** : p99 IPC ≤ 5 ms (lot 2), hit rate clé complète
  + bonus inter-bots (lot 3, seuil ≥ 30 %), contention verrou `get`
  (lot 4) — seuils inchangés, §5.5.

---
*Revirement central (2026-11-07, décision produit — §§1-5.8 réécrits ; mesures
complémentaires 2026-11-08, bench `/tmp` jetable, release, monde warm) :
aucun code modifié, aucune mesure live rejouée, aucun run live — chiffres
repris des §1-2 ou explicitement marqués (MESURÉ vs ré-estimé),
chemins:ligne vérifiés à l'écriture. Ancien modèle enterré au §5.7 (raisons
du revirement). MESURÉ le 2026-11-08 : temps/visite warm 1,6-4,9 µs
(centre ~2-2,5 µs, le « ~2 µs » est confirmé) ; setup warm 2,2-3,5 ms
(froid 83-183 ms) ; Plan sérialisé max 9,6 Ko / médiane 0,7 Ko / min
0,2 Ko ; borne L1 1024 confirmée (~10 Mo max — plafond §5.9, limite dure
1 Go, pas « marge ×100 » molle) ;
N preuve bornée = 2000 visites (conditionné L2 lot 1) ; extension écartée,
troncation exacte retenue (§5.3). Incertitudes ouvertes : hit rate réel
avec clé complète (≤ 63,5 %, §1.4 borne haute — le lot 3 le mesure) ;
bonus inter-bots du cache global (+10-20 pts ré-estimés, §5.8 — le lot 3
le mesure ; seuil d'arrêt ≥ 30 % global) ; overhead IPC p99 (estimé
< 2 ms — le lot 2 le mesure, seuil ≤ 5 ms) ; contention du verrou `get`
en écriture (`static_world.rs:73-75`) à 2 threads plan (le lot 4 la
mesure). Limite dure 1 Go opposable posée au §5.9 (2026-11-08) : budget
~790 Mo nominal / pic reload ~940 Mo, enforcement 80/90/95 %, M1-M3 à
mesurer au lot 2 — ce § fait foi sur tout chiffre RAM antérieur.
L'ancien thread-par-worker subsiste uniquement comme ÉTAPE
TRANSITOIRE revertable (§5.5), jamais comme cible.*
