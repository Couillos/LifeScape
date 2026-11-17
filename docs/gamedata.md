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
members exclu (`f2p_stock`). Une ligne à stock 0 (`.inv`, Grum, Drogo,
Gerrant…) ne contient que ce que les joueurs revendent au PNJ : aucun bot
ne vend aux PNJ, donc jamais une source d'achat ni un prix marchand
(`shop_sources`, `merchant_price`, 2026-11-16 ; `shops.json` recorrigé :
103 lignes lues 1 au lieu de 0). Zones fermées (`access::areas`) :
`Gate::Quest` pour une porte de quête jamais ouverte par le planner
(placard du manoir, arrière-boutique de Wydin), enclos d'argent et salle
nord (four, moule d'amulette) de la Crafting Guild, rangée sud de la
Champions' Guild (Scavvo, étage : `Gate::QuestPoints` 32, une voie boutique
qui porte `Way::quest_points`, Q7 2026-11-16, voir `docs/planner.md`
§ Boutiques rares) ; `Gate::Sealed` pour des cases que nul
n'atteint (présentoir du musée de Varrock : épée, jambières et plastron
en fer, jamais ramassables). Wilderness ouverte à tous, toujours
(utilisateur 2026-11-16) : plus de `Gate::Wilderness`, plus de `NEVER`
de zone (`danger::site_requirement`, `melee_ladder`) ; ses chasseurs
comptent comme les autres (`passive_from` = 2 × niveau + 1, ils attaquent
toujours mais ce niveau les bat) ; seule la règle commune de distance à
la banque (`BANK_TREK_LIMIT`) écarte le nord profond. Karamja :
boutiques accessibles au planner (`shop_site::pick_shop`, aller-retour
du bateau budgété, `boat` au lancement, `market shopbuy … boat` arme les
fares), en plus de la récolte et des combats déjà routés. Équipement : prérequis complets
(`requirements`, `quest`, `tier`, `rangebonus`) ; meilleur set par style
et meilleurs outils : `gear::best_gear`, `gear::best_tool`. Nouveaux
dumps (`dump_plan.mts`) : `drops.json` (+ `rate`, `qty_min/max`,
`exact`), `shop_access.json`, `dialogue_buys.json`, `script_grants.json`,
`config_recipes.json` ; vérification manuelle des taux :
`node gamedata/tools/drop_rates_check.mts <content/scripts> <sujet>…`.
Dumps hors ligne de l'orchestrateur (`dump-npc-ranges`, ou tests
`--ignored` `write_action_odds` / `write_shop_haggle`, lecture seule de
`../lostcity/content/scripts`) : `action_odds.json` (`gamedata::
action_odds` : roches, pioches, arbres, poissons, cuisson, runes par
essence, repousses, et `mining_gems` : jet de gemme 1/`roll` d'une roche
et `mining_gem_table` de `mining.rs2`, parts sur `draws`) ;
`shop_haggle.json` (`shop_delta` des gardiens `.npc`, défaut de
`shopkeeper.param`, haggle des `~openshop`) lu par `gamedata::shop_buy` :
fenêtre d'achat d'une boutique F2P = stock de base unité par unité au
prix serveur du stock restant + réassort (1 unité / `rate` ticks).
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

Région du péage d'Al Kharid (`lifescape-gamedata::toll`,
`gamedata/data/toll_region.json`, 2026-11-14) : le côté Al Kharid de la
clôture en bandes par rangée `[z, x0, x1, …]` plus la fenêtre de
dérivation (`behind`, `toll_free_side`, `across`, `crossing`). Dérivée
de la carte servie par `lifescape_nav::toll_side::derive_region` ;
`rs/crates/nav/tests/toll_region.rs` la recalcule et échoue à l'écart,
`LIFESCAPE_BLESS_TOLL_REGION=1` réécrit le fichier (voir
`docs/navigation.md` § Péage d'Al Kharid).

Forge (`recipes.json`, kind `smith`, 2026-11-14) : XP = `bar_amount ×
xpperbar` dixièmes (`smithing.struct`, payé par `smithing_anvil`), le
dbrow n'a pas de colonne XP : le dump écrivait 0 pour les 129 lignes
(corrigé dans `tools/gamedata-dump.mts`, test
`smith_rows_match_the_server_tables` qui relit dbrow + struct : niveau,
barres, unités produites (clous 2), XP).

Défense distance et magie (R3.3, 2026-11-17) : `npc_combat.json` porte
aussi `magic` (niveau, défaut moteur 1), `rangedefence` et `magicdefence`
(`npc_defence_roll_specific`), lus par `NpcCombat::{magic,
ranged_defence, magic_defence}` : le prior des combats à distance et de
magie (`planner::method_rate_fight`) en tire les chances de toucher.

Cibles de sorts (D18, 2026-11-16) : `orchestrator dump-npc-ranges` écrit
aussi `spell_targets.json` (sort → param PNJ exigé, lu dans les procs
`apnpct` du serveur : `crumble_undead` → `undead`) et ajoute
`"undead": true` aux PNJ de `npc_combat.json` dont la config porte
`param=undead,^true` (48). Accès : `spells::target_param`,
`spells::affects(sort, pnj)`, `npc_combat::has_param`,
`Spell::deals_damage` (`spellcom` + `maxhit`) et `Spell::npc_debuff`
(`spellcom` sans `maxhit` : confuse, weaken, curse, bind). Le `name` de
`spells.json` est l'affichage du dbrow : crumble undead n'en a pas côté
serveur, il ne sert plus à trier les sorts de combat. Catalogue de
combat : `wild_skeletons` (squelettes de surface, Wilderness basse), le
camp mort-vivant qu'un run de magie atteint à pied.

Boucle de vérification : `cargo test --release --manifest-path rs/Cargo.toml -p lifescape-gamedata`.

## Portes d'accès (`access.rs`, 2026-11-13)

`gates_at(x, z, niveau)` : portes d'un site (guildes artisanat 40 + tablier
brun, cuisiniers 32 + toque, mineurs 60 ; clé de laiton d'Edgeville ;
bateau de Karamja 30 po ; Crandor ; souterrain ; Wilderness), portes lues
dans `content/maps` (`crafting_guild_door` (2933,3289), `chefdoor`
(3143,3444), `brasskeydoor` (3115,3450), échelle de la guilde des mineurs
(3019,3339)). `surface_of` : tuile où se lisent banque et danger d'un site
(porte de guilde, rez-de-chaussée d'un étage sans banque). `impassable` :
bateau tant que `BOAT_ROUTED` est faux (la nav n'a pas d'arête pour les
marins), souterrain, Wilderness. Les voies de recette rangent moules et
talismans dans `Way.tools` (portés, jamais consommés), les voies de sort
portent leurs runes, les teintures d'Aggie leurs intrants (`aggie.rs2`).
`quest_requirements` porte `reward_points` (`^<clé>_questpoints`) et
`blocker` (quête impossible en solo aujourd'hui).
