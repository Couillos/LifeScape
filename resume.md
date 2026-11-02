# LifeScape — reprise de session (F2P quests)

## Objectif
Finir les 18 quêtes F2P avec un script fiable par quête. **15/18 terminées.**

## Terminées (save + daemon OK)
cook, sheep, doric, priest, gobdip, imp, hetty, romeojuliet, runemysteries,
haunted, vampire, hunt (pirate=4), prince (princequest=110),
knights (squire=7), demon (demonstart=30).

## En cours
- **black_knights_fortress** : save `spy=1` (varp 130) = étape « démarré ».
  Entrée OK après le fix save (porte 1 → banquet → étage), **bloqué sur la
  porte intérieure** `bkfortressdoor3` en `(3025,3511,1)` : depuis la salle de
  banquet montée, le bot est en `(3030,3510,1)` et le chemin est coupé
  (`oldcastlewall`, `laddertop_directional`) — il faut trouver la bonne
  montée/le bon couloir de l'étage avant la grille puis le sabotage.
- **Reste après** : shield_of_arrav (échange à 2 comptes, voir note),
  dragon_slayer en dernier (32 QP requis).

## Régressions corrigées côté sessions (à garder en tête)
- Un run de quête force `bot.relogin()` : la save n'est écrite qu'au logout,
  donc en session persistante les gates d'étape (`max(live, save)`)
  régressaient et le solver rejouait l'entrée.
- `find_loc` lit `world.static_actions` : une loc actionnable masquée par un
  décor sur la même tuile (porte intérieure BKF, statue du maze) est trouvée.

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
- Banque pleine = refus silencieux "subscribe for a members account..."
  (bank_deposit affiche désormais les messages serveur en échec).
- `worn_only` = état final (le solve habille, le check prouve la
  possession), pas prérequis porté.

## Fichiers clés
- Solvers : `scripts/quests/*.py` (+ `solver.py` helpers, `common.py` runner,
  `gamedata/gamedata/quest_requirements.py` prérequis).
- SDK : `lifescape/client/{core,actions,movement}.py`, `lifescape/{route,doors,portals.py}`,
  `lifescape/data/transitions.json` (généré par `scripts/dump_transitions.py`).
- Sondes : `scripts/probe_*.py`, registre `orchestration/registry.py`.
- Tickrate : `../lostcity/engine/data/config/world.json` (`tickRate`, 20 dev / 600 prod).

## Point de reprise immédiat
1. `scripts/orchestrator.py status` (session admin vivante ?).
2. Sinon `open admin` puis `run admin quest_black_knights_fortress` (gates sautent
   amik+portes si save≥1, sabotage direct si save≥2).
3. `see admin` pour voir ce que le bot voit (positions + distances).

État au dernier passage : `admin` est libre (L1, ~2971,3385, BKF stage 1),
inventaire sans cube. Random events : cube résolu par `rescue_cube` ; maze
terminable côté serveur mais solver client encore partiel ; mime non résolu
(abandon propre).

## Blocage connu : random event maze
**Corrigé côté serveur** (contenu, non versionné ici : `content/` est gitignoré
dans ce clone) :
- la statue de sortie (`macro_maze_complete`) est recouverte par le décor de
  sol `dugupsoil` : elle n'est cliquable depuis aucune tuile (« I can't reach
  that! »). La fin de l'événement est donc détectée par son timer quand le
  joueur atteint le centre (`coordx 2909..2913`, `coordz 4573..4577`) ;
- `macro_return_teleport` utilisait `p_teleport`, qui ne déplaçait personne :
  passé en `p_telejump` (retour effectif au point de départ) ;
- `::maze` appelait une copie du démarrage qui ne posait ni les varps ni le
  timer : il appelle maintenant `~start_macro_maze`.

Récupération d'un compte bloqué : `call <compte> cheat "~mazeend"` → téléport
au centre → le timer termine l'événement et renvoie le joueur à son origine.
`::tele` prend des virgules : `call <compte> cheat "tele 0,50,50,22,18"`.

Côté client (`scripts/randomevents.py`) : `solve_maze` avance **tuile par
tuile** (`Bot.step_to`), sans routeur ni chasse aux portes, apprend les arêtes
refusées et s'arrête proprement avec un diagnostic. Il n'est pas encore fiable
sur tous les layouts (portes `walllow` directionnelles) : un bot qui tombe dans
le maze peut rester bloqué.

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
