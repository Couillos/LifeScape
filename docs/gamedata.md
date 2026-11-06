# Gamedata

Base de connaissances du jeu pour les scripts : objets, PNJ (+ spawns), décors,
composants d'interface, vendeurs, sorts, recettes, quêtes (+ requis), spawns au
sol avec cooldowns. Accès typé depuis Rust (`lifescape-gamedata`,
`rs/crates/gamedata/src/` : `lib.rs` + `banks`, `combat_spots`, `equipment`,
`fishing`, `food`, `quests`, `quest_requirements`, `recipes`, `rocks`, `shops`,
`spawns`, `spells`, `trees`, `varps`) ; l'index du tableau *est* l'id.

Données : `gamedata/data/*.json` (`bank_stands`, `comps`, `equipment`, `foods`,
`gametypes`, `locs`, `npcs`, `objs`, `quests`, `rocks`, `shops`, `spawns_obj`,
`spells`, `transitions`, `trees`, `varps`). Générées par dump, jamais éditées.
Régénérer (rev 274) :

```sh
cd ../lostcity/engine && npx tsx tools/gamedata-dump.mts ../../LifeScape/gamedata/data/
```

(`tools/lifescape-dump.mts` : dump des définitions vers `gametypes.json`.)
Chaque entrée porte son flag members (`True` = membres, `False` = F2P,
`null` = le serveur ne le dit pas : quêtes et décors). Cas banque : le members
d'un booth dérive de sa position (hors carte F2P = membres). Cas arbres :
`product_members` expose le flag du produit (`logs`, `oak_logs`…).

Boucle de vérification : `cargo test --manifest-path rs/Cargo.toml -p lifescape-gamedata`.
