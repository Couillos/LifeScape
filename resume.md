# LifeScape — reprise de session (F2P quests)

## Objectif
Finir les 18 quêtes F2P avec un script fiable par quête. **15/18 terminées.**

## Terminées (save + daemon OK)
cook, sheep, doric, priest, gobdip, imp, hetty, romeojuliet, runemysteries,
haunted, vampire, hunt (pirate=4), prince (princequest=110),
knights (squire=7), demon (demonstart=30).

## En cours
- **black_knights_fortress** : save `spy=2` (varp 130, grill écoutée le
  03/11). Reste : porte 3 → mur secret → trou → Amik. Mécanique comprise
  (code serveur + cache, sans test live) :
  - `witchgrill` (3025,3507,0, décor mural shape 5) ne répond que depuis sa
    propre tuile (ReachStrategy : shapes 4/5 → `src == dest`) ; tuile
    marchable, d'où l'écoute réussie posée dessus.
  - `bkfortressdoor3` (3025,3511,1, mur face EST) : l'ouest n'est jamais
    valide (reachWall1 EST = tuile est, ou nord/sud) — l'ancien solver
    cliquait depuis l'ouest et prenait « I can't reach that! » en boucle.
    Fix : `stand=(3025,3512)` (nord, `x == porte.x` → pas d'aggro).
  - `bksecretdoor` (3030,3510,1, face SUD, op Push pas Open → invisible au
    planificateur de portes) : le nord est invalide, mais la tuile du mur
    est marchable et ouverte côté nord → marcher dessus puis Push depuis
    la tuile même (téléport au sud, salle du trou).
  - `blackknighthole` (3031,3507,1) : 3 stands valides autour
    (3030,3507 / 3031,3506 / 3031,3508) ; chou normal (1965), pas magique.
  - Kit admin OK (déguisement porté, 1 chou normal, spy=2 en save) ; banque
    dev sans chou/fer/bronze → prévoir le kit des t01…t20 avant leurs runs.
  - Tests live ciblés (03/11, sans conso) : porte 3 depuis stand nord OK
    (téléport est, pas d'aggro) ; mur secret Push depuis sa tuile OK
    (téléport sud + message, retour OK) ; les 3 stands du trou marchables
    OK ; **grill KO** : tuile own-only (ReachStrategy shape 5) dans un
    îlot sans aucun pas d'entrée (BFS flags + routeur + `step_to` live
    concordants ; sortie brute S→W le long de z=3510 prouvée live).
    → le grill est impossible en marchant sur ce serveur : décision
    serveur/moteur requise avant les runs frais (admin déjà spy=2, non
    bloqué). `step_to` N (3025,3508)→grill accepté côté serveur.
  - Prochain run : `run admin quest_black_knights_fortress` (solver prêt :
    stands porte 3/mur secret, route cuisine via ouest porte 2, entrée
    par position).
- **Reste après** : shield_of_arrav (échange à 2 comptes, voir note),
  dragon_slayer en dernier (32 QP requis).

## Random events pendant une quête (corrigé)
`common.run` appelle `randomevents.event_at` : si un maze/mime/cube est en
cours, il rapporte l'événement et s'arrête, le daemon lance le rescue puis
reprend la quête. Avant, un event en plein run téléportait le bot et toutes
les étapes suivantes échouaient (c'est arrivé deux fois sur BKF).
`rescue_cube` monte à 200 cubes : le serveur en ajoute un toutes les 150 ticks,
la borne de 40 laissait toujours un cube derrière.

## Maze : état au 03/11 (solver en cours, non fini)
**L'événement maze est une création LOCALE**, absent de l'upstream
(`LostCityRS/Content@274` n'a ni `[timer,macro_maze]` avec sortie, ni
`settimer` dans `start_macro_maze`, ni le test de position). Ne pas le
comparer à l'original pour juger du comportement.

Modifs serveur locales (`content/.../macro_event_maze.rs2`), non versionnées :
1. `settimer(macro_maze, 5)` armé aussi dans `start_macro_maze` (le bloc
   `[mapzone]` ne s'exécute pas quand l'event téléporte).
2. `[timer,macro_maze]` termine l'event : position dans le rect centre
   (coordx 2909..2913, coordz 4573..4577) **ou** `%xplamp <= 0`.
3. `end_macro_maze` : `macro_return_teleport` d'abord, puis jingle/cheer.
4. **Nouveau** : `%macro_event = ^macro_maze` posé dans `start_macro_maze`,
   sinon le `::maze` de test laissait d'autres events spawner pendant le
   labyrinthe (vieux/génie ignorés → fail-teleport au milieu du solve).

Mesures live (20 ms/tick) : ~100 → 0 % en ~1900 ticks (~38 s) ; le timer
ne décrémente que quand `canAccess()` (pas de modal). Une traversée de
porte coûte 2–4 ticks quand elle marche, 50–75 quand elle rate.

### Solver (`scripts/randomevents.py`, `solve_maze`)
Acquis, testés live :
- route = **Dijkstra coûtée** (`DOOR_COST = 80` vs 1/tuile) et plus BFS :
  minimiser les portes, pas les tuiles (15 portes sur la route optimale).
- couloirs marchés par **paquet explicite** (`bot.walk_steps`, ajouté à
  `lifescape/client/movement.py`) : `walk_to` échoue car le routeur refuse
  les tuiles walllow.
- les arêtes refusées sont mémorisées pour **toutes** les arêtes (avant,
  seules les portes l'étaient → boucle infinie sur un pas simple).
- traversée : `bot.open_door` du SDK (il se place du bon côté, clique,
  step-through) ; ne jamais re-cliquer une porte sur laquelle on se tient.
- sur une tuile de porte refermée : `step_to` est refusé, repli sur
  `bot._raw_step_off()` (paquet de pas brut).
- refus = **message de jeu** (« I can't reach that! »), pas seulement un
  modal : les scanner tous les ticks.
- ne jamais fermer un modal pendant le solve (inutile pour le timer, et
  un modal rend les ops inertes → on ferme une fois au début, c'est tout).
- `survive()` : le **swarm** (255 PV, attaque 50) colle le joueur et ne
  despawn jamais seul → il faut le tuer, sinon le bot meurt (kit perdu une
  fois sur admin). Idem triffide enragé.
- `pick_triffid()` (dans `survive()`) : une graine `macro_triffidseed`
  à ≤6 tuiles est cliquée (op 1) — mûre : fruit + event clos ; pas mûre :
  simple message serveur, sans risque. Sinon elle devient hostile à tuer.
  Validé live (spawn forcé `~macro_event 5` sur admin : fruit cueilli).
- `FAIL_TELEPORTS` (24 coords, décodées de
  `macro_events_teleports.enum`) : `check()` ne traite plus *tout* étage ≠ 0
  comme une tour d'event (faux positifs à répétition).
- `rescue_maze` ne renvoie plus 0 à l'aveugle : `_verify()` rapporte
  l'event réellement en cours (avant, un cube/mime était « résolu » à tort).
- `rescue_mime` : le mime joue un emote (`%npc_int`), le client le lit via
  le mask anim NPC (0x2 → `world.npcs[nid]["anim"]`, seq ids dans
  `content/pack/seq.pack`) et clique le bouton `macro_mime_emotes:com_2+i`
  (comps 6546+i) quand l'interface s'ouvre, 4/4 → retour. Validé live
  (admin : mime → Varrock). Registre `RESCUES` : `mime` → `rescue_mime`.
  Piège : `com_2` = 6546, pas 6545 (com_0 = 6544).

### Blocage restant (juste après la dernière modif, NON testée)
Oscillation `(2910,4598) ↔ (2910,4599)` : `open_door` renvoie True mais
laisse le bot du même côté (porte qui n'ouvre que dans l'autre sens), donc
la boucle rebouclait. Correctif écrit : après une arête-porte, si la
position n'a pas changé (`moved == gone`), on bloque l'arête dirigée au
lieu de continuer. **À rejouer** :

```
run admin probe_maze_solve     # sonde: mesure ticks + % restant
```
attendu : plus d'oscillation, arrivée dans le rect centre → `end_macro_maze`
→ retour au point de départ avec `solve=True` sous ~1900 ticks.

### Outils de test ajoutés
- `scripts/dev/probe_maze_solve.py` (+ registry `probe_maze_solve`) : résout le
  cube en attente, lance `~maze`, résout, imprime
  `maze solve=<bool> ticks=<n>` et `maze: <t> ticks in, <x>% left`.
- `scripts/dev/probe_maze_door.py` (+ `probe_maze_door`) : téléporte autour
  d'une porte et dump pas brut / clic / pas de sortie pour chaque côté.
- `probe_use` affiche maintenant `id shape angle` des locs proches.

### Pièges de test (ne pas refaire)
- `::maze` sans `%macro_event` : d'autres events spawnent pendant le
  labyrinthe et un fail-teleport sort le bot du maze (c'était la cause des
  « arrachages » à 300–700 ticks, pas le timer).
- `::mazeend` téléporte au centre : utile pour sortir vite, mais ça
  n'évalue pas le solver.
- Le point de retour du maze = la tuile où l'event a démarré ; si c'est une
  tuile d'escalier, le retour déclenche la transition (le bot atterrit
  ailleurs, ex. crypte 0,3077,9771).

## Spectate orchestrateur (corrigé)
- Fuite de descripteurs : `makefile()` duplique le fd et n'était jamais
  fermé (worker + daemon + `send_worker`) → `select()` casse après 1024
  fds, `idle_pump` meurt, plus aucun write d'état → dashboard figé ~1 min.
  Fermeture explicite des trois sites ; fds stables depuis.
- Tout passe par le **websocket** (`/ws`) : jobs + état + queue de log
  (120 lignes, seulement quand le fichier change). Plus aucun poll HTTP
  dans la page spectate (les routes `/api/*` restent pour curl/debug).
- Bouton `Real-time: off|on` (défaut off = canvas à 1 Hz, persisté en
  `localStorage`) ; les panneaux restent live dans les deux cas.
- Textes du canvas rendus en **dernier passage** : les tuiles dessinées
  après ne recouvrent plus les labels.


## Régressions corrigées côté sessions (à garder en tête)
- Un run de quête force `bot.relogin()` : la save n'est écrite qu'au logout,
  donc en session persistante les gates d'étape (`max(live, save)`)
  régressaient et le solver rejouait l'entrée.
- `find_loc` lit `world.static_actions` : une loc actionnable masquée par un
  décor sur la même tuile (porte intérieure BKF, statue du maze) est trouvée.

## Tâche future : audit protocole SDK (prd/audit-protocole.md)
Audit complet client↔serveur fait le 03/11 (deux inventaires croisés) : ce
qui manque côté SDK = varps non vidés sur relogin, spells ciblées manquantes
(OPHELDT 135, OPLOCT 213, OPPLAYERU 36), réponses friend list non parsées,
état d'onglet/hidden perdu, opcode inconnu en réception tue la session.
Détails, priorités, ce qui est clos sans action : lire
`prd/audit-protocole.md` (au-dessus de la partie « Tâches »).

## Commandes (que via l'orchestrateur, jamais de pypy direct)
```
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py open <compte>
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py run <compte> <script> [args]
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py call <compte> <methode> [args]   # debug SDK direct
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py shell <compte>                   # REPL
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py status | show | see | logs <compte> 40
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py unload <compte> | close <compte>
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py test   # unitaires
```
Plus de `start`/`stop`/`restart`. Pour vérifier un comportement, `call` (lecture
depuis le snapshot, écriture au prochain `pump`) ou `shell` remplacent les
sondes jetables.
Jobs dev : `goto`, `give`, `probe_give`, `probe_door`, `probe_talk`,
`probe_use` (clic loc + use objet + dump modals/unsets/msgs),
`probe_vars`, `drop`, `bank_deposit`, `quest_reset`. `see <compte>` :
tout ce que le bot voit (positions + distances, inventaire, modal).
`show <compte>` expose aussi `messages` (messages serveur) et `texts`
(interface) : c'est le moyen de debugger sans sonde.
Comptes test t01…t20,
`admin`/`admin` = compte de dev (jamais de cheats dans les solvers).

## Règles intangibles
- **Téléport (`goto`) = positionnement de test uniquement.** Vérifié : aucun
  solver n'importe de cheat (`grep cheat|teleport scripts/quests` → rien sauf
  un commentaire "no teleport" dans solver.py).
- Ne jamais modifier le serveur (`../lostcity/engine`, `content`) : tout se
  corrige côté client/SDK.
- Tickrate dev = 20 ms (`tickRate: 20`) → respawns 30x plus rapides qu'en prod (Keli = 10 s).
- Varps live ne pré-chargent pas : gates d'étape = `max(live, save)` +
  `save` rechargé au début du solve (`_SAVED_VARPS`), + poll `_await_stage`
  après chaque dialogue (le varp live arrive après la fermeture du modal).
- Keepalive = `NO_TIMEOUT`, jamais `IDLE_TIMER` (demande de logout).
- Objets consommés mid-quête (rhum, bières, corde, clé, kit prince) :
  hors `quest_requirements.py`, checks de kit dans le solve par étape.
- Debug verbeux : `touch run/debug` (ou `LIFESCAPE_DEBUG=1`) → lignes
  `DBG <pos> <event>` (walks, clics, rides, op sends).
- Commit après chaque quête/étape (`git add -A && git commit`), `run/` ignoré.

## Recettes SDK (lifescape/client/movement.py)
- Côtés de transition : joignable d'abord, puis sans mur, puis distance ;
  la tuile courante passe première (find_path compresse le trajet nul).
- `approach()` : tentative exacte plafonnée à 15 s puis candidats radius.
- `_send_walk` : refuse les routes périmées après téléport (deltas i8).
- Clics de porte : id live résolu, un seul côté, vérifié (pas de toggle) ;
  impasses temporaires FIFO (8, 15 s), portes bloquées 120 s.
- Interactions rejetées silencieusement : compter les `UNSET_MAP_FLAG` et
  lire `world.messages` (ex : porte d'Alidoor refuse au nord/est par garde
  `coordz` → cliquer depuis le sud ; Keli à <10 tuiles bloque, respawn 10 s
  → enchaîner tie→clé→talk dans la foulée, driven par les messages serveur).
- `find_loc` : rayon 150 + attente streaming 20 ticks (un match global
  faisait marcher 600 tuiles vers un manhole fantôme).
- `finish_dialog_like` / `drain_dialog` : ne concluent que sur silence
  (frames `IF_*`/`MESSAGE_GAME`/inv = serveur vivant). Le modal `None`
  transitoire (`if_close` avant le mesbox suivant, ex. les 25 os de
  Traiborn) ne termine plus le dialogue.
- Escaliers : passer par la table des portails (`solver.ride_loc`, bon
  côté calculé) au lieu d'un `op_loc` aveugle (reachLoc angulaire).
- Manhole du palais Varrock absent des locs (live + statique) : cliquer
  aveugle avec les coords de `transitions.json` + vérification d'effet.
- `goto` doit viser une tuile marchable (vérifier via collision map),
  sinon le bot naît dans un mur et rien ne route.
- Zone affichée (`lifescape/zones.py`, ligne `zone:` au-dessus de `intent`
  dans le spectate) : labels `labels.dat` du `worldmap.jag` serveur, rayons
  par type (place 60 / ville 120 / région 600, repli uniquement), instances
  d'events (`maze`, `mime`, mêmes boîtes que `randomevents.py`) prioritaires.
  `zone_dist=None` pour les events. Redémarrer le daemon après toute modif
  de `lifescape/client/core.py`, `orchestration/jobs.py` ou `web.py` (code
  importé au démarrage, sessions à rouvrir).
- Banque pleine = refus silencieux "subscribe for a members account..."
  (bank_deposit affiche désormais les messages serveur en échec).
- `worn_only` = état final (le solve habille, le check prouve la
  possession), pas prérequis porté.

## Fichiers clés
- Solvers : `scripts/quests/*.py` (+ `solver.py` helpers, `common.py` runner,
  `gamedata/gamedata/quest_requirements.py` prérequis).
- SDK : `lifescape/client/{core,actions,movement}.py`, `lifescape/{route,doors,portals.py}`,
  `lifescape/data/transitions.json` (généré par `scripts/dev/dump_transitions.py`).
- Sondes : `scripts/dev/probe_*.py`, registre `orchestration/registry.py`.
- Tickrate : `../lostcity/engine/data/config/world.json` (`tickRate`, 20 dev / 600 prod).

## Point de reprise immédiat
1. `scripts/orchestrator.py status` (session admin vivante ?). Si le daemon
   a été tué : `(nohup ../lostcity/.pypy/bin/pypy3 -m orchestration.daemon
   > run/daemon.log 2>&1 &)` puis `open admin`.
2. **Maze** (priorité) : `call admin cheat "~maze"` puis
   `run admin probe_maze_solve` et lire `maze door ... took N ticks`,
   `maze: N ticks in, X% left`, `maze solve=`. Le dernier correctif
   (bloquer l'arête quand une porte ne fait pas changer de tuile) n'a pas
   encore été rejoué.
3. **BKF** : `run admin quest_black_knights_fortress` (save `spy=2` :
   `_ensure_L1_east` → sabotage → Amik). `_leave_L1_east` (sortie validée
   tuile par tuile : mur nord ouvert, clic porte 3 puis pas ouest) n'a pas
   encore tourné en run complet.

État au dernier passage : `admin` connecté, idle, ~(3100,3160) ou dans le
maze ; kit BKF porté (iron_chainbody + bronze_med_helm), 1 chou normal,
quelques k d'or. Il est mort une fois (swarm ignoré) : kit redonné via
`run admin give <item> 1`. Sauvegardes : `spy=2`, qp=33.


## Blocage connu : random event maze (détail serveur)
**Corrigé côté serveur** (contenu, non versionné ici : `content/` est gitignoré
dans ce clone) :
- la statue de sortie (`macro_maze_complete`) est recouverte par le décor de
  sol `dugupsoil` : elle n'est cliquable depuis aucune tuile (« I can't reach
  that! »). La fin de l'événement est donc détectée par son timer quand le
  joueur atteint le centre (`coordx 2909..2913`, `coordz 4573..4577`) ;
- `macro_return_teleport` utilisait `p_teleport`, qui ne déplaçait personne :
  passé en `p_telejump` (retour effectif au point de départ) ;
- `::maze` appelait une copie du démarrage qui ne posait ni les varps ni le
  timer : il appelle maintenant `~start_macro_maze`, qui pose désormais aussi
  `%macro_event = ^macro_maze` (sinon d'autres events spawnent pendant le
  labyrinthe et un fail-teleport sort le bot du maze en plein solve).

Récupération d'un compte bloqué : `call <compte> cheat "~mazeend"` → téléport
au centre → le timer termine l'événement et renvoie le joueur à son origine.
`::tele` prend des virgules : `call <compte> cheat "tele 0,50,50,22,18"`.

Côté client (`scripts/randomevents.py`) : voir la section
« Maze : état au 03/11 » plus haut — Dijkstra coûtée, jambes en paquet
explicite, `open_door` du SDK, arêtes refusées mémorisées partout. Reste
l'oscillation sur une porte qui n'ouvre que dans l'autre sens (correctif
écrit, à rejouer).

## Random event cube (Strange box)
`scripts/rescue_cube.py` résout l'interface : il lit les trois modèles
(centre/côté/haut), la question (« What colour is the <shape> ? » ou
« Which shape is <colour> ? ») et clique le bon bouton-réponse. Détection
automatique dans `randomevents.check` (un `macro_cube` en inventaire suffit),
rescue mappé `cube` → `rescue_cube`. Validé live sur 29 cubes d'affilée
(admin), 0 restant, récompense versée. Le SDK expose désormais `Bot.com_id()`
et l'état publie `messages` et `texts` : plus besoin de sonde pour lire un
message serveur ou une interface.

## Leçons knights (utiles pour les suivantes)
- Varp non transmis (`transmit=False`, ex. `squire`) : le gate live ne passe
  jamais mid-run, progression d'un stage par run via le save au logout.
- Consommables mid-quête (pie étape 3, blurite+barres étape 6) : hors
  `quest_requirements.py` (items vides + checks de kit par étape dans solve).
- Armoire Vyvin : ne répond que du sud (`stand=(2984,3335,2)`, param `stand`
  de `solver.operate_loc`) ; fouille pilotée par la position de Vyvin
  (rayon 1 sur armoire ET joueur), pas d'attente aveugle.

## Note shield_of_arrav (décision utilisateur)
- Moitié de shield par échange avec un joueur de l'autre gang : item
  **à retirer** des prérequis (`arravshield1/2`), le bot utilisera un
  autre script pour l'obtenir (trade). Ne pas le farmer, ne pas le
  donner via `probe_give` comme acquis légitime.
