# Gamedata

Base de connaissances du jeu pour les scripts : objets, PNJ (+ spawns), décors,
composants d'interface, vendeurs, sorts, recettes, quêtes (+ requis), spawns au
sol avec cooldowns. Accès typé depuis Rust (`lifescape-gamedata`,
`rs/crates/gamedata/src/` : `lib.rs` + `banks`, `combat_spots`, `equipment`,
`fishing`, `food`, `quests`, `quest_requirements`, `recipes`, `rocks`, `shops`,
`spawns`, `spells`, `trees`, `varps`) ; l'index du tableau *est* l'id.

Données : `gamedata/data/*.json` (`comps`, `equipment`, `foods`,
`gametypes`, `locs`, `npcs`, `objs` (+ `tradeable`), `quests`, `rocks`,
`shops`, `spawns_obj`, `spells`, `transitions`, `trees`, `varps`,
`drops`). Générées par dump, jamais éditées.
Régénérer (rev 274) :

```sh
cd ../lostcity/engine && npx tsx tools/gamedata-dump.mts ../../LifeScape/gamedata/data/
cd ../lostcity/engine && npx tsx ../../LifeScape/gamedata/tools/dump_plan.mts ../../LifeScape
```

(`tools/lifescape-dump.mts` : dump des définitions vers `gametypes.json`.)
Le second script (`gamedata/tools/dump_plan.mts`, lecture seule du cache
et des scripts compilés) ajoute `tradeable` à `objs.json` (flag brut du
cache, lu en mode membres pour ne pas écraser les objets membres) et
écrit `drops.json` (paires PNJ→objet extraites des `ai_queue3` compilés
du dossier `drop tables`, sujets catégorie étendus, procs partagés
`randomherb`/`randomjewel` résolus ; butins `death_drop` par défaut,
parchemins d'indice et tables `dbrow` non modélisés).
Chaque entrée porte son flag members (`True` = membres, `False` = F2P,
`null` = le serveur ne le dit pas : quêtes et décors). Cas banque : le
members d'un booth dérive de sa position (hors carte F2P = membres). Cas
arbres : `product_members` expose le flag du produit (`logs`, `oak_logs`…).

Accès F2P (`lifescape-gamedata::f2p`) : objets et recettes exigent
`members == false` (toutes les lignes `brew`/`fletch` sont ainsi exclues) ;
magasins et drops excluent les lignes connues membres (les étals sans
vendeur restent listés). Les 18 quêtes F2P sont la liste blanche du
registre (`F2P_QUESTS`), résolue via `quests.json`.
Sources d'un objet (`lifescape-gamedata::item_sources`) : magasin
achetable (réassorti ou avec vendeur), spawn, recette, sous-produit de
cuisson (`extra.burnt`), drop PNJ, billet (`cert_`), bûcheronnage,
minage, pêche. Orphelins connus (180/1128 au 2026-11-09, listés par le
test `coverage_report`) : récompenses de quêtes/pistes/événements,
dialogue (teintures, tannage), préparations de cuisine et restes mangés.
Table d'XP (`lifescape-gamedata::xp`, unités serveur = 10× l'affichage :
niveau 2 à 830, 99 à 130344310).

Boucle de vérification : `cargo test --manifest-path rs/Cargo.toml -p lifescape-gamedata`.
