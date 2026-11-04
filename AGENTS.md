# LifeScape

Framework Python (PyPy, stdlib uniquement) pour écrire des scripts
de botting sur notre instance LostCityRS (révision 274).

Instructions canoniques : `../lostcity/AGENTS.md`.
Interpréteur imposé : `../lostcity/.pypy/bin/pypy3`.

```sh
../lostcity/.pypy/bin/pypy3 scripts/hello.py   # exemple : login + chat
../lostcity/.pypy/bin/pypy3 -m unittest discover tests
```

## Nommage des variables

Jamais de noms de variables de 1 ou 2 caractères (`x`, `z`, `b`,
`e`, `nid`…). Chaque nom doit être compréhensible par un développeur
qui lit le code (`door_x`, `chicken_id`, `chat_line`,
`magic_level`). Idem pour les fonctions et les modules.

## Contraintes du code (de `../lostcity/AGENTS.md`)

- Python pur, lisible par un humain, compatible PyPy.
- Zéro dépendance externe : uniquement la bibliothèque standard embarquée
  (socket, struct, hashlib, zlib, base64, sqlite3, unittest, ...).
- Interdit : pip, paquets tiers, extensions C, code généré.
- Imports toujours en début de fichier, jamais dans les fonctions.
- Formatage chaînes : f-string uniquement, jamais `%` avec argument.
- Architecture claire, un rôle par module, arborescence qui dit ce que fait quoi.
- Code qui s'explique tout seul : noms explicites, petites fonctions.
  Commentaires rares, seulement pour le "pourquoi" jamais pour le "quoi".
- Référence protocole : le code du serveur (`Engine-TS`, branche 274),
  jamais de suppositions, vérifier dans le code puis valider en live.
- Régénérer les définitions après mise à jour du serveur/jeu :
  `cd ../lostcity/engine && npx tsx tools/lifescape-dump.mts ../../LifeScape/lifescape/data/gametypes.json`.
  Outils engine liés : `tools/lifescape-*.mts` (dump, diff routage/flags).

## Optimisation : faible empreinte RAM et CPU

Tout développement doit être optimisé, objectif faible empreinte
RAM et CPU : pas d'allocation ni de boucle inutile, pas de
polling serré, réutiliser les structures existantes.

## Aucun fallback sans demande explicite

Ne jamais ajouter de fallback (silencieux ou non) sans demander
d'abord à l'utilisateur : en cas d'échec, remonter l'erreur et
attendre sa décision.

## SDK : gestion d'erreur irréprochable

Le SDK (`lifescape/`) est la couche critique : toute erreur doit être
explicite, typée et contextualisée (position, cible, état observé), puis
remontée au script. Jamais d'exception avalée, jamais de fallback
silencieux. Corriger et améliorer cette gestion en continu, à chaque
test live.

## Découpage : aucun fichier au-dessus de 1000 lignes

Un fichier qui approche 1000 lignes cache un mélange de responsabilités.
Avant la limite, réfléchir à l'arborescence, découper par rôle et garder
l'API publique stable.

## Pathfinding : diagnostiquer, pas contourner

À chaque erreur de pathfinding, chercher la cause et la corriger si c'est
possible. `walk_to`/`approach` lèvent un `Unreachable` qui dit pourquoi :
tuile cible bloquée et son décor, point atteignable le plus proche en
distance de parcours, décors bloquants, et cause probable (clé/objet,
niveau, quête, argent/péage, zone membres, porte/grille fermée). Un
script ne masque jamais l'échec par un fallback silencieux : il logge
l'erreur avant de changer de cible.

Quand la cible est une tuile à collision (mur, table, comptoir), le
routeur choisit le point atteignable le plus proche **en distance de
parcours** (BFS), pas en distance à vol d'oiseau.

## Le serveur est la source de vérité

Le code et le contenu du serveur (`../lostcity/engine`, `../lostcity/content`)
font foi : le SDK, gamedata et les scripts s'adaptent à lui, jamais
l'inverse. Ne modifier le serveur (collision, quêtes, prix, taux, opcodes,
données) que sur demande explicite de l'utilisateur. Un dump ou un outil
de lecture peut être ajouté pour comprendre le serveur, mais il ne change
pas son comportement.

## Tout script part de n'importe où

