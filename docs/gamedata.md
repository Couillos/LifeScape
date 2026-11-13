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
Index d'obtention (`lifescape-gamedata::obtain`, PRD3) : pour chaque
objet F2P, toutes ses voies (fabrication, récolte, drop avec taux et
niveau du monstre, sol, boutique, dialogue, quête, sort, mélange, reste)
avec lieux F2P, prérequis, coût ; `producible_f2p` / `merchant_buy_allowed`
(D-A) ; raisons des inobtenables. CLI : `orchestrator obtain <objet>…`,
`--shops` (inventaire marchand + verdict), `--unobtainable`. Boutiques
F2P = ouvertes par un handler évalué en monde F2P (`shop_access`), stock
members exclu (`f2p_stock`). Équipement : prérequis complets
(`requirements`, `quest`, `tier`, `rangebonus`) ; meilleur set par style
et meilleurs outils : `gear::best_gear`, `gear::best_tool`. Nouveaux
dumps (`dump_plan.mts`) : `drops.json` (+ `rate`, `qty_min/max`,
`exact`), `shop_access.json`, `dialogue_buys.json`, `script_grants.json`,
`config_recipes.json` ; vérification manuelle des taux :
`node gamedata/tools/drop_rates_check.mts <content/scripts> <sujet>…`.
Table d'XP (`lifescape-gamedata::xp`, unités serveur = 10× l'affichage :
niveau 2 à 830, 99 à 130344310).
Apparence (`lifescape-gamedata::player_design`) : kits idk activés par
genre et partie (`idk.pack` + `{man,woman}.idk`, mâchoire femme = -1),
nombre de couleurs (`Player.DESIGN_BODY_COLORS`), tirage déterministe
par nom de compte ; le test `tables_match_the_server_content` relit
`../lostcity/content`. Envoi : `Bot::save_player_design` (SDK), appelé
par l'écran de design du tutoriel (`scripts/src/tutorial_design.rs`).

Stands clients des guichets (`lifescape-gamedata::bank_stands`,
`gamedata/data/bank_stands.json`, 2026-11-13) : par banque F2P (7 en
rev 274 : Al Kharid, Draynor, Edgeville, Falador est/ouest, Varrock
est/ouest ; aucun guichet au château de Lumbridge), une case extérieure
publique ; par guichet (32), les stands de la règle de portée serveur
séparés en `client` (planifiés depuis l'extérieur) et `teller` (côté
banquier, poche fermée). Seule table de ce dossier écrite à la main :
`rs/crates/nav/tests/bank_stands.rs` la recalcule depuis la carte
(< 1 s) et imprime la table à recoller en cas d'écart. Branchée au SDK
par `Tables::bank_client_stands` (`scripts/src/tables_live.rs`).

Boucle de vérification : `cargo test --manifest-path rs/Cargo.toml -p lifescape-gamedata`.
