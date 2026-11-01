# gamedata

Base de connaissances du jeu, en dehors du SDK LifeScape, utilisée par les
scripts (`scripts/`) et par l'agent qui développe les scripts.

Contenu : tous les objets (ids, noms, textes, actions, prix, high/low alch),
PNJ (ids, noms, positions de spawn), décors, composants d'interface, vendeurs,
skills, recettes de craft, quêtes (requis items/skills), sorts, spawns au sol
avec cooldowns. Chaque entrée porte son flag members (True = réservé
aux membres, False = utilisable en F2P, None = le serveur ne le dit pas
— cas des quêtes et des décors, documenté, pas inventé).

## Flag members : un seul champ

Chaque entrée porte `members` (booléen, ou null quand le serveur ne le
dit pas : quêtes et décors). Pas de champ zone F2P : la question
"accessible en F2P ?" se répond avec `members` seul — un seul endroit
où regarder, pas deux à croiser.

Spécial banques : les décors n'ont aucun flag serveur, donc le `members`
d'un booth est dérivé de sa position (hors carte F2P = True), calculé au
dump depuis `free2play.csv`, level ignoré comme en prod. Spécial arbres :
`members: null` sur les spots ; utiliser le flag du produit (`logs`,
`oak_logs`... dans `objs.json`), exposé en `product_members` dans
`trees.json`.

## Règles (mêmes contraintes que LifeScape, car importé par ses scripts)

- Python pur, lisible par un humain, compatible PyPy
  (`../../lostcity/.pypy/bin/pypy3`). Zéro dépendance externe : uniquement
  la bibliothèque standard embarquée.
- Imports toujours en début de fichier, jamais dans les fonctions.
- Un rôle par module, un fichier < 1000 lignes, découper avant la limite.
- Noms explicites (jamais 1-2 caractères), petites fonctions.
- Données : `data/*.json` générés par dump, JAMAIS édités à la main.
  Seul le code dans `gamedata/` est écrit à la main.
- Timeouts et sleeps calibrés sur des durées mesurées en live
  (voir `../AGENTS.md` et `../../lostcity/AGENTS.md`).

## Régénérer les données

```sh
cd ../../lostcity/engine && npx tsx tools/gamedata-dump.mts ../../LifeScape/gamedata/data/
```

## Vérifier (boucle de vérification)

```sh
../../lostcity/.pypy/bin/pypy3 scripts/verify.py            # statique + live
../../lostcity/.pypy/bin/pypy3 -m unittest discover tests   # si tests ajoutés
```

`verify.py` compare le dump aux sources serveur, rejoue des vecteurs
connus et fait des contrôles live rapides (login bot, positions, shops).
Tant qu'un contrôle échoue, corriger le dump ou le code, puis relancer.
