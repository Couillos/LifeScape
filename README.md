# LifeScape

Framework Python (PyPy, stdlib uniquement) pour écrire des scripts
de botting sur notre instance LostCityRS (révision 274).

Instructions canoniques : `../lostcity/AGENTS.md`.
Interpréteur imposé : `../lostcity/.pypy/bin/pypy3`.

```sh
../lostcity/.pypy/bin/pypy3 scripts/hello.py            # login + chat
../lostcity/.pypy/bin/pypy3 scripts/probe_walk.py t01 10 0  # marcher
../lostcity/.pypy/bin/pypy3 scripts/probe_tutorial.py t02  # guide + porte
../lostcity/.pypy/bin/pypy3 scripts/parallel_login.py 5   # 5 bots
../lostcity/.pypy/bin/pypy3 scripts/tutorial/run.py t24  # tuto complet
../lostcity/.pypy/bin/pypy3 -m unittest discover tests     # 36 tests
```

## SDK (`lifescape/`)

Connexion et protocole :

- `buffer.py` : lecture/écriture binaire big-endian, chaînes `jstr`, bits.
- `isaac.py` : chiffrement ISAAC des opcodes, vecteurs du serveur.
- `rsa.py` : RSA brut, N/E lus dans la clé privée du serveur.
- `cache.py` : lecture du cache packé, les 9 CRC du login.
- `protocol.py` : tables d'opcodes 274 dans les deux sens.
- `words.py` : packing chat 4-bit + phrases, à l'identique du serveur.
- `connection.py` : socket, handshake, login, trames chiffrées.
- `client.py` : `Bot` (login, relogin, chat, marche, portes,
  objets, NPC, dialogues, options, bank/combat via les mêmes primitives).


## Tutoriel de bout en bout (`scripts/tutorial/`)

`run.py [username]` termine l'île des tutoriels (~15 min, validé live
jusqu'au téléport à Lumbridge). Boucle détection → action, reprenable
à tout moment, même après re-log :

- étape = texte d'instruction (coms 6180-6184) + flèche (`world.hint`)
  + onglet qui clignote (`world.side_flash`) ; jamais le varp 281
  (non transmis) ni l'inventaire (chaque étape vérifie son matériel) ;
- paquet `scripts/tutorial/` : un rôle par module (`common`, `north`,
  `mine`, `combat`, `south`, `run`) ; voir `FEEDBACK.md` pour le retour
  d'expérience SDK complet.

État du monde (`world.py`) : position, joueurs/NPC proches nommés,
objets au sol, locs (statiques + dynamiques), inventaire, stats,
varps, énergie, interfaces, textes, flèche d'indice (`hint`),
onglet qui clignote (`side_flash`), chat privé/public, options.

Pathfinding (`collision.py`, `route.py`, `doors.py`, `portals.py`) :

- collisions lues du build serveur, BFS identique au serveur
  (différentiellement testé : 30/30 chemins + tous les flags),
- portes génériques + custom ouvertes automatiquement en marchant,
- portails déclarés par l'utilisateur (escaliers, bateaux) avec Dijkstra.

Données (`lifescape/data/gametypes.json`) : définitions d'objets/NPC/objets
dumpées depuis le code du serveur. Régénérer après mise à jour du contenu :

```sh
cd ../lostcity/engine && npx tsx tools/lifescape-dump.mts \
  ../../LifeScape/lifescape/data/gametypes.json
```

## Scripts (`scripts/`)

Sondes live (une par usage) et exemples : login, chat, marche, porte,
dialogue guide, prise/dépôt d'objet, capture de trames, comparaisons
contre le moteur (`diff_routes.py`), parallèle.

## Tests (`tests/`)

Unitaires pypy + rejeu de captures live. Règle : une théorie protocole
= une sonde live avant d'entrer dans le SDK.
