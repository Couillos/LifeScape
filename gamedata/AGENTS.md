# gamedata

Base de connaissances du jeu, en dehors du SDK LifeScape, utilisée par les
scripts (`rs/crates/scripts/`) et par l'agent qui développe les scripts.
Implémentation : crate Rust `lifescape-gamedata` (`rs/crates/gamedata/`),
stdlib uniquement, zéro dépendance.

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

- Rust lisible par un humain, édition 2021, `#![forbid(unsafe_code)]`.
  Zéro dépendance externe : uniquement la std. Pas de serde : le mini
  lecteur JSON de `lib.rs` suffit.
- Un rôle par module (`banks.rs`, `combat_spots.rs`, `equipment.rs`,
  `fishing.rs`, `food.rs`, `quests.rs`, `quest_requirements.rs`,
  `recipes.rs`, `rocks.rs`, `shops.rs`, `spawns.rs`, `spells.rs`,
  `trees.rs`, `varps.rs` + types `Obj`/`Npc`/`Loc` dans `lib.rs`),
  un fichier < 1000 lignes, découper avant la limite.
- Noms explicites (jamais 1-2 caractères), petites fonctions, mêmes noms
  snake_case que le Python d'origine (`all_banks`, `nearest_bank`,
  `best_spot`, `tier_for_level`, ...).
- Données : `data/*.json` générés par dump, JAMAIS édités à la main.
  Seul le code dans `rs/crates/gamedata/src/` est écrit à la main.
- Timeouts et sleeps calibrés sur des durées mesurées en live
  (voir `../AGENTS.md` et `../../lostcity/AGENTS.md`).

## Régénérer les données

```sh
cd ../../lostcity/engine && npx tsx tools/gamedata-dump.mts ../../LifeScape/gamedata/data/
```

Les JSON restent lus depuis `gamedata/data/*.json` via `table()` :
aucun changement de chemin côté Rust.

## Vérifier (boucle de vérification)

```sh
cargo test --manifest-path rs/Cargo.toml -p lifescape-gamedata
cargo build --manifest-path rs/Cargo.toml
```

Chaque module rejoue 2–3 vecteurs connus (banques, stuff, nourritures,
quêtes, recettes, shops, spawns, sorts, varps...). Tant qu'un contrôle
échoue, corriger le dump ou le code, puis relancer.
