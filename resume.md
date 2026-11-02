# LifeScape — reprise de session (F2P quests)

## Objectif
Finir les 18 quêtes F2P avec un script fiable par quête. **15/18 terminées.**

## Terminées (save + daemon OK)
cook, sheep, doric, priest, gobdip, imp, hetty, romeojuliet, runemysteries,
haunted, vampire, hunt (pirate=4), prince (princequest=110),
knights (squire=7), demon (demonstart=30).

## En cours
- **black_knights_fortress** : stage 1 (spy=1), déguisement porté
  (chainbody+helm), bot au L1 près porte secrète/trou
  (`(3030,3510)` / trou `(3031,3507)`), chevaliers aggro
  ("Die intruder!!!", HP 99). Reste : sabotage → report Amik.
- **Reste après** : shield_of_arrav (voir note échange ci-dessous),
  dragon_slayer en dernier (32 QP requis).

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

## Blocage connu : random event maze
`admin` est resté bloqué dans le maze (`0_45_71`, ~2911,4574) : la statue de
sortie (`macro_maze_complete`, loc 3634, 3×3 en 2911,4575) ne répond à aucun
clic, y compris sur ses 9 tuiles et sur toutes les tuiles voisines. Côté carte,
le sol sous la statue porte `dugupsoil1/2_macromaze` (même tuile) ; le serveur
garde probablement la loc de décor et la résolution de clic n'atteint jamais la
statue. Les cheats `::mazeend`, `::pp2` et le `::tele` de `goto.py` ne déplacent
plus le joueur dans cet état, donc l'événement est **infini** côté bot : à
corriger côté contenu (retirer le dugupsoil sous la statue, ou déplacer la
statue), pas côté client.

Côté client, `scripts/randomevents.py` (solve_maze) a été réécrit et vérifié
live : il calcule la poche atteignable, clique les `walllow` et franchit
chaque porte d'un pas (`Bot.step_to`), il atteint la tuile voisine de la statue
puis s'arrête proprement avec un diagnostic. Le SDK expose désormais
`world.static_actions` : les locs actionnables qu'un décor posé sur la même
tuile masquait dans `static_locs` (c'était la cause du solver qui tournait à
vide).

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