Un script doit démarrer depuis n'importe quelle position, niveau de
compétence, inventaire et banque. Il découvre l'état réel du joueur
(stats, objets portés, inventaire, banque), choisit le meilleur stuff
disponible pour ses niveaux, et rejoint sa zone de travail avec le
pathfinding du SDK. Pas de position de départ supposée, pas de stuff
codé en dur, pas d'échec si la banque est vide : il s'adapte.

## Comptes et cheats

Seul le compte `admin` (mot de passe `admin`) est admin, réservé à
l'agent pour ses vérifications manuelles. Le compte `bite`
(mot de passe `bite`) est personnel : ne jamais l'utiliser, ne jamais
s'y connecter. Les comptes de test `t01`…`t48` et nommés
(mot de passe = pseudo) sont des comptes joueurs normaux, leurs runs
doivent rester légitimes (les cheats fausseraient les états, inventaires
et validations).
Aucun script commité ne doit utiliser de cheats (`bot.cheat`,
`::...`) : les cheats servent uniquement aux vérifications manuelles
et au développement, jamais dans du code.

## Fonctionnalités réutilisables → SDK

Quand un script a besoin d'un mécanisme qui servirait à d'autres
scripts (parler/marchander avec un PNJ, boucle de récolte, dépôt et
retrait en banque, achat/vente en magasin...), l'implémenter dans le
SDK (`lifescape/`), pas en copie locale dans le script. Les scripts
composent avec le SDK + `gamedata` ; ils ne dupliquent jamais de
logique partagée. Même règle pour les données : tout ce qui vient du
jeu va dans `gamedata` (dump + requêtes typées), jamais en dur dans
un script.

## Paquets et protocole : SDK uniquement

Seul le SDK (`lifescape/`) parle le protocole réseau : ouverture et
fermeture de session, trames, opcodes, encodage/décodage des paquets.
Un script ne manipule jamais `connection`, `Writer`, `client_op_id`
ni un id d'opcode : il appelle une méthode du SDK (`bot.op_loc`,
`bot.op_npc_t`, `bot.if_button`, `bot.close`...). Une fonctionnalité
ajoutée au SDK doit rester utilisable par n'importe quel script et
contexte, pas seulement par celui qui l'a motivée. Les sondes de
diagnostic (`scripts/dev/probe_*.py`) peuvent descendre au niveau paquet,
mais jamais un script de bot commité.


## Ticks serveur réduits pour le dev

Le serveur tourne en ticks de 20 ms (`tickRate: 20` dans
`engine/data/config/world.json`, défaut 600) pour accélérer dev et
tests. Retour au défaut : remettre 600 (ou supprimer la clé) +
`docker compose restart server`.

## Attentes en ticks, jamais en secondes

Toutes les attentes des scripts Python s'expriment en ticks serveur
(`bot.world.ticks`, incrémenté à chaque paquet player-info, donc à
chaque tick quand on est connecté), jamais en secondes : le même
code se comporte pareil à 600 ms comme à 20 ms. Helper partagé :
`lifescape.waits.wait_ticks(bot, n)` (+ `wait_hitpoints`,
`ban_area`, constantes `TICK_MS` et bornes ticks dans le SDK :
`walk_to`, `take`, `approach` prennent des ticks). Les garde-fous
process (spawn/join/kill, espacement relogin, timeouts socket) restent
en secondes et sont documentés comme tels.

## Mesures in-game, jamais de sleep

Calibrer chaque attente sur des durées mesurées en live, pas sur des
valeurs rondes ; préférer une condition de jeu observée (message,
inventaire, NPC visible, level) à une attente aveugle. Protocole :
mesurer le temps réel pris par l'action sur plusieurs essais, puis
fixer la borne en ticks = mesure + petite marge. Un sleep nu
(`time.sleep`) est interdit : attendre, c'est pomper en boucle
jusqu'à un état observable, avec une borne en ticks au cas où.

## Orchestrateur (dossier `orchestration/`)

Un seul daemon supervise tous les bots, un worker de session par compte.
Il possède le cycle de vie (open/close des sessions, run/unload des
scripts, relogin borné), les logs et l'état live ; il ne parle jamais
le protocole réseau (ça reste le SDK).

- `paths.py` : layout `run/` + IO JSON atomique
  (`run/users/<compte>.json` état fusionné, `run/users/<compte>.bot.json`
  heartbeat du SDK, `run/logs/<compte>.log`, `run/session-<compte>.sock`
  commande du worker).
- `registry.py` : scripts lançables + mapping événement → rescue.
- `jobs.py` : modèle d'une session (`phase` idle|running, `script`
  nullable, `status`, `last_error`) et son rescue en cours.
