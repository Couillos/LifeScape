# gamedata

Base de connaissances du jeu pour les scripts LifeScape et l'agent qui
les développe : objets, PNJ (+ positions de spawn), décors, composants
d'interface, vendeurs (+ vendeurs requis), sorts, recettes de craft,
quêtes (+ requis), spawns au sol (avec cooldown).
Chaque entrée porte son flag members (F2P ou membres).

```sh
# régénérer depuis le serveur (rev 274)
cd ../../lostcity/engine && npx tsx tools/gamedata-dump.mts ../../LifeScape/gamedata/data/

# boucle de vérification : tests Rust du workspace
cargo test --manifest-path rs/Cargo.toml -p lifescape-gamedata
```

Accès typé depuis Rust (`lifescape-gamedata`), par id : l'index du
tableau *est* l'id.

Règles dans `AGENTS.md`. Données dans `data/` : générées, jamais éditées.
Historique archivé dans `../docs/journal/` (ne pas utiliser comme référence).
