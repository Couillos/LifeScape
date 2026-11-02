# LifeScape — reprise de session (F2P quests)

## Objectif
Finir les 18 quêtes F2P avec un script fiable par quête. **14/18 terminées.**

## Terminées (save + daemon OK)
cook, sheep, doric, priest, gobdip, imp, hetty, romeojuliet, runemysteries,
haunted, vampire, hunt (pirate=4), prince (princequest=110),
knights (squire=7).

## En cours
- **demon_slayer** : prochaine (aucun prérequis QP).
- **Reste après** : black_knights_fortress (12 QP requis),
  shield_of_arrav, dragon_slayer en dernier (32 QP requis).

## Commandes (que via l'orchestrateur, jamais de pypy direct)
```
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py start|stop|status [job] [account]
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py test   # unitaires
```
Jobs dev : `goto`, `give`, `probe_give`, `probe_door`, `probe_talk`,
`probe_use` (clic loc + use objet + dump modals/unsets/msgs),
`probe_vars`, `drop`, `bank_deposit`, `quest_reset`. Comptes test t01…t20,
`admin`/`admin` = compte de dev (jamais de cheats dans les solvers).

## Règles intangibles
- **Téléport (`goto`) = positionnement de test uniquement.** Vérifié : aucun
  solver n'importe de cheat (`grep cheat|teleport scripts/quests` → rien sauf
  un commentaire "no teleport" dans solver.py).
- Ne jamais modifier le serveur (`../lostcity/engine`, `content`) : tout se
  corrige côté client/SDK.
- Tickrate dev = 100 ms → respawns 6x plus rapides qu'en prod (Keli = 10 s).
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

## Fichiers clés
- Solvers : `scripts/quests/*.py` (+ `solver.py` helpers, `common.py` runner,
  `gamedata/gamedata/quest_requirements.py` prérequis).
- SDK : `lifescape/client/{core,actions,movement}.py`, `lifescape/{route,doors,portals.py}`,
  `lifescape/data/transitions.json` (généré par `scripts/dump_transitions.py`).
- Sondes : `scripts/probe_*.py`, registre `orchestration/registry.py`.
- Tickrate : `../lostcity/engine/data/config/world.json` (`tickRate`, 100 dev / 600 prod).

## Point de reprise immédiat
1. `scripts/orchestrator.py status` (aucun job attendu).
2. `start quest_demon_slayer admin` (kit : bones/bucket à vérifier dans
   `gamedata/gamedata/quest_requirements.py`, entrée `demon`).

## Leçons knights (utiles pour les suivantes)
- Varp non transmis (`transmit=False`, ex. `squire`) : le gate live ne passe
  jamais mid-run, progression d'un stage par run via le save au logout.
- Consommables mid-quête (pie étape 3, blurite+barres étape 6) : hors
  `quest_requirements.py` (items vides + checks de kit par étape dans solve).
- Armoire Vyvin : ne répond que du sud (`stand=(2984,3335,2)`, param `stand`
  de `solver.operate_loc`) ; fouille pilotée par la position de Vyvin
  (rayon 1 sur armoire ET joueur), pas d'attente aveugle.
