# gamedata

Base de connaissances du jeu pour les scripts LifeScape et l'agent qui
les développe : objets, PNJ (+ positions de spawn), décors, composants
d'interface, vendeurs (+ vendeurs requis), sorts, recettes de craft,
quêtes (+ requis), spawns au sol (avec cooldown).
Chaque entrée porte son flag members (F2P ou membres).

```sh
# régénérer depuis le serveur (rev 274)
cd ../../lostcity/engine && npx tsx tools/gamedata-dump.mts ../../LifeScape/gamedata/data/

# boucle de vérification (statique + live)
../../lostcity/.pypy/bin/pypy3 scripts/verify.py
../../lostcity/.pypy/bin/pypy3 scripts/verify_live.py t48
```

```python
from gamedata import objs

sword = objs.by_name("bronze sword")
print(sword[0].obj_id, sword[0].ops, sword[0].alch_high)
```

Règles dans `AGENTS.md`. Données dans `data/` : générées, jamais éditées.
Retour d'expérience transverse dans `../FEEDBACK.md`.