- `daemon.py` : un worker de session par compte, lecture des `@@EVENT`,
  machine à états (event non résolu → unload + rescue en session puis
  reprise du script interrompu), socket unix.
- `session_worker.py` : session connectée persistante, thread script
  dédié, file de commandes drainée aux safe-points `pump()`,
  lectures depuis le snapshot heartbeat.
- `cli.py` : client CLI + REPL `shell` (démarre le daemon au besoin).
- `web.py` : dashboard et API HTTP (port 8765).

```sh
# CLI (le daemon démarre tout seul)
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py open olivia_king
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py run olivia_king fishing 1
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py call olivia_king inventory
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py shell olivia_king
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py status
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py show olivia_king
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py logs olivia_king 40
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py unload olivia_king
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py close olivia_king
../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py shutdown
# daemon seul
../lostcity/.pypy/bin/pypy3 -m orchestration.daemon
```

Docker : service compose `bots` (conteneur `lifescape-bots`), dashboard
sur **http://5.83.152.180:8765/**. Variables lues par le SDK et le daemon :
`LIFESCAPE_SERVER_HOST`/`LIFESCAPE_SERVER_PORT` (serveur de jeu),
`LIFESCAPE_WEB_PORT` (8765), `LIFESCAPE_STATE` (posé par le daemon).

Contrat d'un script de bot :

- `run(bot, *args)` : le worker ouvre la session avant et la referme
  après ; le script ne fait ni `Bot()`, ni `login`, ni `close`, et lit
  ses arguments depuis `args` (l'orchestrateur appelle `run t01 goto
  3200 3227`, pas `pypy scripts/goto.py`) ;
- `bot.intend("...")` à chaque changement d'action : c'est l'intention
  affichée en temps réel par le dashboard et l'état du compte ;
- `randomevents.check(bot)` rapporte les événements non résolus
  (`@@EVENT {...}`) ; le worker décharge le script et le daemon joue le
  rescue mappé en session, puis reprend le script interrompu ;
- les bornes et attentes jeu s'expriment en ticks (`bot.world.ticks`),
  jamais en secondes ni `time.sleep` (voir `lifescape/waits.py`,
  `TICK_MS = 20`) ; seuls les garde-fous process (spawn/join/kill,
  espacement relogin, timeouts socket) restent en secondes, documentés
  comme tels.

Spectate : l'état live contient position, hp, ticks, PNJ/décors/joueurs
proches, inventaire et chat ; `web.py` en dessine une grille 15×15 autour
du bot (le serveur garde son propre `/spectate`). Un compte sans job en
cours reste listé (`status: idle`) avec son dernier instantané : il reste
spectatable au lieu de disparaître de la liste.

Arrêt du service : `orchestrator.py shutdown` (le daemon sort en 0, le
service ne redémarre pas) ; relance : `docker compose start bots`.

## Tout passe par l'orchestrateur

Ne jamais lancer un bot, un rescue ou les tests en direct (`pypy scripts/...`) :
l'orchestrateur est le seul point d'entrée. Il garantit un job par compte,
les logs, l'état live, le logout propre et les relances ; un run manuel
laisse des sessions orphelines et des états faux (« run brocant »).

Les outils de diagnostic aussi : `check_transitions` et `check_doors` sont
des scripts supervisés (`open admin` puis `run admin check_transitions`).
C'est ce qui alimente l'état live et donc le spectate du front ;
un run direct apparaît comme un bot « closed » sans carte.

```sh
# bots : une session par compte, scripts chargés/déchargés sans reconnexion
scripts/orchestrator.py open <account>
scripts/orchestrator.py run <account> <script> [args...]
# appels SDK directs dans la session (snapshot ou safe-point pump)
scripts/orchestrator.py call <account> <method> [args...]
scripts/orchestrator.py shell <account>
# suite unitaire, exécutée comme session éphémère (logs + code de sortie)
scripts/orchestrator.py test [pattern]
# suivi
scripts/orchestrator.py status | show <account> | see <account> | logs <account> 40
```

`see` affiche en plus une scène ASCII 25×25 centrée sur le bot (nord en
haut : `@` bot, `N` pnj, `P` joueur, `*` route, `X` cible, `o` objet,
`D` décor actionnable, `#` mur — murs lus des décors streamés, le vide
lit ouvert), le tick jeu + heure du snapshot, et chaque message jeu avec
son tick (`msg [t=123]`). Les lignes de log worker sont préfixées
`[HH:MM:SS]` (les lignes `@@EVENT` restent intactes pour le daemon).
Les logs de jambes pathfinding portent `dest=(x,z,niveau)` du `walk_to`
en cours (imbrication restaurée) : comparer avec la tuile d'arrivée.

Debug pathfinding sans marcher : `run <account> debug_walk <x> <z>
[level]` (script supervisé, session idle) ou `debug <account> <x> <z>
[level]` (commande CLI : le worker joue `Bot.debug_walk` sur le pump
thread, le script en cours n'est jamais déchargé). Sortie : statique
autour de la cible, trajets brut/portes-ouvertes, frontière injoignable
avec bloqueurs nommés, portes à 15 tuiles avec état live
ouvert/appris ; lecture seule. `run <account> probe_tiles
<x> <z> [level] [radius]` (marchabilité statique + pas vers l'ouest).
Quand le routeur dit mur mais la carte statique dit ouvert, c'est de
l'état dynamique empoisonné (porte apprise, unseal, mur invisible) :
`learned_total`/`unsealed` dans debug_walk, session neuve (`close` puis
`open`) pour repartir propre.

## Debugger, tester, lancer (sessions)

Lancer un script : `open` (connecte, `idle`), `run` (charge,
`running`), `unload` (décharge, session gardée, même pid), `close`
(logout propre). Un `run` qui lève rend `idle` + `last_error`, jamais
de mort de session ; un event non résolu déclenche le rescue en
session puis reprend le script.

Tester, dans l'ordre : unitaires (`-m unittest discover tests`
depuis la racine, 227 attendus), session éphémère
(`orchestrator.py test [pattern]`), puis live sur `t01` (`open`,
`run`, `call`, `unload`, `close`, `show` ×2 pour voir les ticks
avancer). Une théorie protocole = une sonde live avant d'entrer
dans le SDK ; jamais `admin`/`bite` pour les tests.

Debugger sans script temporaire : `show` (état fusionné + `last_error`),
`see` (vue triée par distance), `logs` (marqueurs
`=== session open/run/unload/call/close ===`), `call` (lectures
depuis le snapshot même si un script tourne ; écritures au prochain
safe-point `pump()`, borne 500 ticks), `shell` (REPL + `help`).
Erreurs franches à connaître : `no session` (ouvrir d'abord),
`already open`, `busy` (script sans `pump`), `unknown method`
(hors whitelist), `forbidden` (`cheat` hors `admin`).

Les comptes tournent en parallèle : un thread de supervision et un process
worker par compte, donc plusieurs bots peuvent tourner en même temps sans
se bloquer. Le dashboard (anglais) : `http://5.83.152.180:8765/`.

Le service `bots` monte `../LifeScape` en direct (scripts, SDK, gamedata,
orchestration) et `./engine` en lecture seule : un script, le SDK ou une
table gamedata modifiés sont visibles sans redémarrer le service. Le
registre se recharge tout seul (`registry.reload_if_changed`) quand on
ajoute un script ; le worker de session persiste tant que la session est
ouverte (seul `run` charge du code frais). Seul le code du daemon déjà
importé (web, jobs, superviseur) demande encore un
`docker compose restart bots`.

## Interface : noms gamedata, pas d'ids

Le dashboard ne montre jamais un id brut : `orchestration/naming.py`
résout noms de PNJ, décors, objets, fenêtres (composants) et flèche
d'indice via gamedata, et humanise les snake_case (`young_dark_wizard`
→ « Young dark wizard », `olivia_king` → « Olivia king »). gamedata
garde des index id→nom et les listes `all_*` en cache : 400 000
lookups ≈ 60 ms. Le décor purement cosmétique (herbe, brindilles,
fleurs, tapis…) est filtré ; les objets, PNJ, monstres, portes et
structures restent affichés.

## Banque et pathfinding

- `Bot.bank_deposit_all(side_com, keep_ids)` : cette révision n'a pas
  de bouton « déposer tout l'inventaire » ; l'action groupée est
  « Deposit All » par slot. Le SDK envoie tous les clics d'un coup,
  attend que le sac ne contienne plus que les objets gardés, puis
  réessaie une fois. Les trois scripts de métier l'utilisent.
- Chaque jambe de marche publie son itinéraire (`route`, `route_target`)
  dans l'état live ; le canvas le dessine (ligne + pointillés + anneau
  de destination).
