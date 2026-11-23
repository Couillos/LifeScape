# Planificateur et cerveau (L7 livré)

But : transformer un but long terme en graphe de tâches exécutables,
sans toucher au socle une fois posé. Implémenté en `rs/crates/planner/`
(pur : aucun réseau, aucun fichier, aucune lecture d'un autre bot) et
`rs/crates/orchestrator/src/brain.rs` (daemon : rapports → replanning).

## Choix

Réseau de tâches hiérarchique (HTN) avec sélection de méthode au coût :
prévisible au dashboard, simple à implémenter, adapté au F2P (recettes
« pour avoir X, faire A puis B »).

## Nœuds

`reach_level`, `obtain_items`, `have_coins`, `make_money`, `complete_quest`,
`equip_best`, `access_area`, `space_cleanup`. Statuts : `pending` →
`ready` → `active` → `done` | `blocked{reason}` | `failed{error}` |
`abandoned`. Fichier : `run/lives/<compte>.json`, écriture atomique
(`paths::write_json`), schéma v1.

Retraite des nœuds finis (demande utilisateur 2026-11-14,
`planner/src/lifecycle_retire.rs`, `life_memory.rs`) : le graphe ne porte
que le travail vivant, sa taille suit le but courant, jamais l'historique.
Un nœud `done` quitte le graphe ET les listes `deps` (dépendance faite =
satisfaite). Un nœud `abandoned` reste tant qu'il sert le but courant
(numéro ≥ `memory.floor`, posé par `append_goal`) : la période de blocage
le lit (rachat au marché, cause et portée du bannissement, relance) ; dès
qu'un but suivant est tiré, il part avec sa cascade. Exceptions : un
abandonné encore listé par un nœud gardé (nœud actif) reste avec lui ; le
tutoriel abandonné sur l'île reste tant qu'il n'est pas prouvé (l'opérateur
décide). La retraite tourne dans `brain::save_life` (sur une copie : les
index du passage en cours restent valides) et au premier chargement d'un
ancien fichier (`read_life_file`, une ligne `life loaded: N done and M
abandoned nodes retired …`, plancher déduit : nœuds ouverts + racine(s)
du but courant + leurs dépendances). Clé `memory` du fichier (bornée) :
`last_number` (plus haut numéro jamais émis : un id retraité n'est jamais
réutilisé, `lifecycle::max_id_number` ; un rapport tardif pour lui est
loggé `report for retired node nX, ignored`), `floor`, `quests_done`
(quêtes prouvées par le graphe, tutoriel compris ; un blocage
`missing:quest:<clé>` les réfute), `done_total`/`abandoned_total`, et
`recent` : les `RECENT_CAP` = 40 derniers nœuds retraités `{id, kind,
params, outcome, last_error}` (preuve de la racine du but
`goal_root_done`, série des concierges `space::janitor_stalled`,
dashboard). Historique compacté au-delà de `HISTORY_COMPACT_AT` = 300
entrées : les 200 dernières, plus les 60 dernières entrées `goal`, la
dernière entrée de bannissement/levée par compétence, la dernière clôture
de but, la dernière entrée hors `goal` (période), et la dernière entrée
par (nœud vivant, issue) (`awaiting_fill`, cause d'abandon) : chaque règle
« dernière entrée » lit la même réponse.

Mesure (2026-11-14, copie de `run/lives`, bench ignoré
`brain::retire_tests::life_retire_bench`, `LIFESCAPE_BENCH_LIVES=<copie>`) :
bbkjz2vk 802 Ko → 46 Ko (666 → 2 nœuds, 4539 → 238 entrées), chargement
25 → 1 ms CPU, écriture 21 → 1 ms ; bb4v5d9m 719 → 45 Ko (715 → 1 nœud),
25 → 0,8 ms ; bst29bh9 222 → 32 Ko (206 → 1), 9 → 0,8 ms ; george_97
264 → 33 Ko (246 → 2). Toutes les vies longues tiennent entre 25 et 47 Ko
(l'historique borné et l'ambition dominent).

## Batterie de tâches (règle D21)

Le brain fixe objectif (XP, niveau, quantité) et ressource exacte ; le
script trouve seulement *où* l'exploiter (choix du spot). Squelette
complet à la création (`expand_goal`), feuilles concrètes résolues au
plus tard (`resolve_ready`, prix et rendements du moment). Table
`méthode → (script, args)` dans `planner/src/mapping.rs`, jamais en JSON.
Les gatherers travaillent le meilleur palier à ou sous leur niveau live
dans la tranche fixée (pas de positionnel ressource : `woodcut`/`mining`/
`fishing` la liraient comme un compte).

Paliers : pêche [5,10,20,25,35,40], bûcheronnage [15,30,45,60,75], minage
[10,15,20,30,40,55], cuisine [10,20,30,45], mêlée [10,20,30,40], prière
[10,20,30,40] (enterrer en banque : `bury:bones` puis `bury:big_bones` à
20), magie [10,20,21,25,30,…,95] (sous 21 le meilleur sort de combat
ouvert au départ : `fight:magic/<sort>/<spot>` ; dès 21 tranche au prix,
voir « Entraînement au prix »), forge aux déblocages d'enclume et de
fonte (5, 9, 15, 18, 20, 24, 30, 33, 35, 39, 40, 48, 50…, tirés du dump :
`training::smithing_cuts`). Chaque
tranche porte la méthode du palier ouvert à son niveau de DÉPART (1→5
crevettes, 5→10 sardines ; jamais le homard pour 35→40) : le contrôle
d'outils du planner porte sur ce que le bot pêchera vraiment. Pas de
méthode de substitution : outil manquant ⇒ `Blocked{missing}` ⇒
sous-tâche. Ressources par palier (2026-11-13, contenu serveur) : pêche
au-delà de 30 = saumon (thon et homard sont à Karamja : bateau payant,
voir « Karamja » ; les paliers restent sur le continent, une obtention
`farm:raw_lobster` part en bateau), bûcheronnage 45 = saules (l'érable n'a
aucun site F2P), cuisine saumon dès 25 et au-delà (homard, thon, espadon
crus = eau de Karamja, par obtention seulement),
artisanat aux noms d'objets (`unstrung_gold_amulet`, `nostringstar`,
`unstrung_diamond_amulet`).

Outillage (`tools::required_tool`, 2026-11-13) : dérivé des voies serveur
(`method_way::method_way` → `Way.tools`, plus aucune liste à la main) :
`ToolNeed { tools, consumables }`. Récolte : pioche, hache, filet/canne +
appât (y compris `farm:`) ; forge ⇒ marteau ; allumage ⇒ briquet ;
bijouterie ⇒ moule (outil porté, jamais un intrant) ; runes ⇒ talisman ;
gemmes ⇒ ciseau ; cuir ⇒ aiguille + fil (consommé) ; traite ⇒ seau
(consommé) ; distance ⇒ arc + flèches ; magie ⇒ runes du sort de la
méthode (un bâton élémentaire vaut sa rune). Un outil non DÉTENU (sac,
banque, porté, hors annonce de vente du bot) ⇒ `Blocked{missing:<outil>}`
au résolveur ⇒ obtention (magasin, marché) AVANT le lancement : aucun
exécutant n'achète son équipement (commit 44bde31), un outil « finançable »
n'est plus compté comme tenu (`tools::gear_ok`, 2026-11-13 : `fishing
raw_tuna` partait sans harpon avec 500 po et bloquait aussitôt).

Dimensionnement (`sizing.rs`, 2026-11-13) : une tranche coûte
`ceil((XP(cible) − XP(départ)) / XP par action)` actions (tables serveur,
dixièmes) ; ses consommations = intrants de la voie par action ÷ chances
de réussite (fer : 2 minerais par barre), appât et plumes par prise, une
flèche par tir (4 XP par point de dégât, modèle : 60 % de touches à la
moitié du coup max ; le script ne ramasse pas ses flèches), runes du sort
par lancer (XP du sort + 2 par point de dégât), un fil pour 5 coutures.
`need_count_from` dimensionne un blocage de tranche sur le reste de la
tranche depuis le niveau live (jamais 1 ni 27), borné par le lot du nœud
(`node.batch`, alchimie : ce que le lot lance). Forge à l'enclume : XP par
barre du serveur (`smithing.struct` `xpperbar` : bronze 12,5, fer 25, acier
37,5, mithril 50, adamant 62,5, rune 75 ; le dump n'en porte pas,
`gamedata::recipes::smith_xp_per_bar`), une plaque vaut 5 barres.
Alchimie : un objet, une nature et 3 (basse) ou 5 (haute) feux par lancer.

## Entraînement par le guide et les traits (R6, 2026-11-17)

Remplace, côté cerveau, le choix au prix décrit à la section suivante.
`orchestrator/src/brain_train.rs` + `planner/src/train_score.rs` (pur).
Une tranche dont la méthode se choisit à la résolution
(`training::priced_at_resolve` : forge, magie dès confuse, cuisine,
artisanat, feu, prière) lit le GUIDE D'ENTRAÎNEMENT PRÉCALCULÉ
(`guides_store::current()`, `Arc` partagé du dernier cycle, jamais une
construction à la demande) :
- lignes du bot : `GuideSet::training` filtré par lecture (niveau live,
  quêtes faites, compétence), seules celles qu'une montée lance
  (`mapping::map_method` ; un raccourci de quête reste un nœud de quête) ;
- score d'une ligne (plus bas gagne) = ticks propres par XP (6000 / XP
  par 6000 de la ligne) + `coin_weight(thrift)` × pièces par XP
  converties en ticks au D17 du bot (6000 / profit de sa meilleure ligne
  money making accessible, lue dans la vue money de sa tranche) ;
  `coin_weight = 0,5 + thrift` (rapide 0,5 ↔ économe 1,5 ; 1 = total D17
  neutre) ; une ligne MARCHÉ pèse ×`buy_weight(market_affinity)` =
  2^(2 − 4 × affinité) (levier utilisateur 2026-11-17 « entraînement
  marché plus fréquent » : courbe continue, ×1 à 0,5, ×0,76 à 0,6, ×0,44
  à 0,8, ×3 à 0,1 ; remplace 1,5 − affinité, que gardent les lignes
  market du guide money et `cost::choose_market_or_farm`) : un profil
  marché achète ses intrants tant que leurs pièces au D17 ne rendent pas
  l'achat bien plus lent, un D17 élevé (meilleure ligne d'argent
  pauvre) rend la ligne autonome ; une ligne
  bloquée, inaccessible, sans XP, ou marché pour un Iron, n'a pas de
  score ;
- gain compté zéro (2026-11-20, « le smithing avance si lentement ») :
  les pièces qu'une ligne RAPPORTE (produits vendus au-delà de ses
  achats) ne baissent jamais son score sous son temps : terme pièces =
  max(0, pièces par XP converties), tout score ≥ 0. Une montée se choisit
  pour son XP ; l'argent est l'affaire du choix d'argent. Constat live :
  lara_lane forge 60 -> 68 et bellarhodes 59 -> 66 prenaient le bronze
  extrait et fondu (0,09 XP/tick, +1,6 pièce/tick créditées au taux
  mesuré du bot, 0,67 pièce/tick : score −5,5) devant les plastrons
  d'acier de barres achetées (3,7 XP/tick, −1,3) ; 40 060 minerais de
  cuivre déposés pour une tranche. Mesure de la flotte : ~0,1 XP de forge
  par tick d'un forgeron ;
- magie et distance : xp/coût à l'entraînement (utilisateur 2026-11-20,
  « si le bot veut s'entraîner alors il faut privilégier l'xp/coût et
  s'il veut combattre alors le meilleur sort dispo avec les runes
  dispo ») : un COMBAT d'entraînement (`fight:`) paie ses runes et ses
  flèches quoi que rapporte son butin (terme pièces = max(dépense nette,
  runes + flèches au prix d'achat de la ligne),
  `train_score::fight_consumables_per_xp`) ; alchimie, superheat,
  enchantements gardent leur bilan (leurs pièces sont leur produit). Le
  prix d'achat d'une rune est ce qu'elle coûte MAINTENANT : asks, puis
  marchand en stock, puis prix courant ; un marchand que le marché
  prouve vide (objet échangé au-dessus de son prix le plus cher,
  `planner::shop_drain::keepers_sold_out`) ne compte aucune unité dans
  les guides (achat et disponibilité) ; l'obtention, elle, essaie encore
  le marchand (le script voit la vraie étagère : un marché cher n'y
  prouve rien, live : 179 flèches d'acier achetées chez Lowe quand le
  marché les échangeait à ~300). Une rune que personne ne vend (asks + marchands + tenue <
  lot) rend la ligne infaisable, jamais gratuite. Constat live : chaque
  mage montait par crumble undead sur les squelettes de la Wilderness
  (2,3 XP/tick, butin > runes au prix d'Aubury 15) quand les runes du
  chaos s'échangeaient ~2 000 (2 sur les asks, Aubury et Betty vidés) :
  « out of runes for crumble_undead … chaosrune » 79 fois par jour, temps
  de magie de la flotte ~6 % → 0,3 %. Le combat (drops, argent, kill,
  tout nœud autre que `reach_level`/`reach_combat`) lance `magic … --spell
  <sort> --best-spell` : le script lance le sort le plus fort que ses
  runes paient (dégât max puis niveau, cible morte-vivante pour crumble),
  descend l'échelle quand elles manquent, banque si seule la banque en
  paie un, bloqué sur les runes du dernier sort sinon
  (`scripts::kit::strike::best_cast`) ; une montée lance son sort exact.
  Un run de magie n'ouvre plus jamais par le clic Attaque (`kit::foes::
  attack_nearest`) : il lance son sort (live : ida_reed, 92 XP d'attaque
  et 0 de magie sous crumble undead, runes restées en banque). Distance :
  le kit tire déjà les meilleures flèches que l'arc accepte et passe aux
  suivantes (`kit::gear::refill_quiver`) ; le choix de la flèche à
  l'entraînement reste celui du kit ;
- choix : le tirage R7 (section suivante, `train_score::pick`) sur les
  mêmes lignes classées (`best` = tête de l'ordre du score seul) ;
  consommables dont le bot tient moins d'une action
  (`training::missing_inputs`, tirés du rendement) à obtenir d'abord ;
  verrouillé par `lock_training_pick` comme avant ; une ligne MARCHÉ
  marque le nœud (`TrainPick::buys_inputs` → `Node::buys_inputs`, levier
  utilisateur 2026-11-17) : les obtentions que ses blocages déposent pour
  ses intrants (pas ses outils) n'ont que `market:buy`, au volume de la
  tranche (`replan::buys_by_market`, bid `need` borné par le
  portefeuille ; economy.md § Montées en mode marché) ; la ligne marché
  du guide compte un trajet et une attente par tranche achetée
  (`guide_training::band_slots`), plus par sac ;
- aucun guide publié (premier cycle) ou aucune ligne jouable : aucun
  choix, loggé, méthode statique de la tranche.
Log : `brain <compte>: nX firemaking 60->65 trains by fire:yew_logs
(market, score … = … ticks/XP + … coin ticks/XP, thrift …, affinity …,
…)`. Exemples testés (`train_score_tests`, `brain_train_tests`) : feu
d'if, un bot « marché » achète les bûches, un autarcique les coupe ;
ligne autonome bloquée (pioche rune que personne ne forge) : tous
achètent ; deux lignes marché, rapide = la chère et rapide, économe =
la lente et bon marché.

Trait `thrift` (D13, `Profile::thrift`, rapide 0 ↔ économe 1) : tiré de
la graine (`model::draw_thrift`, splitmix64 de graine ⊕ sel 103,
`Profile::new` le tire toujours) ; écrit dans la vie (`profile.thrift`) ;
une vie écrite avant le trait reçoit le MÊME tirage au chargement
(stable, sauvé à l'écriture suivante), une valeur stockée est gardée.

`training::best_pick` (prix à la demande, section suivante) n'est plus
appelé par le cerveau ; il reste au planificateur, ses temps lus du
rendement (R3.3).

## Choix des bots dans les guides (R7, 2026-11-17)

Une seule règle pour tout choix lu dans les guides précalculés
(`guides_store::current()`, asks du cycle, jamais de calcul à la demande)
— argent (make_money, have_coins, financement d'un outil) et TOUTE montée
(récolte, combat, recettes, sorts) ; `planner/src/guide_pick.rs` (pur),
décision datée dans `docs/decisions.md`.
- Candidats : lignes du guide de la tranche du bot jugées par
  `guide_feasible::feasible_for` sur son état LIVE (niveaux, combat,
  quêtes, stock libre, bourse libre, asks du cycle). Classes, la première
  non vide tire : 0 faisable maintenant (argent : `food_gate::ready_now`
  aussi, premier critère gardé), 1 après un prérequis obtenu seul (pièces
  d'un lot, nourriture), 2 après un outil à obtenir (le nœud bloque sur
  lui) ; jamais : `sick()` (quarantaine, refroidissement), `excluded`
  (cycle), quête manquante, combat hors portée.
- Tirage (D10) : bande = lignes à ≤ 15 % du meilleur score
  (`BAND_SHARE`) ; poids = score × profil (`merit_money` : profit, ligne
  marché ÷ (1,5 − market_affinity) ; `merit_training` : 1 / score
  `train_score`, thrift et affinité déjà dedans).
- Exploration (D14) : les lignes hors bande à < 3 runs mesurés
  (`MEASURED_RUNS`) et de poids positif se partagent 5 % des tirages
  (`EXPLORE_SHARE`, au total), uniformément.
- Graine : `seed_for(profil.seed, nœud, LifeMemory::draws)` ; le
  compteur avance à chaque choix enregistré. Collant : le choix précédent
  d'un nœud bloqué sur son outil (`blocked_by`) revient dès qu'il est
  faisable ET dans la bande du meilleur score faisable maintenant
  (2026-11-20 : un choix que le score classe désormais loin derrière est
  retiré au sort) ; un lot trade acheté est crafté d'abord (`trade_craft::
  sticky`).
- Argent : `orchestrator::brain_money::publish` lit les lignes de la vue
  de chaque niveau du bot (compétence de bande de l'activité), recettes
  réévaluées sur SON stock (`GuideFilter::held`, moins ce qu'il garde, ses
  propres candidats exceptés : `brain_alch::kept_except`), seulement celles
  dont il détient chaque intrant ; mappe chaque ligne vers la méthode
  lancée (farm : sa méthode si un nœud argent la lance ; market :
  `trade:craft/<recette>` quand le bot a l'offre ; recette sur stock :
  `alch:` et TOUTE transformation que son script lance en lot, suite R7
  2026-11-17 ; chaîne : jamais), cotée et rentable, une par méthode →
  `ReadyCtx::money_guide` ; `planner::money_pick::resolve_from_guide`
  tire. Aucun guide ou aucune ligne tirable : classement statique d'avant
  R7, loggé.
- Demande partagée (plan équilibre étape 2, 2026-11-19) : le profit
  d'une ligne pour ce bot est relu AU CHOIX sur les réclamations des
  autres à l'instant (`planner::guide_share::SharedDemand` : chaque
  produit de la ligne revalorisé le long de l'échelle de vente partagée,
  `guide_engine::sale_ladder` : bids et volume récent moins annonces au
  repos au prix et réclamations ; une ligne qui ne gagne plus disparaît).
  Le run lancé publie ses unités au tableau des commandes
  (`brain_money::claim_launch`, docs/economy.md § Tableau des commandes),
  libérées à son rapport : le bot suivant lit ce qui reste.
- Recette sur stock (suite R7) : `planner::stock_lot` (pur) :
  `stock_inputs` (intrants de stock d'une action : recette, sort ou
  service sur un objet, jamais une rune, une récolte ni un combat ;
  `None` pour un sort compté en lancers : alchimie, téléports, bananes,
  malédictions, et pour un lot trade), `lot_of` (actions que le stock
  libre couvre, l'intrant le plus rare décide : 200 truites crues → 200,
  200 cuivre + 50 étain → 50), `uses` (les buts `--use` d'un lot).
  `GuideLine::lot` → `node.batch` au tirage ; `mapping` ajoute
  `--use <intrant> N` à un nœud argent (jamais à un lot trade, qui porte
  `--want`) ; le script finit `goal_reached` à N consommés (brûlés et fers
  ratés compris : l'intrant compte) ou quand un pas bloque alors qu'un
  intrant du lot est épuisé (`scripts::kit::stock_use`, `cycle::run`,
  `spell_load::run_casts`, tannage borné par le lot) ; bloqué avant toute
  consommation : le blocage reste (aucun stock, le jugement du cerveau
  était faux). Familles : cuisson, repas, pain, fonte, forge, filage,
  tannage, couture, poterie, taille, bijoux, enfilage, runes, remplissage,
  teinture, traite, superheat, enchantement.
- `reach_combat` (suite R7) : le combat se tire du guide d'entraînement
  comme une montée (`brain_train::fill_ctx`) : compétence du style
  (distance, magie), mêlée la plus basse d'attaque, force, défense
  (`combat_climb_skill`, attaque à égalité), tranche de
  `COMBAT_TRANCHE_LEVELS` (5) niveaux pour ses consommables ;
  `expand::lock_training_pick` verrouille aussi un `reach_combat` ; sans
  choix : la méthode statique (`combat::training_method`).
- Réponse aux bids (suite R7) : `bid_response::Producer::rate` = lecteur
  mesuré du planificateur (`brain_plan_read::measured_rate`, mémoïsé pour
  la passe dans `brain_bids::with_bot_state`) : l'entraînement d'un lot
  (`training_rows` : actions, intrants brûlés) via `sizing::*_with` ; les
  unités/6000 d'une jambe lisent déjà l'estimateur (`rates_read`, n ≥ 3
  mélangé au prior, outil du bot). Le taux de pièces du bot est son gain
  réel mesuré par tick de travail (`brain_d17::coin_rate`, D1 du plan
  équilibre, 2026-11-19 ; son D17 dans le guide money du 2026-11-17 au
  2026-11-19, la table `rates_profit` avant) ; `own_use_items` ne lit que des
  NOMS d'intrants (la mesure change les quantités, pas l'ensemble) : prior
  gardé.
- Trait rapide ↔ économe : ne pèse PAS sur l'argent (décision utilisateur
  2026-11-17, pas tant que la mesure live ne montre pas de manque) ;
  seulement sur l'entraînement (`train_score`).
- Niveau d'une ligne de bande : la condition de sa compétence est la
  PORTE de l'activité (`guide_prereq::prereqs_of`), jamais le dernier
  niveau de la bande (correctif live 2026-11-17).
- Entraînement : `brain_train::fill_ctx` pour TOUTE montée (plus
  seulement `priced_at_resolve`) ; une ligne de mêlée seulement dans le
  style de la compétence de la montée (PV : toutes) ; lignes `stock` du
  stock propre (alchimie de ses objets, lot = son stock) ; consommables
  manquants lus de la mesure (`missing_inputs_with`).
- Lecteurs du planificateur (`plan_read::PlanRead`, variantes `*_with`) :
  rendement MESURÉ (`brain_plan_read::measured_rate`, prior sous 3 runs)
  pour la taille des obtentions d'un blocage, du squelette et la durée
  des tranches ; oracle `guide_trains` : une tranche que le guide
  choisira ne dépose rien de sa méthode statique.
- Mesure : log `brain <compte>: nX money|training pick <méthode> (score
  …, class …, k in band of n, exploration, previous pick again)`,
  mémoire `picks` (40) et `draws`, section « bot choices »
  d'`orchestrator economy`.
- CPU (bench ignoré `brain_picks::bench::brain_choice_bench`) : décision
  d'entraînement 1,9 ms (R6 1,7), d'argent 2,6 ms (statique 1,7), montée
  de magie ~20 ms, oracle du squelette 26 ms pour 15 tranches.

## XP et temps dérivés des rendements (R3.3, 2026-11-17)

`planner/src/work_rate.rs` : un `WorkRate` par (méthode, niveau) =
actions, XP par compétence (dérivée des unités, `unit_xp`) et flux signés
par 6000 ticks ; lu de la mesure (lecteur passé par l'appelant) ou du
prior du jeu (`work_rate::prior_rate`, mémoïsé). Tout en découle :
ticks par XP = 6000 / XP par 6000 ; XP par action = XP / actions ;
consommables par XP = |flux| / XP (intrant, rune, munition, appât,
fourniture). Constantes supprimées : presse 5, fonte 5, sort de combat 7,
`BANK_TRIP_TICKS` 60, `LOAD_SLOTS` 27 (`training`), feu/cuisson/... par
famille (`training_production::action_ticks`), téléport 5, malédiction
25, bananes 3 + 60 (`training_spells`), `HIT_ODDS` 0,6 (`sizing`),
`ticks_each`/`xp_each` des étapes de chaîne (`chains`, D58).
- `training` : une option = `rated_option` (temps, intrants, produits,
  péages par XP du rendement) ; `TrainPrices::rate` fournit le
  rendement (sans rendement, méthode non cotée) ;
- `sizing` : XP par action, durée (`tranche_ticks`) et consommation d'une
  tranche sommées NIVEAU PAR NIVEAU (les chances montent), arrondies à
  l'action entière ; variantes `*_with(lecteur)` pour un appelant mesuré ;
  le reste du planificateur lit le prior ;
- tranches `ReachLevel` : `est_ticks` de l'alternative = `tranche_ticks`
  (rendement), plus 0 → 6000 ;
- chaînes : `eval_chain_with(lecteur)` (`eval_chain` = prior) ; une étape
  sans rendement (moulin, méthode sans voie) laisse la chaîne non cotée
  (`ChainEval::unrated`, profit 0) ; la chaîne `bury` = combat puis
  enterrement (le prior d'un combat ne paie pas de prière) ;
- combat distance et magie (prior, `method_rate_fight`) : jets du
  serveur au lieu de `HIT_ODDS` : attaque = niveau effectif × (bonus +
  64) (distance : niveau + 8 + 3 en précis ; magie : niveau + 8 + 1),
  défense = (défense PNJ + 9) × (`rangedefence` + 64) ou (magie PNJ + 9)
  × (`magicdefence` + 64), dump `npc_combat.json` (clés `magic`,
  `rangedefence`, `magicdefence` ajoutées à `dump-npc-ranges`).
Constantes restantes, toutes du serveur : cadences d'action
(`method_rate`), bonus de style (+3 précis, +1 magie), force d'une flèche
de bronze (7) quand l'équipement de la clé n'en nomme pas.

## Entraînement au prix (2026-11-14)

`planner/src/training.rs` (pur) + `orchestrator/src/brain_train.rs`
(remplacé côté cerveau par le guide, R6 ci-dessus).
Forge et magie (dès 21) choisissent leur méthode AU LANCEMENT avec le
marché, comme un joueur : fondre puis forger ce qui paie, alcher plutôt
que bombarder des vaches quand c'est moins cher par XP.

- Score d'une méthode = pièces par XP : `(ticks par action × taux du bot
  + intrants − valeur du produit) / XP par action`. Ticks : frappe 5,
  fonte 5, lancer 5, sort de combat 7, plus un aller-retour banque de 60
  ticks partagé par la charge (27 cases). Intrants : unités détenues à ce
  que leur vente rapporte (meilleur bid / prix courant ; Iron : 0), le
  reste au coût d'achat ou de fabrication du bot, temps compris
  (`bid_response::unit_cost`), sinon au prix courant (offre au repos),
  sinon magasin si D-A le permet (bâton de feu) ; un bâton élémentaire
  paie sa rune ; une barre que le bot fond lui-même ajoute l'XP de fonte.
  Produit : prix courant × part absorbée par le carnet
  (`saturation_factor` du lot de la tranche), ou net d'alchimie si le bot
  lance et que c'est mieux.
- Forge : chaque produit F2P des quatre colonnes ouvert au niveau live
  (`training::smith_products`, dump ; ni clous, ni fil, ni boulets —
  membres en 274, `cannonballs.rs2`) et chaque barre à fondre (or 40,
  argent 20…). Méthode statique du squelette (sans prix) : métal le plus
  haut ouvert, produit au plus de barres par frappe (dague de bronze,
  cimeterre à 5, marteau de guerre à 9, dague de fer à 15, plastron
  d'acier à 48) ; ses barres sont déposées devant la tranche (marteau une
  fois), et des barres détenues pèsent le choix vers elles. Les barres
  s'obtiennent récursivement (marché, ou fonte avec minerais minés ou
  achetés) ; enclumes et fourneaux sont des sites (pas de fourneau à
  Varrock), le marteau est un outil exigé.
- Magie : sous 21, le sort de combat de la tranche (inchangé, runes
  déposées d'avance). Dès 21, rien n'est déposé d'avance : le choix
  compare le sort de combat (`reach_method` au niveau live) et
  l'alchimie (basse dès 21, haute dès 55) de chaque objet fourni : stock
  propre libre (hors intrants du plan, équipement gardé, annonces) et asks
  d'autres comptes au plus à la valeur d'alchimie. Lot = min(stock ou
  asks, reste de la tranche, ce que la bourse paie), au moins 10 lancers.
  Feu : bâton détenu, sinon runes, sinon bâton acheté quand les runes de
  la montée coûtent plus. Runes de nature : marché seulement (drop F2P de
  nombreux PNJ — gardes, géants, nains, magiciens —, autels de nature
  membres, seules boutiques : Mage Arena en Wilderness et guilde des
  magiciens membres) : sans vendeur, les runes détenues bornent le lot.
- Résolution (`expand::lock_training_pick`) : le choix verrouille sa
  méthode et son lot (`node.batch`) ; outils manquants (marteau) et
  intrants courts bloquent EN UNE FOIS (`missing:hammer,bronze_bar`,
  `missing:naturerune,staff_of_fire`), la méthode gardée pour
  dimensionner les obtentions ; sans marché, aucun choix : méthode
  statique. Lancement alchimie : `alchemy <objet> <lot> --level magic N`
  (compte positionnel du script) ; la fin du lot (`goal_reached` avec XP)
  rend la tranche prête et le choix suivant se refait au prix du moment.
- Ventes : les produits forgés partent par la vente par défaut (ou
  l'alchimie d'un lanceur si mieux) ; une montée de magie au-delà de 21
  garde ses runes de nature, de feu et le bâton ; un objet d'une méthode
  `alch:` ouverte n'est jamais listé.
- `bid_response::training_cost` (entraînement pour une offre) suit la
  méthode statique : forge = frappe, barres au coût unitaire.
- Log : `brain <compte>: nX smithing 18->20 trains by smith:bronze_scimitar
  (… coins/XP, … XP/tick, obtains bronze_bar)`. CPU : un calcul par
  lancement d'une tranche forge/magie, carnet lu une fois, coûts mémorisés
  (phase `train.fill_ctx` du profil).

Dépendances au squelette (`skeleton_deps.rs`) : `expand_goal_from`,
`expand_access`, `expand_quest_from` et le blocage `level:` passent par
`reach_chain_planned` : chaque tranche dépose devant elle un `obtain` par
outil (une fois par chaîne), un `obtain` par objet consommé (taille ci-
dessus), la quête de sa méthode et la guilde quand TOUS ses sites sont
derrière une porte de guilde ; les obtains de la tranche k attendent la
tranche k−1 (juste à temps). Test : la recette de chaque tranche s'ouvre à
son niveau de DÉPART pour toutes les compétences 1..98.

Le contrat d'arguments (table complète ci-dessous, section « Contrat
d'arguments ») est la seule interface brain → scripts : la méthode fixe
script, ressource, site, sort, magasin, quantité et niveau ; le script
exécute, ne choisit rien.

Montée depuis le niveau LIVE (2026-11-13) : `expand_goal_from(goal,
profile, level_of)` part du niveau live (1 s'il est inconnu) et ne génère
jamais de tranche à ou sous ce niveau (attaque 55 à attaque 45 = une seule
tranche 45→55, méthode du palier ouvert à 45 ; niveau déjà tenu = aucun
nœud). `expand_goal` (niveaux 1) ne sert qu'à la création (pas encore de
snapshot) et aux tests ; succession (`append_goal`) et niveau manquant
(`record_block_from`) passent les niveaux live. Au lancement, un nœud
`reach_level`/`reach_combat` que le niveau live atteint déjà est clos sans
lancement (`lifecycle::reached_live` + `complete_node`, historique
`already_reached`). Toute tranche (récolte ET combat : `melee`, `ranged`,
`magic`) part avec `--level <compétence> <N>` : le script s'arrête au but
(contrôle `poll_stop` entre deux spots pour le combat ; un arrêt au sein
de `kit::fight::run_spot` reste à brancher).

## Coûts (ticks équivalents)

`market:buy` = aller-retour place de Varrock + prix × quantité / taux de
gain ; `farm` = quantité / rendement × 6000 + trajets ; `craft` = intrants
+ quantité / rendement. Le trait `market_affinity` pondère market contre
farm (×(1,5 − affinité) : 0,78 à 0,72). Engagement `make_money` : tirage
[6000, 18000] au démarrage du script, dépendances exclues, pauses market
suspendues par le compteur (cycle + session). Replanning sur `blocked`,
`failed`, mort, perte, écart de prix > 25 %, fin d'engagement.

`shop:buy` est exécutable (`market shopbuy`) mais n'existe que pour les
objets non productibles en F2P (D-A, PRD3). `market:make` = poste d'un
teneur de marché (plan marché Q6, vague 3 ; remplace la revente des
marchands, supprimée) : le script `market make --post x,z,niveau` se
tient à sa case de la fontaine de Varrock pendant l'engagement, le démon
cote sa tranche (`orchestrator::market_maker`), sortie sur budget.

## Contrat v2 (TaskSpec / TaskReport)

Inchangé (voir §5.3 du PRD) ; `cook` et `market make` portent désormais
un `TaskStart` (budget + nœud), les jambes d'ordre (`buy`/`sell`) et les
quêtes restent minimales.

## Cerveau

`on_supervise` (un appel par passe) met le compte en file pour le thread
`brain-tick`, qui plie le rapport reçu (`brain_fold.rs`) puis lance le
nœud suivant (`brain.rs::maybe_launch`). Sans fichier de vie : aucune
action (parité R1-R7 des bots manuels).

Cycle de vie (`planner/src/lifecycle.rs`, seul endroit qui change un
statut ; côté daemon aucun `status =` hors fixtures de test). Mouvements
nommés : `mark_launched`, `record_failure`, `record_contract_error`,
`record_block[_from]`, `record_progress`, `complete_node` (preuve positive :
rapport, avoirs, bourse, niveau live, bot vu hors de l'île),
`retry_uncounted` (arrêt opérateur, redémarrage, remplissage partiel,
verrou Iron refusé), `end_engagement`, `insert_ready` (concierge, tuto),
`revive_stuck`. `normalize` est appelé après chaque mutation et garantit :
dépendance abandonnée, inexistante ou en cycle ⇒ abandon transitif ;
`Ready` ⇔ toutes les dépendances `Done` ; un seul `Active`, égal à
`life.active` ; vivacité (graphe non fini ⇒ un nœud actif ou prêt).
`graph_state` : `Running`, `Launchable`, `Finished`, `Stuck`. Compteurs
persistés par nœud : `failures` (max 3), `blocks` (max 3), `revives`
(max 3), `last_error`. Un abandon en cascade écrit sa cause
(`dependency nX abandoned`), jamais l'ancien blocage du nœud. Un nœud
`obtain` ouvert dont une jambe ne mappe aucun script (jambes figées par
un ancien planner, ex. `farm:ball_of_wool`) re-dérive ses jambes de
l'index d'obtention à `normalize`.

But atteint malgré d'anciens abandons (2026-11-13, `brain_stuck::effective_state`) :
un graphe `Stuck` dont le but courant est atteint — niveaux live, ou nœud
racine du but (`goals::goal_root_done` : le plus récent nœud equip/obtain/
argent/quête/zone du but, dans le graphe ou `memory.recent` une fois
retraité) `done` — est traité `Finished` (clos `done`, but
suivant), jamais une période de blocage (live : 540 `graph stuck`/h ;
bb4v5d9m bouclait « equip melee » fait toutes les 4 s, clos `stuck`, jalon
bloqué ×3, ambition guerrier ↔ prêtre toutes les 30 s).

Graphe bloqué (`brain_stuck.rs`) : une période de blocage = une ligne de
log + `history: graph stuck`, une seule fois. La ligne est bornée
(`stuck_summary`) : `abandoned: N nodes (K by cascade); roots: nX=<erreur>
| … (+R more roots)` — 3 racines au plus, 160 caractères d'erreur chacune
(avant : bst29bh9 alignait ~90 paires `nX=dependency nY abandoned`). Le but courant est clos
une fois : `done` si les niveaux live l'atteignent (des nœuds abandonnés
plus anciens n'en font pas un échec), `stuck` sinon ; puis but suivant.
Plus aucun but ⇒ règle de relance (`lifecycle::revive_stuck`) : après
`REVIVE_COOLDOWN_TICKS` = 6000 ticks de blocage (1 h en prod, 6 min en
dev), chaque nœud abandonné du but courant (ceux des buts clos sont
retraités) — sauf les concierges `space_cleanup`, qui ont
leur propre arrêt — repart avec des budgets neufs et des jambes re-dérivées
(index d'obtention ; méthode de palier si toutes bannies), sa cascade
avec lui ; `history: graph revived`. Au plus `MAX_REVIVES` = 3 relances
par nœud ; au-delà `history: graph final`, une ligne, l'opérateur décide.

Rapports (contrat v2) : tout lancement `--node` produit un `task_report`
(tuto, walkto, quêtes compris, échec de parsing compris) portant
`error: {type, message}`. Classement (`brain_fold::verdict`) : `Usage`
(arguments, dispatch impossible) ⇒ abandon immédiat ; `Failed`/`Sdk`/
mort ⇒ échec compté puis relance ; échec de voyage ⇒ méthode bannie +
échec compté ; `blocked` ⇒ sous-tâche `obtain` par objet manquant
(réutilisée si déjà ouverte) ; `stopped` (unload, redémarrage) ⇒ prêt,
non compté ; fenêtre finie avec but atteint ⇒ `done`. Boucles muettes
(2026-11-14, `brain_fold::verdict_watched`, `report_progress.rs` : n133
de harry_page relancé ~115 fois, 689 124 ticks, `budget_exhausted` sans
objet ni XP plié en fin de fenêtre) : un rapport que le détecteur signale
(`no_output` : fenêtre ≥ 1000 ticks finie au budget sans XP ni objet
gagné ; `bank_churn` : ≥ 40 banques, moins d'unités gagnées que de
banques, 0 XP ; marché, `walkto`, tuto, `equip` exclus) échoue son nœud
(`no progress: …`, compté, 3 ⇒ abandon puis règles de rachat/ban) ; un
nœud mesurable (montée, combat, obtention) qui finit 3 fenêtres d'affilée
sans progrès vers lui (`engagement_idle` dans l'historique, arrêts
opérateur ignorés) échoue aussi, ligne `brain <c>: nX no progress: …`.
Une sous-tâche bloquée sur un intrant d'un étage intermédiaire (argile et
eau de l'argile molle d'un plat) est dimensionnée à travers l'étage
(`replan::nested_per_unit`, 2 étages : 16 plats crus ⇒ 16 argiles, pas
1) ; les récipients d'eau d'une méthode `pottery:` sont plafonnés à
`POTTERY_VESSELS` = 7 (le script les remplit lui-même). Progrès
(2026-11-13, `brain_fold::xp_toward`, `lifecycle::record_xp`) : l'XP que
le rapport crédite à la montée (sa compétence ; toute compétence de combat
pour `reach_combat`) remet à zéro le compteur de blocages comme un niveau
(t01, eric_smith, bb4v5d9m : chaque recharge de 20 crevettes d'une
montée de mêlée comptait un blocage, la 4e bannissait la compétence) ;
`goal_reached` court mais avec progrès (XP, ou objets gagnés pour un
`obtain`) ⇒ fin de fenêtre non comptée, la montée continue (frank_lane,
leo_king : prière aux vaches, +652 XP par fenêtre, bannie après trois).
Une quête n'est `goal_reached` que sur l'achèvement vu (« Quest complete »)
: `quests::common::quest_outcome` rend l'erreur typée du solveur, un échec
sans message de fin, ou `Blocked` sur les niveaux/objets manquants (avant :
toujours 0). Un nœud actif sans
rapport (worker repassé idle, crash) compte un échec après 30 s, sauf
s'il suit un redémarrage du daemon ou un `unload`/`close`/`open`
opérateur (`brain_grace.rs`, une excuse par arrêt, effacée au lancement
suivant). Un rapport drainé quand la file du cerveau est pleine repasse
en attente au lieu d'être perdu. Une
commande market non remplie attend au plus 6000 ticks : la JAMBE marché
du nœud échoue (comptée, le nœud replanifie), l'offre reste au carnet
avec sa lignée (plan marché M2) ; `place_obtain_bid` la reprend au
prochain achat, elle se ferme quand le besoin est servi ou qu'aucun
nœud ouvert ne veut plus l'objet (`release_orphan_needs`).

Décision sans données interdite : sans position live, rien ne part.
Tuto (`brain_tutorial.rs`) : sur Tutorial Island (rectangles, pas les
labels de zone) seul le tuto part, en nœud rapporté ; clôture sur son
rapport ou sur preuve positive (bot vu hors de l'île sans tuto en cours) ;
tuto abandonné sur l'île ⇒ aucun lancement, l'opérateur décide.
`create-user [pseudo] [--iron] [--goal kind] [--role market_maker]` :
pseudo (`Iron`+8, 12 max ; 20 Iron max), profil graine, squelette, `open`
; le verrou tuto lance le tutoriel au premier passage, puis le but.
`Iron` ne marchande jamais (aucune méthode `market:*`, jamais teneur).

Teneur de marché (`--role market_maker`, plan marché Q6, vague 3 ;
`create_user_cmd.rs`, `brain::maker_start`, `planner::market_maker`) :
la vie porte `profile.archetype = "market_maker"` et `profile.maker =
{slice, slices, capital_target, phase, capital_at_switch, switched_at}` ;
tranche = la première libre parmi les 11 (`market_maker::taken_slices`
sur `run/lives`), capital cible COMMUN à tous les teneurs
(`market_maker::common_capital_target`, 103 000 ; tranches équilibrées
par capital, migration des vies anciennes au chargement :
`market_maker::migrate_role`, docs/economy.md) ; refus typé si toutes les tranches ont
leur teneur, si `--iron`, ou rôle inconnu. Phase `earning` : bot normal
(tutoriel, banque, combat, équipement), ambition opérateur
`market_maker` (échelle de pièces, tenue pour toujours comme `overall`,
jamais tirée par la flotte) dont les jalons `coins` donnent `GetRich`
(méthodes d'argent productrices). Dès que pack + banque ≥ capital cible
(`maybe_switch`, chaque passe de lancement) : bascule DÉFINITIVE
`quoting` (`become_quoting` → `lifecycle::abandon_for_role`, cause
`role: market maker`, achats ouverts et annonces de la tranche annulés
`role`), but `MakeMarket` = un nœud `make_money` à jambe unique
`market:make` dont le site est le poste de la tranche
(`MAKER_POSTS[slice % 8]`, anneau de remise de la fontaine). En
`quoting` : aucune passe d'ambition, de souhait, d'outil, d'équipement ni
d'alchimie, aucun bid répondu ; restent la passe de cotation
(`market_maker::quote_pass`), la vente des restes hors tranche, la vente
PNJ des invendables (Q3), le concierge (tranche gardée), la nourriture.

Migration (vague 3) : une vie d'avant (archétype `merchant`, but
`get_rich` marchand, jambe de revente, ambition `merchant`) se charge en
productrice : jambe retirée de chaque nœud (un nœud qui la tenait se
résout à nouveau), ambition retirée (le cerveau en assigne une de flotte
au passage suivant), but `get_rich` producteur (`migrate_merchant`).

Épingle d'ambition (décision utilisateur 2026-11-14) : `--goal
ambition:<compétence>:<niveau>` (ex. `ambition:magic:99`) crée la vie avec
l'ambition du groupe de la compétence (`magic` → `mage`, `smithing` →
`smith`, `cooking` → `cook`…) ÉPINGLÉE (`ambition.pin = {skill, level}`,
`why = operator:pin:<compétence>:<niveau>`, `ambition::pinned_ambition`)
et part sur son premier jalon (`goal_origin = m1`). Tant que la compétence
n'a pas le niveau : jamais de redraw (bloqué ×3 ⇒ pour une épingle
seulement, le jalon est retiré, `CloseOutcome::Dropped`, l'échelle
continue ; hors épingle il est mis de côté puis repris, voir « Clôture »
plus bas), jamais de changement
facultatif ni de réparation de couverture, échelle épuisée ⇒ prolongement
(`extend`, +5 niveaux jusqu'à 99). Refus typé (`NotPinnable`) si aucune
ambition ne grimpe la compétence (PV, membres) ou niveau hors 2..=99.
Niveau atteint ⇒ l'épingle tombe (`release_pin`, loggé) et l'ambition vit
comme les autres.

Succession (`planner/src/goals.rs`, `planner/src/bans.rs`,
`brain_succession.rs`) : un but est clos UNE fois (`goal_closed` : la
dernière entrée hors `graph`/`unban:` est un `done:`/`stuck:`), puis
`goals::draw_goal` — point d'entrée unique du tirage, où se brancheront
les ambitions par bot — tire le suivant : buts d'argent clos `done` ⇒ même
but (nouvel engagement), sinon compétence entraînable la plus basse non
bannie, jusqu'au prochain multiple de 5 au moins 5 niveaux plus haut,
nœuds dépliés depuis le niveau live ; historique `goal: next:<libellé>`.

Rachat avant bannissement et portée du bannissement (décision utilisateur
2026-11-13, `planner/src/bans_scope.rs`, `brain_rescue.rs`) : à la
première passe d'un graphe bloqué, chaque obtention abandonnée du chemin
en échec (la racine d'abord, puis ce qui l'attendait) est testée au
marché (échangeable, pas Iron, bourse libre ≥ quantité × meilleure offre
de vente, sinon × prix de référence) ; la première qui passe repart en
`market:buy` seul (`lifecycle::retarget_market`, compteurs neufs, ses
dépendants en cascade reviennent, une seule fois par nœud) et rien n'est
banni. Une obtention dont le dernier échec est son propre achat resté
sans vendeur (`bans_scope::market_failed`, texte
`market_unfilled_error`) ne rachète jamais : elle repart sur ses jambes
hors marché (`lifecycle::retarget_produce` : fabrication, récolte,
élevage, magasin ; live fiona_park 2026-11-14, cinq amulettes de saphir
rachetées en boucle) ; sans telle jambe, pas de sauvetage, le ban suit et
le planificateur replanifie. Un seul sauvetage par nœud, quel qu'il soit
(historique `market_rescue` / `produce_rescue`, `rescued_before`).
Sinon le but clos bloqué bannit TOUT le sous-arbre de l'objet
(`banned_scope`) : compétences de la racine et de chaque nœud qui
l'attend, plus celles dont l'objet est l'outil (barres de bronze ratées
⇒ forge, bûcheronnage — la hache —, allumage s'il brûlait ces bûches).
Exception : une quête en échec ne bannit AUCUNE compétence (2026-11-14,
Rune Mysteries avait banni le minage de l'essence qui l'attendait) ; la
quête elle-même refroidit (`goal: stuck:quest:<clé>@6000:…`,
`quest_hold::cooled_quests`), ses nœuds prêts attendent comme un achat
parqué.

Objets de quête (`planner/src/quest_hold.rs`, `brain_quest_gate.rs`,
2026-11-14, bh99tw2f : ses annonces verrouillaient les dix talismans
d'air en banque, boucle Rune Mysteries) : tant qu'un nœud de quête est
ouvert, ses objets (`quest_requirements::reserved_objects` : objets
apportés à leur quantité, objets remis par les PNJ — talisman, paquet,
notes — à 1) sont réservés : la passe de vente ne les propose pas et
coupe l'annonce qui les verrouille (reste ré-annoncé au même prix), le
marché refuse toute annonce qui les prendrait (`free_coverage`). Une
quête ne part qu'avec ses objets en main (`quest_shortfalls`) : objet
apporté manquant ⇒ obtention d'abord (`record_block_from`), unités
détenues mais annoncées ⇒ libérées puis attente d'une passe ; objet
remis détenu nulle part ⇒ la quête le donne. Pendant la quête, le script
garde ces objets dans le sac (`Bot::hold_in_pack`) : aucune livraison
marché ne les range ni ne les échange.

Bannissements temporaires et typés (décision utilisateur 2026-11-13) : un
but clos bloqué (graphe bloqué, but non atteint en live) bannit chaque
compétence qu'il entraîne pendant `SKILL_BAN_TICKS` = 6000 ticks (1 h en
prod, 6 min en dev ; temps mur converti avec la durée de tick mesurée),
jamais à vie. Entrée `goal: stuck:<compétence>@6000:<classe> (node <id>,
<dernière erreur>)` ; la cause est la racine du dernier abandon
(`bans::stuck_cause`, cascades `dependency nX abandoned` remontées),
classe `N failures` | `N blocks` | `contract error`. Log :
`brain <compte>: skill <s> banned 6000 ticks: <classe> (node <id>,
<erreur>)`. À l'expiration : une ligne `skill <s> ban expired after 6000
ticks (was: …)` et une entrée `goal: unban:<s>`, une seule fois. Les
anciennes entrées `stuck:<a>,<b>` (sans tick) comptent comme expirées
(log `ban lifted (old entry without tick …)`). Outil, intrant, pièces,
niveau ou combat manquants ne bannissent pas : ils deviennent des
sous-tâches (`record_block_from` ⇒ `obtain`/`have_coins`/`reach_*`) ; un
bannissement ne vient que d'un abandon (échecs répétés, blocages qui ne
guérissent pas, erreur de contrat). Plus rien à tirer ⇒ le tirage est
retenté à chaque passe (graphe fini, période de blocage ou `graph final`) :
l'expiration d'un bannissement suffit à relancer le bot ; en parallèle la
relance des nœuds abandonnés (ci-dessus) reste bornée à 3. Le libellé
affiché est `fishing 10` (but `max:fishing:10`).

Données avant décision : rien ne part sans position live ni sac reçu
(`inv_streamed`) ; tant que la banque n'a pas été vue (`bank_streamed`),
le contrôle d'outils ne conclut jamais « manquant » (le script récupère
l'outil en banque ou lève `Blocked`). Une montée de niveau au combat
ne lâche jamais rien (économie : chaque objet compte) ; le mode
`--no-bank` des scripts (combat sans banque, pêche qui lâche sa prise)
existe mais le planner ne l'envoie pas. Pêche de débutant (crevettes,
anchois, sardines, harengs, et `farm:raw_shrimp`) : seule eau gratuite
sans péage ni bateau = Draynor, chassée par les mages (niveau 7) jusqu'au
combat 14 ⇒ `min_combat_for_method` = 15, le verrou de combat entraîne
d'abord (poulets), puis le bot pêche et dépose tout en banque.
Combat : clés de spot réelles (`chickens`, `cows`) ; un spot inconnu est
une erreur `Usage` (avant : repli silencieux sur l'auto-pick). Dès le
niveau 10, CHAQUE style monte l'échelle (`tranches::tranche_resource` :
`fight:melee/ladder`, `fight:ranged/ladder`, `fight:magic/<sort>/ladder`,
utilisateur 2026-11-20 : archers et mages vont aussi aux démons) ; le
sort d'une tranche magique est le meilleur sans paramètre de cible ; le
prior d'une échelle distance/magie lit les vaches
(`tranches::LADDER_PRIOR_SPOT` : flèches et runes dimensionnées comme
avant, les taux mesurés prennent le relais) ; les malédictions et les
guides gardent les camps fixes (`tranches::starter_camp`) ; une échelle
n'est jamais une activité de guide ni une méthode d'argent
(`tranches::is_ladder`). Danger d'un site de combat :
`danger::fight_site_requirement` (sans banque à `BANK_TREK_LIMIT`, le
site seul : un voyage qui dure jusqu'à la fin de la nourriture), une
récolte garde `site_requirement` (`NEVER` sans banque). Ferme
par drop (`farm:<objet>`, `mapping::spot_for_drop` →
`gamedata::combat_spots::drop_spots`) : le spot NON GARDÉ le plus faible
dont un PNJ lâche l'objet (os ⇒ poulets), un spot à péage seulement s'il
est le seul (péage budgété, voir ci-dessous), un camp souterrain que la
navigation atteint après ceux de surface de son groupe (big bones ⇒ géants
du donjon d'Edgeville, 2026-11-21) ; jamais clé ou bateau, jamais un spot
sans position (bâton de magie : moss giants derrière la toile ⇒ pas de
jambe `farm:`). Les spots
du drop sont ses sites (`combat::method_sites`), comme une récolte.
Seulement un spot que l'exécutant de mêlée sait coter
(`melee_ladder::RUNGS` : jamais les mages noirs, qui lancent des sorts) et
que les stats live du bot survivent (`combat::rung_fit` : `rate` à nu,
nourriture et portes supposées obtenues ; sans niveaux live, pas de
filtre) ; sinon la jambe `farm:` n'est pas faisable (`method_feasible`) et
le marché ou une autre voie sert (live 2026-11-13 : bd23dqpq, 53 partout,
mort aux mages noirs pour une rune cosmique).

Péage d'Al Kharid (décision utilisateur 2026-11-13, `gamedata::toll`,
serveur `border_gate.rs2` : 10 pièces prises dans le SAC, dans les deux
sens, gratuit après Prince Ali Rescue). Le choix de site
(`combat::pick_site_paid`) suit la règle des scripts (bot à ≤ 150 cases de
la porte (3268,3227), cible de l'autre côté de la clôture,
`gamedata::toll::crossing`) : le côté est une RÉGION dérivée de la
connexité de la carte (`gamedata/data/toll_region.json`, voir
`docs/navigation.md` § Péage), jamais la droite x = 3268 (la clôture
zigzague, la côte sud d'Al Kharid descend à x 3259 : live bd23dqpq
2026-11-14, banc (3267,3148) lancé sans `toll`, NoRoute, pêche bannie).
Trois classes : derrière le péage, côté gratuit (reste de la fenêtre au
sud du haut de la clôture, z ≤ 3329 : Lumbridge, Draynor, Port Sarim),
terrain ouvert au-delà (Varrock, Falador : détour gratuit par le nord,
jamais « de l'autre côté ») ; scripts (`kit::bans::across_toll`,
`same_side`, `same_flank_clusters`, banque la plus proche, passage de la
porte) et cerveau lisent la même région. Un site du
même côté d'abord (or de Rimmington, pas d'Al Kharid, depuis Lumbridge —
sara_miller) ; sinon un site de l'autre côté seulement si le péage est
budgété (bourse ≥ 10 ou quête faite) ⇒ `node.toll`, lancement avec
`toll` (pêche, minage, bûcheronnage, mêlée, distance, magie) ; sinon
`Blocked missing:coins:10` ⇒ dépendance `have_coins`. Côté script,
`kit::toll::arm` va chercher les 10 pièces en banque avant le premier
passage et `bank_trip` les garde dans le sac pendant une course payante.

Portes d'accès des sites (2026-11-13, `gamedata::access`) : guilde
d'artisanat (40 + tablier brun porté), des cuisiniers (32 + toque portée),
des mineurs (60), clé de laiton (Edgeville), Karamja (bateau 30 po,
`sailors.rs2`), Crandor, souterrain, Wilderness. Souterrain (décision
utilisateur 2026-11-21) : un combat ou une ferme de butin
(`combat::is_fight_method`, pas les malédictions) lit
`access::fight_gates_at`, qui lève la porte souterraine sur une position
de camp prouvée atteignable aller-retour (`nav/tests/fight_sites.rs`,
voir `docs/navigation.md`) ; ses autres portes restent (bateau du volcan,
clé, guilde). Récoltes et postes gardent `gates_at` : jamais sous terre
(sauf guilde des mineurs). Ouverts ainsi : démons mineurs du volcan
(bateau + corde, besoin `coins:60` sans le passage), donjon d'Edgeville
(trappe, sans clé), donjon de glace (échelle de Mudskipper Point), égouts
de Varrock, mine des Nains (scorpions). Danger et banque se lisent au
spawn (`site_banks` prix par route, à travers les échelles) ; un camp à
risque ne compte pas ses propres adversaires (`danger::camp_site_requirement`,
comme `cluster_requirement` de l'échelle : 165 pour les démons, jamais
atteint), son plancher (niveau − 2) reste. La marche d'un choix de site
passe par l'entrée souterraine la plus proche
(`method_rate_trip::site_walk_tiles`, aussi `market_value::walk_ticks`).
La chasse (`kit::hunt`) descend au site souterrain du cerveau ; un nom de
camp souterrain sans site ni grappes de l'échelle reste refusé.
`combat::pick_site_gated`
+ `Pass` (niveaux live, objets détenus) : un site gardé compte seulement si
la porte s'ouvre ; sinon `SitePick::Access` ⇒ `missing:access:<guilde>`
(déplié : montée + uniforme) ou `missing:<clé>`. Banque et danger d'un site
de guilde se lisent à sa porte ; un poste d'étage sans banque à son étage
se lit au rez-de-chaussée. Les postes (fourneau, enclume, fourneau de
cuisine, rouet, tour, autel, trémie, tanneur, Aggie, apparitions au sol)
sont des sites choisis comme les récoltes ; le magasin d'un `shop:buy` est
choisi par `shop_site::pick_shop` (quête, porte, danger, péage).
Karamja (2026-11-13) : la navigation embarque (arêtes `custom_edges`
« npc_fare » des marins de Port Sarim et de l'officier des douanes,
passerelles de débarquement, voir `docs/navigation.md`) :
`access::BOAT_ROUTED` = true. Un site de Karamja (homard, thon, espadon,
or de Karamja) est un site payant budgété comme le péage
(`combat::pick_site_gated`, `fare_need`) : l'aller-retour coûte 60 po
(30 chaque sens, pris dans le SAC), plus 10 si le trajet franchit le
péage d'Al Kharid. Un site continental gratuit gagne toujours ; sinon le
site payant le moins cher puis le plus proche, budgété (bourse ≥ coût) ⇒
`SitePick::Boat` ⇒ `node.boat` (et `node.toll` si péage), lancement avec
`boat` ; sinon `Blocked missing:coins:60` (ou 70) ⇒ `have_coins`. Seuls
les scripts qui arment la traversée prennent un site de Karamja
(`combat::boat_capable` : pêche, minage, bûcheronnage, combats,
`farm:`) ; les postes y restent fermés, Crandor (bateau de Ned) aussi.
La banque d'un site de Karamja se lit au quai de Port Sarim
(`access::mainland_anchor`, `danger::site_bank`) : Draynor, d'où un
combat 15 pour le homard (les mages de Draynor).

Familles de production (2026-11-13) : `cut:` (gemmes, ciseau), `bake:`
(pain, tartes, ragoût, pizza, gâteau ; intrants aplatis jusqu'aux
composants de base que `cook_meals` assemble), `dye:` (Aggie : 2 oignons /
3 baies / 2 guède + 5 po), `ground:` (apparitions libres, sans porte,
banque à portée), `telegrab:wine_of_zamorak` (magie 33), `superheat:`
(magie 43, intrants de fonte + runes, jamais d'échec), `bury:` (prière en
banque). Les voies de sort brûlent leurs runes. Achats en dialogue : jambe
`dialogue:buy` (voir « Achats en dialogue »).

Achats en dialogue (2026-11-13, `gamedata::dialogue_shops`) : PNJ qui
vendent par leur dialogue, réponses lues dans les scripts serveur (Karim
et ses kébabs, le marchand de soie, Baraek et ses fourrures, la serveuse
du Rising Sun, Ned et sa corde, les tenanciers). Même règle marchande que
`shop:buy` (`allowed` = `merchant_buy_allowed` + vendeur F2P) : kebab,
silk, fur, asgarnian_ale, wizards_mind_bomb, dwarven_stout, rope (la bière
n'y passe pas). `obtain_alternatives` ajoute `dialogue:buy`,
`resolve_obtain` la prend après le marchand (`coût × manquant` couvert
par la bourse, sinon `missing:coins:N`), `place_on_site` choisit le
vendeur (`dialogue_site::pick_seller` : sûr au combat du bot, hors
porte, le moins cher puis le plus proche ; péage ajouté au coût quand le
trajet franchit la clôture ⇒ `toll`).

Échelle de points de quête (`quests::quest_point_ladder`) : points les
moins chers d'abord (récompense ÷ effort : objets, compétences, combat
recommandé), une quête à seuil seulement une fois son seuil atteint par
les marches d'avant, jamais Shield of Arrav (deux joueurs) ; Pirate's
Treasure en est (bateau planifiable : 90 pièces, une bêche, les bananes
cueillies par le solveur) ; les quêtes faites comptent. Dragon Slayer, Black
Knights' Fortress et la Champions' Guild (32) en dépendent (fini le repli
Cook's Assistant). Objets de quête lus dans les scripts serveur
(`gamedata::quest_requirements`, 39 points en F2P).

Quêtes (décision utilisateur 2026-11-13, `planner/src/quests.rs`) : une
méthode dont la quête manque ne part jamais (`method_feasible`) ; le
résolveur prend une autre voie, sinon bloque `missing:quest:<clé>` et
`record_block_from` déplie la quête (`expand_quest_from` : niveaux,
objets, nœud quête). Sources serveur : essence ⇒ Rune Mysteries
(`essence_mine.rs2`, `Way::quest` de la récolte `blankrune`), magasin dont
toutes les boutiques sont gardées ⇒ leur quête (plastron rune : Dragon
Slayer), port d'équipement gardé (`gamedata::gear`), péage gratuit
(Prince Ali). Quêtes faites = varps de la sauvegarde moteur (jamais
transmises au client) ∪ nœuds quête faits de la vie
(`brain_rescue::quests_done`) ; un nœud quête déjà faite se clôt sans
lancement. Un blocage `missing:quest:<clé>` est une preuve live que la
quête N'EST PAS faite : `record_block_from` marque chaque nœud quête fait
de cette clé `last_error = "disproved: …"` (statut inchangé, historique)
et `quests_done_in_graph` l'ignore ; le nœud quête neuf part pour de bon
(live 2026-11-13, bxkchh6x/leo_king/nina_rhodes/sara_miller : le script
de quête rendait 0 sans finir, le nœud fait fermait ensuite chaque nœud
quête « déjà fait » pendant que l'essence bloquait 4 fois ⇒ minage banni).
Un nœud bloqué ne se réutilise jamais lui-même ni ce qui l'attend comme
dépendance (t01 : cuisine 10→20 bloquée sur un niveau de cuisine,
cycle n8↔n9 abandonné).

Quête légère d'abord (2026-11-14, `quests::light_quest` : ni niveau, ni
objet, ni combat, ni points de quête, faisable seul — Rune Mysteries,
Cook's Assistant…) : quand la seule voie de production est derrière elle
et qu'aucune offre du marché n'est couverte, le résolveur bloque
`missing:quest:<clé>` au lieu de poser une enchère au repos que personne
ne remplit (live : `market order for 17 blankrune unfilled after 6000
ticks`, runecraft et minage bannis, aucun bot n'ayant la quête). Une
offre couverte achète toujours. Objets que la quête remet et laisse
(`quests::QUEST_GRANTS`, preuve contenu : `duke_horacio.rs2` donne
`air_talisman`, `sedridor.rs2` le rend à la fin) : l'obtention attend la
quête, jamais la ferme aux gobelins (0,78 %/kill) ni le marché. Un ban
de compétence dont la cause nomme une quête (`quest:<clé>` ou le script
`quest_<nom>`) se lève dès que la quête est faite
(`bans::quest_lifted_bans`, appelé par `brain_succession::note_expired_bans`
avec `brain_rescue::quests_done`), sans attendre ses 6000 ticks.

Rune Mysteries (script `quest_rune_mysteries`,
`scripts/src/quests/rune_mysteries.rs`) : le varp `runemysteries` n'est
pas transmis (`scope=perm`, sans `transmit`) — l'ancien solveur lisait 0
et rejouait tout. L'étape se lit dans le sac : notes d'Aubury ⇒ Sedridor ;
paquet ⇒ Aubury deux fois ; rien ⇒ le Duc (« Have you any quests for
me? », « Sure, no problem. »), Sedridor (« I'm looking for the head
wizard. », « Ok, here you are. », « Yes, certainly. »), puis Aubury si la
manche finit sans objet. Réponses jamais vers la boutique d'Aubury
(« No thank you »). Preuve live 2026-11-14 (t40 puis t48, `direct` sur un
daemon privé portant la nav du correctif) : Duc → Sedridor (porte
diagonale, échelle, porte 1530 de la cave) → Aubury ×2 → Sedridor, varp
63 = 6 en ~1500 ticks.

Runecraft (2026-11-14, `tranches::talisman_obtainable`) : chaque tranche
travaille la meilleure rune ouverte dont le talisman s'obtient — quête
(air) ou jambe de production exécutable (terre : hommes 1,56 %, corps :
gardes 3,12 %). Esprit (diablotins, sorciers), eau et feu (sorciers,
mages noirs, squelettes des égouts) ne tombent d'aucun échelon de combat :
air jusqu'à 9, terre jusqu'à 20, corps ensuite (paliers 2/5/9/14/20
inchangés). Un nouvel échelon qui chasse ces PNJ rouvre leurs paliers
sans autre changement. Essence : `essence_run` lit rochers et portails
de la mine en statique (le serveur ne streame jamais une loc statique ;
avant : « Aubury did not teleport » une fois DANS la mine), arrivée et
sortie par le carré 45_75, portail cliqué depuis un côté orthogonal
(diagonale : « I can't reach that! »).

Magasins (règle utilisateur 2026-11-13, `obtain::merchant_buy_allowed`) :
un PNJ ne vend au bot que ce que F2P ne donne d'aucune autre façon —
ni production ni drop qu'un spot farme. Plumes (poulets), appât (hommes),
flèches bronze : marché ou ferme, jamais magasin ; filets, cannes,
cisailles, harpon : magasin. Pêche : l'appât est un INTRANT consommé (une
plume par truite, un appât par brochet, dimensionné par `need_count`),
la jambe `farm:raw_<poisson>` porte le contrôle d'outil/appât de la
récolte et part avec le poisson en premier positionnel (b8jxrdd2 : quota truite pêché au
brochet) ; la viande crue (`raw_beef`) est un drop, jamais une pêche.

Buts : `max:a,b:40` | `obtain:item:n` | `produce:item:n` | `money` | `quest:clé` |
`equip:style` | `area:zone` | `rich` (méthodes productrices ; la variante
marchande est supprimée en vague 3).

Banc hors ligne : `brain_sim_tests.rs` rejoue 30 comptes (graphes réels,
issues aléatoires : succès, échec, Usage, blocage, arrêt, rapport perdu)
et vérifie vivacité, terminaison et scripts enregistrés.

## Ambitions par bot (2026-11-13)

Chaque vie porte une AMBITION long terme (`life.ambition`) qui tire ses
buts : `planner/src/ambition.rs` (modèle, jalons, seul endroit qui change
un statut de jalon), `ambition_catalog.rs` (catalogue), `ambition_fleet.rs`
(couverture de flotte), côté daemon `brain_ambition.rs` et
`market_demand.rs`.

Catalogue (un par groupe F2P rev 274, la mêlée = UN groupe : attaque,
force, défense, PV ; la taille d'arc est membres) : `warrior` (mêlée),
`ranger`, `priest`, `mage`, `cook`, `woodcutter`, `fisher`, `firemaker`,
`crafter`, `smith`, `miner`, `runecrafter`, plus `generalist`, `overall`
et `market_maker` (opérateur seulement, jamais tirés ; le `merchant` est
supprimé en vague 3). Chaque entrée est une
échelle de paliers : niveau à atteindre puis quota de production
(plusieurs produits candidats par palier, choisis par la demande ouverte,
sinon par la graine : les bots d'un même type ne fabriquent pas tous la
même chose), tier d'équipement (`equip_best`) ou cible de pièces
(phase de jeu d'un teneur de marché). Exemple forgeron : minage 10, forge 10 puis 20 dagues/épées/
casques bronze, forge 18 puis 10 plastrons/jambières/boucliers bronze,
fer, acier, mithril. Les paliers dépassés de 10 niveaux sont sautés
(frank_lane, bûcheronnage 64, commence aux 150 bûches d'érable/saule).
Tous les produits ont une jambe de production (testé).

Jalons (`milestones`, 3 ouverts matérialisés d'avance) : `level` (clos
quand le niveau live l'atteint), `produce` (but `GoalKind::Produce
{item,count}` = un nœud `obtain` à jambes de PRODUCTION seulement, jamais
acheté ; 2026-11-15 : jamais ramassé au sol (`ground:`), et une ferme de
drops seulement quand aucune compétence ne fabrique l'objet (les peaux
du guerrier) — sinon les smiths ramassaient des iron_platelegs et les
runecrafters des mind runes, sans XP (`produce_alternatives`) ; un échec
de ramassage ne bannit aucune compétence (`bans_scope::node_skill`) ; intrants achetables via les sous-tâches ; la vente par défaut
liste le résultat une fois le nœud fini), `equip`, `coins` (teneur de
marché : `GetRich` jusqu'au montant détenu, pack + banque). Demande :
une offre d'achat ouverte qu'une ambition sait servir (à ≤ 10 niveaux,
entraînement d'abord) devient un jalon `produce` en tête (`origin:
demand:<objet>`, quantité = non servie, 5..100), un seul à la fois,
seulement si elle PAIE (`DemandItem::margin` > 0 : meilleure offre moins
les intrants de la meilleure recette à leur référence marché,
`profit::chain_profit` ; sans marge connue, rien) et si le bot ne détient
pas déjà la quantité (sac + banque + porté ; le stock annoncé reste sur
le bot jusqu'à la livraison). Un quota `produce` compte depuis le stock
détenu au tirage (`append_goal(…, held_of)` : le nœud `obtain` demande
`détenu + quota`), jamais crédité du stock existant (revue 2026-11-13 :
un cuisinier à 100 crevettes protégées bouclait jalon/fait/réinsertion).
Les jalons faits sont bornés à `MAX_DONE_KEPT` = 40 (les plus anciens
partent, ids jamais réutilisés).

Rentabilité de tout le marché (2026-11-14, voir `docs/economy.md`
§ Rentabilité de tout le marché) : le tirage pèse chaque objet de valeur
(bids, prix courant, alchimie), pas seulement les bids ; les poids
d'ambition comptent toute demande que la compétence primaire d'un type
fabrique, même hors de portée (adamant → forgerons, minerais et charbon
à moitié → mineurs).

Réponse aux offres (2026-11-14, `brain_bids` + `planner::bid_response`,
voir `docs/economy.md` § Producteurs qui répondent aux offres) : à chaque
tirage, AVANT le jalon, une offre d'achat publique que le bot fabrique
maintenant à un taux ≥ 2 × la valeur de son temps devient un quota
`Produce` d'origine `bid:<objet>` (ambition intacte, comme un but
d'appoint) ; sinon le tirage ci-dessous. Depuis le 2026-11-20 : seulement
avec une place libre sur un site de récolte à places limitées et les
intrants que les autres réponses n'ont pas déjà réservés ; une réponse
bloquée (toutes méthodes refusées) plus de 6000 ticks est rendue (voir
§ Contention).

Tirage (`goals::draw_goal_origin`, toujours l'unique point d'entrée) :
premier jalon ouvert dont les compétences ne sont pas bannies (`bans.rs`
respecté), but + `life.goal_origin = "mN"` + historique `goal:
next:<libellé>`. Tous les jalons ouverts bannis : but d'APPOINT temporaire
(`side_goal`, D-SIDE : demande à marge > 0 faisable hors bans, sinon
argent hors compétences bannies ; origine `side:…`, ambition intacte,
retour à l'ambition au tirage suivant). Sans ambition : AUCUN but
(`NoGoal::NoAmbition`, typé et loggé) ; le plus-bas-skill ne sert que le `generalist` explicite. Clôture
(`brain_succession::close_goal` → `brain_ambition::after_close`) : quota
ou équipement fait ⇒ jalon `done` ; but bloqué ⇒ `blocked`,
`blocked_count + 1`. A2 (décision du 2026-11-21) : une ambition ne change
JAMAIS avant d'être atteinte. À `MILESTONE_MAX_BLOCKS` = 3 le jalon est
MIS DE CÔTE (`ambition::set_aside` : statut `blocked`, compteur gardé, la
trace reste dans la vie ; `CloseOutcome::SetAside`, log `milestone mN
blocked 3 times, set aside; ambition K kept, next milestone`) : il n'est
plus tiré (`next_milestone`, `next_feasible_milestone`), ne compte plus
parmi les jalons ouverts (`open_count` : les rungs suivants se
matérialisent à sa place) et le bot passe au jalon suivant. La règle lit
le compteur écrit : une vie déjà écrite avec un jalon `blocked` ×3 la
suit au chargement, sans rien déclencher de plus. Si TOUS les jalons
restants sont mis de côté, le tirage ne trouve aucun jalon ouvert : but
d'appoint (`side_goal`) comme pour des jalons bannis, jamais un
retirage. Un jalon mis de côté n'est pas perdu (règle stricte : l'ambition ne change jamais avant
d'être atteinte) : sa mise de côté pose un refroidissement de
`AMBITION_CHANGE_TICKS` = 72 000 ticks (clé `milestone_cool_key`), et
quand le bot n'a plus AUCUN autre jalon tirable ET que ce
refroidissement est fini, `ambition::resume_set_aside` le rend tirable
(compteur remis à zéro, statut `pending`, log `milestones mN set aside
resumed`) à `before_draw` ; entre-temps, but d'appoint. `achieved()`
reste « tous les jalons `done` » (jamais de fausse atteinte). Les
lecteurs d'« ouvert » (outils, objets gardés, niveaux planifiés,
demande, quotas de l'offre) filtrent avec `ambition::drawable` : un jalon
de côté n'achète ni n'immobilise rien et un jalon `demand:*` de côté ne
bloque pas une nouvelle demande. A3 (même décision) : un jalon `equip` ne bloque jamais sur un
objet que personne ne fabrique (`equip_wait::unmade_equip_piece` : un
`obtain` de la pièce, pas fait, ni détenu (sac, banque, porté) ni
obtenable maintenant — offre au carnet, boutique ouverte, voie de
production dont un bot de la flotte a le niveau,
`gear_plan::obtainable_now` ; règle générale, tout objet d'équipement,
l'amulette de pouvoir des mages n'étant que le cas vu en direct) : la
clôture (`GoalEnd::UnmadeGear`) rouvre le jalon sans compter de blocage
ni bannir de compétence, le met en pause 6000 ticks
(`MILESTONE_SHORTFALL_COOL_TICKS`, comme un manque du marché) et le bot
travaille ses autres jalons ; il retente à la fin de la pause (log
`milestone mN waits: <objet> is neither held, nor on the market, nor made
by anyone`). Échelle épuisée
(`achieved`) d'un bot au besoin (D3) : nouveau tirage au besoin, jamais de
prolongation (voir « D3 dans les vies ») ; pour les exceptions et les
épingles : nouveau tirage seulement si un autre bot tient le groupe
ET que 72 000 ticks sont passés depuis le dernier changement ; sinon
l'échelle se prolonge (`ambition::extend`, origine `extend` : les 3
derniers paliers tournent, niveau live + 5 plafonné à 99, quotas et
équipement répétés, cible de pièces doublée) ; rien à prolonger ⇒ nouveau
tirage (loggé). Un tirage qui ne donne aucun jalon ouvert est refusé
(`AmbitionError::NoMilestone`, loggé) et retiré (jamais une ambition
atteinte à la naissance, retirée à chaque passe) ; une ambition de
couverture se prolonge à la place (`new_ambition_extended`). Une nouvelle
ambition lit les pièces détenues (un teneur riche ne repart pas à 5 000).
Plus de changement facultatif (A2, 2026-11-21) : ni après un jalon fait,
ni à la clôture d'un but, ni quand `refresh` clôt un jalon niveau/pièces,
ni 1 chance sur 4 de retirage, ni « bot redondant comble un trou » :
`optional_change` et `coverage_repair` n'existent plus. Seules restent
les révisions de l'opérateur (épingle, teneur de marché retiré,
opérateur) et le tirage d'une ambition atteinte. Couverture
(`brain_ambition::coverage_pass`, toutes les 300 s par compte au passage
de lancement) : les trous de la flotte sont seulement loggés à chaque
changement (`fleet coverage gap: …` ; un bot fermé laisse son groupe) ;
aucune ambition en cours n'est retirée pour en combler un, le trou ne
sert qu'au tirage de la prochaine ambition atteinte.

Couverture de flotte (`assign_fleet`, pur) : au premier passage qui
rencontre une vie sans ambition, toute la flotte est assignée (fichiers
de vie ; niveaux du snapshot live, sinon de la sauvegarde moteur). Trous
= groupes qu'aucune ambition ne couvre en groupe PRIMAIRE (le rôle
`merchant` de couverture est supprimé en vague 3) ; glouton global : la
paire (bot, trou) de meilleur score part d'abord (score = adéquation
`(1 + avance/5)²` de la meilleure compétence du groupe sur la moyenne du
bot ; PV exclus de l'adéquation : ils partent de 10). La flotte = les comptes ayant un job dans le daemon
(hors `admin`/`bite`), relue à chaque lancement ; les vies de `run/lives`
sans job ne comptent pas. Au-delà de la couverture : tirage aléatoire
(graine, splitmix64) pondéré par demande × √adéquation × inclinaison du
trait, jamais un type opérateur ; les niveaux penchent le tirage sans le
figer (décision utilisateur « au-delà aléatoire » ; l'ancienne règle
`levels:<groupe>` faisait 9 guerriers sur 12). Inclinaison du trait
(levier utilisateur 2026-11-17 « entraînement marché plus fréquent »,
`ambition_fleet::affinity_tilt`) : ×2^(1,5 × (2a − 1) × (2s − 1)), a =
market_affinity, s = part de transformation du type
(`ambition_catalog::transform_share` : part des activités du catalogue
des guides qui entraînent la compétence primaire et que nourrissent des
intrants achetables, transformation de stock ou sort à runes, calculée
des données du jeu : forgeron, cuisinier, allumeur, artisan, prêtre,
mage, runier 1 ; guerrier, archer, bûcheron, pêcheur, mineur 0) ;
appliquée aussi au score de couverture (adéquation × inclinaison). Flotte
fabriquée de 76 profils neufs (test `ambition_fleet::market_tests`) :
buts de transformation 20 % sous 0,4, 53 % entre 0,4 et 0,6, 74 % à
partir de 0,6, les 12 groupes couverts. Log par compte : `brain <acct>: ambition <kind>
(<why>) — milestones …`.

Besoin seul (décision utilisateur D3, 2026-11-19, docs/plan-equilibre.txt
étape 6) : le tirage ci-dessus (adéquation aux niveaux) faisait gagner la
compétence déjà en tête (bûcheronnage en tête pour 43 bots sur 74, 28 %
du temps de la flotte contre 0,6 % d'artisanat) et la boucle
s'entretenait. Désormais le poids d'un type = son BESOIN seul
(`KindDemand::need`) × l'inclinaison du trait `market_affinity` (le
trait n'est pas un niveau). Besoin = part de la MASSE des offres d'achat
ouvertes qu'il sert (0..1), + part du SURPLUS qu'il consomme (débouché,
demande utilisateur 2026-11-19 « la prière, l'allumage de feux, la
magie, le tir à distance font disparaître des objets » ; relecture
tour 2 : en FENÊTRES D'ENTRAÎNEMENT, `ambition_catalog::outlet_windows` =
Σ par objet consommé de (unités que la flotte DÉTIENT au-delà des offres d'achat, banques comprises, depuis 2026-11-20 ; avant : unités au repos sur les annonces ÷ unités que
la méthode la plus rapide du type en brûle par 6000 ticks), puis ln(1 +
fenêtres) ÷ somme de tous les types ; objets et cadences lus des
méthodes d'entraînement du catalogue des guides, leurs flux du prior au
niveau d'ouverture sous `work_rate::CONSUMED_CAUSES` :
`ambition_catalog::consumption_of`, jamais une liste ; une méthode dominée par une autre voie (`dominated` : les 8 sorts de fonte, « ça coûte rien de faire fondre aux fours ») ne compte pas, le minerai en surplus appelle des forgerons, jamais des mages ; offres des
teneurs exclues, `market_demand::fleet_surplus_from`, voir docs/economy.md § Stock de la flotte : le même surplus choisit le PRODUIT des quotas, `ambition_supply`). Une seule compression,
ln(1 + fenêtres), parce que les stocks couvrent quatre ordres de
grandeur (live : 1 864 os = 0,7 fenêtre d'enterrement, 637 000 plumes =
2 600 fenêtres de pêche à la mouche) : en linéaire un seul objet
posséderait tout le signal ; additionner des unités brutes (plumes +
plaques) n'avait pas de sens, des fenêtres de travail s'additionnent.
Les jours de stock invendu du lot 0 ne sont pas lus (une lecture de
24 h du magasin par tirage ; les fenêtres disent déjà combien de travail
la pile représente), le tout × (1 + part de gain : meilleur profit du guide ÷
somme des profits de la flotte, linéaire, lu une fois). Offres et
surplus pèsent chacun 1 sur la flotte ; le gain n'INCLINE que ce qui est
demandé ou en surplus (×1 à ×2) : un métier ni demandé ni nourri de
surplus n'a aucun besoin, quel que soit son gain (relecture 2026-11-19 :
ajouté seul, le gain donnait 0,6 à 1 à chaque type contre 0,14 au plus
aux offres, un besoin aplati). `weight`, tirage au niveau des
exceptions, garde la part de gain linéaire sur le max. Puis
saturation. Le profit d'un type est celui que l'offre réelle permet
(`brain_d17::kind_profits` × `guide_money::supplied_share`, voir
§ guide money ; en live une ligne d'alchimie à 36 M tirait 21 mages sur
54). Masse d'un objet (`ambition_catalog::need_mass`,
revue du lot 6) = ln(1 + ln(1 + poids du signal)) : une offre aberrante
ne possède plus le besoin (en live, un souhait d'amulettes de puissance
à 1,3 M l'unité faisait 72 % de la valeur des offres ; une offre un
million de fois plus lourde pèse ~1,5 offre ordinaire ; un simple
ln(1 + poids) lui laissait encore ~2,6 offres) ; × `UNASKED_FACTOR` = 2
quand AUCUNE annonce de vente ne repose pour l'objet (D3 : rareté =
offres non servies ET annonces absentes ; lu dans le même carnet public
que les offres, `DemandSignal::asks` → `DemandItem::asked`, teneurs
compris, et les annonces du bot lui-même aussi : seules ses OFFRES
d'achat sont écartées, `market_demand::signals_seen_from`). Saturation (`ambition_fleet::saturate`) : besoin ÷ (1 + nombre
de bots de la flotte qui tiennent déjà ce type) ; le cerveau la calcule
sur les autres vies (`brain_ambition::need_demand`), `assign_fleet` sur
les types tenus et tirés dans la passe. `weight` (tirage au niveau des
exceptions) garde l'ancienne formule. Les niveaux ne jouent
plus aucun rôle : ni dans le tirage, ni dans le choix d'un trou de
couverture ni dans le score de couverture d'`assign_fleet`
(`gap_score` = (1 + besoin du type du trou) × inclinaison, départage
par la graine ; la réparation de couverture lit donc les poids de
demande quand un trou est ouvert). Un type sans besoin n'est jamais tiré tant qu'un
autre en a ; aucun besoin nulle part (marché fermé, premier cycle) :
besoins égaux, tirage uniforme × inclinaison (`why` = `random`). `why`
= `need:<objet>` quand un objet porte au moins `NEED_LABEL_FLOOR` = 5 %
du besoin DU TYPE (même parmi des centaines d'offres), sinon `need`.
Tous les jalons ouverts refroidis : log `every open milestone cools`
(but d'appoint), jamais « banned skill » sans compétence. (Le changement facultatif d'un bot au besoin, seulement vers un type
qui a du besoin, est retiré par A2.) Exceptions
(`ambition_fleet::LEVEL_LED_ACCOUNTS`, `level_led`) : Zezima, N0valyfe,
Yogosun et tout teneur de marché (archétype `MarketMaker` ou ambition
`market_maker`, `FleetBot::market_maker`) gardent le tirage d'avant
(demande × √adéquation × inclinaison). Tests :
`ambition_fleet::need_tests` (bûcheron 70 / artisan 30 : demande
d'artisanat seule → artisan à chaque graine, demande de bûches seule →
bûcheron, exceptions au niveau ; une offre de 78 M parmi 30 ordinaires
≤ 1,5/n de la part ; objet sans annonce > objet annoncé ; dix artisans
abaissent le besoin d'artisan ; étiquette seulement pour un vrai moteur).

D3 dans les vies (2026-11-19, « pourquoi on a toujours autant de cons qui
coupent du yew », `planner/src/ambition_review.rs`, cerveau
`brain_ambition::{review_pass, replace_stale_goal}`) :
- Marque : chaque ambition tirée porte la règle suivie
  (`ambition.draw_rule` = `need` ou `level`, posée par
  `brain_ambition::build` via `mark_rule`) ; une vie écrite avant la
  marque la lit vide (champ absent = ancienne règle).
- Plus de révision unique qui retire une ambition en cours (A2,
  2026-11-21) : le retrait au démarrage du daemon des ambitions sans
  marque `need` (révision `review:need`, `needs_review`, fentes
  `review_slot_ticks`) n'existe plus dans le cerveau. `review_pass` ne
  fait qu'adopter la marque `need` des tirages au besoin d'avant la
  marque (`why` `need` / `need:*`, `adopt_need_mark`). Une ambition ne
  change qu'une fois atteinte. Les fonctions pures de `ambition_review`
  restent (outil de simulation, fin d'échelle).
- Fin d'échelle (`keeps_ladder_end`) : hors exceptions et épingles, plus
  de prolongation (`extend`) de la même ambition ; nouveau tirage au
  besoin immédiat (révision `achieved`, sans exclure le type : le besoin
  peut redonner le même métier s'il lui reste des jalons ; le seul
  détenteur de son groupe le garde, `redraw_gaps`). Les exceptions
  et les épingles gardent la règle d'avant (prolongation, seuil de
  72 000 ticks).
- But remplacé (`goal_from_earlier_ambition`) : `revise` ne délie plus
  que les liens de jalon (`m<k>`) ; un but encore ouvert, sans lien vers
  un jalon de l'ambition courante, dont l'entrée `next:` est antérieure au
  dernier changement d'ambition et qui n'est pas un but `side:`, `bid:`
  ou `alch:` vient d'une ambition précédente (mila_wade, nina_rhodes :
  bûcheron → pêcheur à 18:24, « woodcutting 95 » encore coupé 2,5 h
  après). Au passage de lancement suivant sans nœud actif (le rapport
  d'abord), ses nœuds (numéro ≥ `memory.floor`) sont abandonnés
  (`lifecycle::abandon_goal_work`, cause `ambition changed: …`) sauf le
  nœud actif, un ordre de marché réellement posé au carnet (`market:*`
  dont la dernière entrée est `awaiting_fill` ; un achat jamais posé part
  avec le but), les marches d'équipement et le concierge ; le but se clôt
  comme un jalon retiré du catalogue (`done`, aucun ban) et le suivant
  est tiré de la nouvelle ambition. S'applique aussi après une réparation
  de couverture, et pour une ambition sans échelle (bot retiré vers
  `generalist`). Tests : `ambition_review::tests`,
  `brain_ambition::review_tests` (dont le cas mila_wade).

Voies dominées (décision utilisateur 2026-11-19, « pourtant ça coûte
rien de faire fondre aux fours » ; `ambition_catalog::dominated`) : une
jambe de production qu'une AUTRE jambe faite par une compétence domine
(intrants de l'autre ⊂ les siens, chaque quantité ≤, et elle en demande
strictement plus) ne fait pas servir l'objet à sa compétence, ni à ses
intrants : `serves` et `servable` ne lisent que les jambes non dominées.
Données du jeu seules, aucun prix. Une apparition au sol ou une ferme de
butin (sans compétence ni intrant) ne domine rien : ce n'est pas une
façon de FABRIQUER. Dans les données rev 274, seules les 8 jambes de
sort de fonte sont dominées (bronze, fer, acier, argent, or, mithril,
adamantite, runite : intrants du four + 4 runes de feu + 1 rune de
nature) : le mage ne répond plus à la demande de barres ni de pièces
forgées, le runier non plus par la rune de nature ; forgeron 1, mineur
½ inchangés. Aucun objet n'a deux compétences en voies non dominées
(le cas « alternative gardée » est testé sur des jambes construites :
`need_tests::only_a_superset_of_inputs_is_dominated`). Le poids au
niveau des exceptions lit le même `serves`.

Manque au marché (D3) : un but bloqué parce qu'un achat est resté non
servi ou a été refusé par le marché (`bans_scope::market_shortfall` sur
la dernière erreur de la cause) ne bannit aucune compétence ET ne compte
plus comme jalon bloqué : `brain_succession::close_goal` passe
`GoalEnd::MarketShortfall` à `brain_ambition::after_close`, qui rouvre le
jalon (`ambition::on_goal_shortfall` : `pending`, `blocked_count`
inchangé) et le REFROIDIT `MILESTONE_SHORTFALL_COOL_TICKS` = 6000 ticks
(décision utilisateur 2026-11-19 : entrée `cool:milestone:<type>/<id>`,
`ambition::milestone_cool_key`, historique `cool` ; `next_feasible_milestone`
le saute pendant ce temps, le bot travaille le jalon suivant ou un but
d'appoint ; sans cela « market rejected: buyer holds X coins » le
relançait à chaque cycle ; `goals::goal_closed` ignore les entrées
`cool`, écrites ici après la clôture). Log `milestone mN: market shortfall, not a
block; cooled 6000 ticks`. Avant :
49 changements d'ambition en 24 h après « jalon bloqué 3 fois », les
transformations d'abord. Tout autre échec compte toujours un blocage
(tests `brain_succession::a_market_shortfall_is_no_milestone_block`,
`a_market_shortfall_cools_its_milestone`).

Contrat du fichier (rendu par la page `life`, lu par les ventes) :
`ambition = {kind, label, groups, why: coverage:<groupe>|
random|demand:<objet>|need:<objet>|need, milestones: [{id, label, kind: level|produce|equip|coins,
target, status: pending|active|done|blocked, progress: [cur, target],
blocked_count, origin: ladder|demand:<objet>|extend}], drawn_at_tick,
drawn_at, rung, revisions: [{tick, at_secs, from, to, why}]}` et
`goal_origin`. `drawn_at_tick`/`tick` = horloge daemon (ms murales / tick
mesuré) ; les durées se mesurent sur `drawn_at`/`at_secs` (secondes unix ;
les fichiers d'avant le 2026-11-13 écrivaient `at`, toujours lu). Anciens
fichiers sans `ambition` : chargés tels quels, ambition assignée au
passage suivant.

Signaux de demande (`market_demand.rs`, lecture seule, données publiques) :
`signals()` → `DemandSignal {item, item_id, open_bids, bidders,
unmet_qty, best_bid, oldest_age_secs, oldest_age_ticks, weight}` (offres
d'achat au repos ; poids = meilleure offre × quantité non servie × (1 +
min(âge/6000 ticks, 5)), âge = celui de la LIGNÉE, t0 gardé par les
amends), `kind_profits()` (meilleure ligne du guide money précalculé
par type d'ambition, `brain_d17::kind_profits` : catalogue coté, sans
condition impossible pour tous, chaînes comprises ; chaque ligne comptée
pour le gain que l'offre réelle permet : profit × `guide_money::
supplied_share` = min, sur les intrants de stock d'une recette (D5 : le
guide général suppose le stock détenu) et sur tout objet acheté
(`prereqs.market`), de (annonces au repos + fenêtre des boutiques F2P,
`guide_engine::supply_of`) ÷ unités de la fenêtre ; la ligne elle-même
garde son économie (D22) ; live 2026-11-19 : `alch:rune_platebody`
36,2 M → 0,27 M, seules les ~14 plaques d'Oziach par fenêtre ; vide avant le premier
jeu ; 2026-11-17 : plus `chain_profits` ni la table `rates_profit`),
`kind_demand()` (entrées du planner :
`DemandItem` (avec `asked` : une annonce de vente repose pour l'objet),
`KindDemand` : `weight` = 1 + part de la VALEUR des offres servie + part
du meilleur profit (tirage au niveau des exceptions), `need` = part de
la MASSE servie (`need_mass`) + part du meilleur profit (tirage au
besoin, saturé ensuite), `need_item` = l'objet qui porte ≥ 5 % du
besoin). `DemandSignal::asks` = unités en annonce de vente au repos.

## Entraînement = investissement, ambition donnée par la valeur (plan progression A1 + A4, 2026-11-21)

Décision utilisateur du 2026-11-21 (`docs/plan-progression.txt`, lot 2). Deux
modules purs du planner, un branchement dans l'orchestrateur.

### A1 : une valeur commune, « gain par 6000 ticks, montée comprise »
`planner/src/investment.rs`. Pour un objectif P (objet demandé à `unit_price`,
ou l'alchimie d'un lanceur) et un bot aux niveaux `L0` :

```text
paliers    = (compétence, niveau) que la voie de P demande + ceux de chaque
             intrant QUE PERSONNE NE PRIXE (le bot doit le fabriquer lui-même)
montée     = Σ paliers au-dessus de L0 de  xp_between(L0, palier) / taux_xp_moyen   [ticks]
gain_tick  = flux de la voie par 6000 ticks (work_rate : produit au prix demandé,
             intrants au prix du marché, pièces) / 6000        (comme profit::chain_profit)
total      = min( gain_tick × (H − montée) , marge_unitaire × capacité )
valeur_6000 = total / (H / 6000)                    (0 si montée ≥ H)
```

- **Horizon `H` = `INVESTMENT_HORIZON_TICKS` = 720 000 ticks** (120 heures de
  production à 600 ms, 12 h au tick de dev de 60 ms). Assez long pour qu'une
  montée de 100 000 ticks n'en prenne qu'un septième, assez court pour qu'un
  niveau hors de portée (montée ≥ H) ne vaille rien. La montée ne rapporte
  rien dans le modèle (ses produits se vendent en plus : bonus ignoré).
- **Taux d'XP** : `best_xp_rate(compétence, niveau)` = meilleur XP par tick
  parmi les méthodes du catalogue que le niveau ouvre (`guide_methods`, taux
  dérivés des unités du dump, `work_rate`) ; la montée utilise la moyenne
  harmonique des taux en `from` et `to − 1`. Jamais une constante à la main ;
  pas de méthode = pas de montée chiffrable (`None`, pas de valeur par défaut).
- **Capacité** = unités voulues (quantité non servie d'une offre ; unités
  posées aux ventes pour l'alchimie) : une offre d'une unité à 1 M ne pèse pas
  une demande de 66 unités.
- **Paliers propres** (`own`) : un métier ne répond qu'aux étapes de ses
  compétences ; les intrants des autres sont achetés (artisan : seul le
  palier artisanat de l'amulette de pouvoir, pas l'enchantement magie 57).
- **Alchimie** (`alchemy_investment`) : lancers par tick
  (`alchemy::CAST_TICKS`) sur les offres qui rapportent après les runes
  (`Caster::profit`), palier 21 (basse) ou 55 (haute).

(a) **Choix de la ligne d'entraînement** (`brain_train::fill_ctx`) : une montée
en attente qui ouvre une voie lucrative (`brain_value::pending_investments` :
la demande non servie du marché, la compétence cible parmi les paliers à
grimper) relève le prix du temps du bot : `coin_rate = max(son gain mesuré,
valeur_6000 / 6000)` (`investment::time_value`). Une ligne qui coûte des
pièces mais fait gagner des ticks passe devant la ligne gratuite et lente
(test `a_lucrative_target_makes_the_bot_pay_for_a_faster_climb`) ; la ligne
choisie reste le tirage R7 (`train_score::pick`) sur ce score. Le log
`a tick is worth X coins (the climb opens P)` le dit.

### A4 : le cerveau DONNE l'ambition (plus de tirage au sort)
`planner/src/ambition_value.rs`, branchement `orchestrator/src/brain_value.rs`.
Mesure « b » : chaque demande non servie pèse la valeur A1 de la servir ;
fin de la double compression `compress` / `need_mass` pour ce choix (elles
restent pour les exceptions de niveau, `draw_kind`).

```text
valeur(bot, métier) = Σ lignes de demande P : serves(métier, P) × asked(P) × valeur_6000(P)   (montée DU BOT)
                    + alchimie (le mage)
                    + débouché des tas : min(ce qu'UN bot du métier brûle sur l'horizon,
                      tas restant après les autres affectés de la passe) × prix courant
score = valeur / (tenants du métier + 1), chaque ligne comptée sur sa capacité RESTANTE
        (non servi − unités déjà couvertes par les bots affectés pendant la passe)
```
- lignes de demande (toutes les sources de valeur du marché déjà lues par le
  cerveau, economy.md § Rentabilité) : les offres d'achat ouvertes (prix
  unitaire, unités restantes ; sans vente posée elles pèsent 1, avec ventes
  posées `ASKED_SHARE` = 0,5), les VENTES du marché (`market_demand::sale_lines` :
  par objet échangé dans les 24 h et fabriqué par une voie, les unités qu'un
  nouveau vendeur place sur l'horizon, `SALES_SHARE` 50 % du volume du jour,
  moins les ventes déjà posées au prix courant ou en dessous, au prix courant ;
  un objet qui a une offre garde sa seule ligne d'offre), l'alchimie des
  natures posées aux ventes ;
- **partage** : les bots déjà sur le métier au départ divisent sa valeur par
  `tenants + 1` (formule de l'utilisateur) ; les bots affectés PENDANT la passe
  couvrent la demande qu'on leur donne (diviser par `tenants + 1` seul
  empilait 28 bots sur le mage : une offre d'amulette à 1 M l'unité, un mage en
  fait 0,3 par tick pour 66 unités). Un bot ne fait qu'une chose à la fois
  (`cover`) : son temps (l'horizon) va aux lignes les plus denses de son
  métier d'abord, chacune prend les ticks que sa capacité restante demande ;
  quand les lignes sont couvertes, le bot suivant va à la demande suivante ;
- débouché des tas (règle utilisateur : les métiers consommateurs détruisent
  des objets, on règle les tas en consommant plus, jamais en récoltant moins) :
  dans le SCORE, valorisé au prix courant de l'objet consommé (1 pièce = valeur
  faible, voulu : les plumes pèsent peu, les bûches ou le poisson cru
  davantage) ; les bots affectés pendant la passe usent le tas ;
- avance du bot DANS le score : la valeur A1 se calcule par bot, le temps de
  montée part de SES niveaux actuels jusqu'aux paliers de la voie (un forgeron
  73-75 a une montée courte vers la forge, un débutant la paie entière) ;
- meilleur score gagne, déterministe, sans graine ; égalité (aucune demande ou
  tout couvert, scores nuls) : le moins de tenants, puis le débouché, puis
  le meilleur ajustement de compétences (`ambition_catalog::fit`), puis le nom ;
- **s'applique** (1) à chaque nouvelle ambition (`brain_value::draw` depuis
  `redraw`, `redraw_by_need`, `build`, `assign_kind` ; `ensure` :
  `assign_by_value`) pour tout bot hors exceptions ; (2) une passe unique de
  rééquilibrage initial.
- **Exclus TOUJOURS** : `PROTECTED_ACCOUNTS` (Zezima, N0valyfe, Yogosun,
  sans casse ; ils gardent le tirage « niveau » de D3), et toute ambition
  d'opérateur (`FleetBot::operator` : épingle, teneur de marché, alcher,
  overall). Leurs métiers comptent comme tenants, jamais réassignés.

### Couverture de chaîne (2026-11-21)
`planner/src/chain_coverage.rs`. Une demande dont la chaîne passe par une
étape que personne ne couvre (diamant : artisanat 70) n'est plus servie par
le métier de la dernière étape (enchantement, magie) : poids du terme × part
couverte des étapes que le métier n'entraîne pas ; l'étape non couverte du
métier porte toute la demande (`serves` = 1, montée comprise, A1).
Couverture binaire : un autre bot (hors protégés, opérateurs, teneurs de
marché) qui a déjà le niveau, ou dont une ambition a un jalon ouvert exactement
à ce niveau et qui l'atteint en moins de `INVESTMENT_HORIZON_TICKS`. Le
marqueur du rééquilibrage a changé (`a4c`) : la passe unique rejoue.

### Passe de rééquilibrage initial (seule exception à A2)
`orchestrator/src/brain_rebalance.rs`, appelée dans `brain_launch` après
`ensure`. Au premier passage sans marqueur `run/ambition_rebalance_a4c.json` :
plan de toute la flotte (`planner::rebalance` : meilleure paire (bot, métier)
d'abord ; un bot ne change que si le meilleur métier bat de plus de
`MIN_GAIN_TO_CHANGE` = +25 % la valeur de garder le sien, évaluée de même
(son métier partagé par ses tenants, sans la couverture des autres affectés) ;
chaque affectation compte comme tenant), attente que chaque vie active
(écrite depuis moins d'une heure) ait son job (`covers`), marqueur écrit avec
le plan, une ligne de log par bot avec sa raison chiffrée
(`brain rebalance <compte>: <avant> -> <après> (valeur …)`). Chaque bot applique
SA ligne dans SA passe (`assign_rebalanced`, révision `rebalance`, la vie est
sauvée par son propre thread), puis est marqué appliqué : redémarrage sans
effet, jamais de second plan. Le but de l'ancienne ambition est remplacé par
`replace_stale_goal`. Après la passe A2 tient (une ambition ne change qu'atteinte).
Simulation hors ligne sur une copie de l'état live : test ignoré
`simulate_rebalance_on_the_live_state` (`brain_rebalance_sim_tests.rs`,
variables `SIM_LIVES`, `SIM_DB`, `LIFESCAPE_SAVE_DIR`).

Les ventes sont lues dans la fenêtre des 24 h du carnet
(`Store::volumes_since`) ; l'horizon en jours = `H` × durée du tick
(`market_eta::live_tick_ms`).

## L7 : précisions d'implémentation

- Alternatives source-aware (`item_sources`, L1) : `farm` si la source est
  woodcut/mine/fish/drop (socle), `shop:buy` si un magasin PNJ stocke,
  `market:buy` si échangeable, non-Iron et couvert par la bourse (aucun
  ordre non couvert, même sans autre source). Les outils (hache, filet)
  ne farm jamais ; `farm:` sans activité productrice ⇒ pas de script
  (abandon typé, plus de repli poulets).
- Couverture : une jambe market/shop que la bourse (pack + banque) ne
  couvre pas est retirée avant le choix (même règle que le carnet).
- `market shopbuy` (nouveau) : achat PNJ via le helper partagé, avec
  retrait du complément en banque ; refuse bruyamment si fauché.
- (L7 : la revente marchande auto-sélectionnait le stock le plus valuable ;
  supprimée en vague 3, remplacée par les teneurs de marché.)
- Abandon en cascade : un nœud dont une dépendance est abandonnée est
  abandonné à son tour (jamais de pending silencieux infini).
- Robustesse superviseur (découverte L7) : un `job.inner` qui ne répond
  plus ne parque ni le superviseur (try_lock + report), ni le dispatch
  (`describe` contended), ni le market (pas de jobs verrouillé pendant
  les lectures). Cause racine non isolée (verrou sans détenteur visible,
  voir rapport de lot) ; le système dégrade au lieu de geler.
- R8 (live, partiel) : autopilote vérifié sur petits buts (obtain 50
  plumes, woodcut mon tour R1 en parité, shopbuy 5 plumes avec provenance,
  revente marchande 10913 ticks jusqu'à budget_exhausted, rôle supprimé
  depuis) ; blocked{hache} →
  sous-tâche goblins (D21 par drop) → abandon après 3 échecs (péage sans
  pièces). La montée pêche 40 + cuisine 30 + minage 30 + 1 quête reste à
  jouer en endurance (même boucle, objectifs plus longs).

## L8 : espace (protégé, D2, D12, D17)

`planner/src/space.rs` (pur) : `needs_cleanup` (banque ≥ 55 types sur 60),
`triage_bank` (pièces, nourriture jusqu'à 100 unités, puis keep-list),
`fallback_for` + `reserve_floor` (formule D2 : alchimie − runes > rachat
magasin, sinon magasin, sinon jet D10), `d17_order` (piles entières,
valeur totale croissante, ordres annulés). Le cerveau insère le nœud
`space_cleanup` (même passe, prêt aussitôt), verrouille
`space:cleanup` → `market cleanup --keep… [--iron]`, budget une fenêtre
de 6000 ticks. Le déclencheur précède le test de graphe terminal (une
banque pleine se nettoie même graphe fini) ; l'arrêt sur deux abandons
consécutifs borne les pannes persistantes sans bloquer les nettoyages
sains. Une passe qui sort sur budget avec la banque encore ≥ 55 types
est close `done` mais notée `cleanup_short` (`brain_fold::done_outcome`) :
aucun nouveau concierge pendant `CLEANUP_RETRY_SECS` = 900 s
(`space::cleanup_cooling`), jamais une relance toutes les 7 s (live
bdr53jee 2026-11-14 : quinze passes en deux minutes à 56 types). Le repli `await_fill` persiste son `done` (sinon l'obtention
ferme ne se termine jamais). L'activité worker (`activities/market_cleanup.rs`, sous le
plafond 200) : marché pour le surplus échangeable (lot et prix du
daemon, `seller_quote` : réserve = coût propre du bot, voir
`docs/economy.md` § Réserve vendeur et § Butin ; sans coût connu, rien n'est
listé), magasin
général, jet du reste, D17 jusqu'à repasser sous la limite : le surplus
que le carnet refuse (sans coût propre connu, hors bande) toujours — il
ne se vend jamais, seul son jet libère la case ; il n'est plus retiré de
la banque pour rien (il bouchait le sac et affamait les lots suivants) —
et le stock listé depuis une passe précédente, même quand la passe en
liste de nouveaux (2026-11-15, ryan_park : 3 piles fraîches par passe
sur 46 de surplus, D17 ne tournait jamais, banque bloquée à 60 types) ;
seules les annonces de la passe courante ont leur passe pour se remplir
(le non-listé reliste à la passe suivante, D12). Une pile non notée plus grosse qu'un sac part
sac après sac. Jambes alchimie décidées mais non exécutées
(pas de script de magie sur le socle : repli magasin, log).

## L10 : gate combat du cerveau

`planner/src/combat.rs` (pur) : niveau du bot (formule XP-max),
`min_combat_for_method` par méthode, `safe_area` et `combat_gate`
`Clear`/`Relocate`/`TrainFirst`, `training_method` (style du but,
poulets). Le cerveau (`maybe_launch`) évalue la gate au CB live avant
toute assignation récolte/transformation : `Relocate` lance d'abord un
nœud prioritaire `access:lumbridge` (même méthode après, one-shot),
`TrainFirst` un nœud prioritaire `reach_combat` (poulets, style du but)
avant tout trip, `Clear` inchangé. Garde : jamais d'assignation vers un
spot au-delà du CB (test `launch_never_dispatches_past_the_level`).

## L9 : profils et Iron

Tirage à la création (`draw_profile`, graine persistée, reproductible) :
`market_affinity` (pondère market vs farm, D15), `thrift` (rapide ↔
économe, R6 ci-dessus, `model::draw_thrift`), `price_patience`,
marge initiale `0.05 + patience × 0.15`, archétype producteur (le tirage
de 10 % de marchands est supprimé en vague 3 ; le teneur de marché se
crée exprès : `create-user --role market_maker`). Planner sans aucune
méthode `market:*` pour Iron (création + résolution, garde
`launch_allowed` au cerveau contre les vieux verrous). Le cerveau refuse
tout lancement market pour Iron et passe `--iron` (D2 direct, aucun appel
market). Les Iron n'ont jamais d'ordres : le carnet, les deals et les
agrégats holdings les excluent structurellement ; leurs rendements
comptent (même snapshot rates). Plafond 20 Iron (`iron_allowed`,
contrôlé à la création). Wilderness ouverte à tous (D13) : aucune
préparation, mort → replanning.

## PRD3 : résolution d'un besoin

`obtain_legs.rs` tire toutes les voies de `gamedata::obtain` : `market:buy`
d'abord, puis chaque production routée (`farm:`, `smith:`, `smelt:`,
`cook:`, `spin:`, `jewelry:`, `string:`, `mill:`, `runecraft:`,
`pottery:`, `craft:`, `gather:sheep/`, `pick:`, `fill:`, `tan:`, `milk:`,
`enchant:`), puis `shop:buy` sous D-A. `resolve_obtain.rs` : ask couvert
→ production faisable (niveau, outil) + offre permanente `market:bid` →
marchand → offre au repos → blocage typé (`missing:level:…`,
`missing:<outil>`, `missing:coins:N`). `record_block` transforme
`coins:N` en `have_coins` et `level:skill:N` en chaîne de paliers.
`have_coins`/`make_money` choisissent la méthode d'argent au meilleur
profit publié. Le repli de site (`place_on_site`) ne ressuscite jamais
une jambe d'achat refusée. `equip_best` se déplie depuis
`gamedata::gear::best_gear` ; distance et magie exigent arc + flèches /
runes (`required_tool`).

## Contrat d'arguments (2026-11-13)

Source : `rs/crates/planner/src/mapping.rs` (tests :
`mapping_contract_tests.rs`). Copie de travail pour l'agent scripts :
scratchpad `args_contract.md`.

Ordre d'une ligne de lancement : `<script> <positionnels> [--level <comp> <N>] [--spell <sort>] [--site x,z,niveau] [--wear <uniforme>]… [--want <objet> <N>] [toll] [boat] --node <id> [--budget-min <t> --budget-max <t>]`.

Drapeaux communs :

| Drapeau | Quand | Sens pour le script |
|---|---|---|
| `--node <id>` | toujours | corrélation du `task_report` |
| `--budget-min/--budget-max <ticks>` | engagements `make_money` | fenêtre d'engagement |
| `--level <comp> <N>` | nœud `reach_level` (N ≥ 2) | s'arrêter au niveau N de `<comp>` (récolteurs : leur compétence ; combat : la compétence du nœud) |
| `--want <objet> <N>` | nœud `obtain` (sauf `market`, `walkto`, `equip`, quêtes) | s'arrêter quand N unités de plus sont détenues (N = quantité − détenu au lancement) |
| `--site x,z,niveau` | le planner a choisi un site (`pick_site_gated`) ; tout script sauf `walkto`, `equip`, `quest_*` et `market buy|sell|make|cleanup` (seul `shopbuy` marche vers un marchand ; `make` reçoit son poste en `--post`) | travailler CE site (poste, rocher, eau, spot, magasin, apparition au sol), jamais un autre |
| `--wear <objet>` | le site est dans une guilde à uniforme (artisanat : `brown_apron`, cuisiniers : `chefs_hat`) | porter l'objet avant la porte (le planner l'a fait obtenir) |
| `toll` | site au-delà du péage d'Al Kharid, péage budgété ou gratuit (Prince Ali) | le script peut payer la porte et porte les tarifs de ses routes planifiées, rien si elles ne coûtent rien (`kit::toll::arm_fares`, `Bot::route_fare`) |
| `boat` | site à Karamja (`node.boat`), aller-retour budgété (60 po) ; seulement pêche, minage, bûcheronnage, combats | le script porte les tarifs de ses routes (aller + retour : 60 depuis Draynor) et peut prendre le bateau (`kit::toll::arm_fares`, `bot.fare_reserve`) |
| `--spell <sort>` | magie | lancer CE sort (le planner a dimensionné ses runes) |
| `--shop <clé>` | `market shopbuy` | acheter dans CE magasin (clé `gamedata::shops`) |

Blocages attendus en retour (`Blocked: missing …`, compris par `record_block_from`) : `<objet>`, `coins:N`, `combat:N`, `level:<comp>:<N>`, `quest:<clé>`, `access:<zone>` (`crafting_guild`, `cooks_guild`, `mining_guild`, `champions_guild`, `lumbridge`).

| Méthode planner | Script | Positionnels (dans l'ordre) | Drapeaux propres |
|---|---|---|---|
| `gather:fishing/<poisson>` | `fishing` | `<poisson>` (ex. `raw_trout`) | `--level fishing N`, `--site`, `toll`, `boat` |
| `gather:mining/<minerai>` | `mining` | `<minerai>` | `--level mining N`, `--site`, `--wear` (guilde), `toll`, `boat` |
| `gather:woodcut/<bûches>` | `woodcut` | `<bûches>` | `--level woodcutting N`, `--site`, `toll`, `boat` |
| `farm:<poisson>` | `fishing` | `<poisson>` | `--want`, `--site`, `toll`, `boat` |
| `farm:<minerai>` / `farm:coal` / `farm:clay` | `mining` | `<minerai>` | `--want`, `--site`, `toll`, `boat` |
| `farm:<bûches>` | `woodcut` | `<bûches>` | `--want`, `--site`, `toll`, `boat` |
| `farm:blankrune` | `essence_run` | — | `--want blankrune N` |
| `farm:<drop>` | `melee` | `<spot>` (spot non gardé le plus faible qui lâche l'objet) | `--want`, `--site`, `toll` |
| `fight:melee/<spot>` | `melee` | `<spot>` ou `ladder` | `--level <comp> N`, `--site`, `toll` |
| `fight:ranged/<spot>` | `ranged` | `<spot>` ou `ladder` | `--level ranged N`, `--site`, `toll` |
| `fight:magic/<sort>/<spot>` (ancien `fight:magic/<spot>` = `wind_strike`) | `magic` | `<spot>` ou `ladder` | `--spell <sort>` juste après le spot, `--level magic N`, `--site`, `toll` |
| `cook/<cru>` | `cook` | `<cru>` | `--level cooking N`, `--site` (fourneau), `--wear chefs_hat` en guilde |
| `cook:<produit>` | `cook` | `<cru>` (résolu depuis la recette) | `--want`, `--site`, `--wear` |
| `bake:bread` | `bake_bread` | — | `--want`, `--site` |
| `bake:<repas>` (`redberry_pie`, `meat_pie`, `stew`, `apple_pie`, `plain_pizza`, `cake`) | `cook_meals` | `<repas>` | `--want`, `--site`, `--wear` ; intrants en banque = composants de base (farine, eau, plat à tarte, baies…) |
| `smelt:<barre>` | `smelt` | `<barre>` | `--level smithing N`, `--want`, `--site` (fourneau), `toll` |
| `smith:<produit>` | `smith` | `<produit>` | `--level smithing N` (tranche au prix), `--want`, `--site` (enclume ; jamais celles de Doric ni de l'île du tutoriel) ; marteau porté |
| `superheat:<barre>` | `superheat` | `<barre>` (le script lance sur son minerai, charbon porté) | `--want`, `--level magic N` (minerais, charbon, runes de feu + nature retirés du sac puis de la banque ; bâton de feu porté, au sac ou en banque accepté) |
| `jewelry:<produit>` | `smelt_jewelry` | `<produit>` (nom d'objet : `unstrung_gold_amulet`, `nostringstar`…) | `--level crafting N`, `--want`, `--site` (fourneau) ; moule = outil porté |
| `string:<non_enfilé>` | `stringing` | `<non_enfilé>` | `--want` |
| `spin:<produit>` | `spin` | `<intrant>` (`wool`) | `--level crafting N`, `--want`, `--site` (rouet) |
| `pottery:<produit>` | `pottery` | `<produit>` (`pot_empty`, `piedish`, `bowl_empty`) | `--level crafting N`, `--want`, `--site` |
| `cut:<gemme>` | `cut_gems` | `<brut>` (`uncut_sapphire`) | `--want` ; ciseau = outil porté |
| `craft:<cuir>` | `leatherwork` | `<produit>` | `--want` ; aiguille portée, fil consommé |
| `tan:<peau>` | `leatherwork` | `<peau>` | `--want`, `--site` (tanneur), `toll`, `--wear` |
| `runecraft:<rune>` | `runecraft` | `<rune>` | `--level runecraft N`, `--want`, `--site` (autel), `toll` ; talisman = outil porté |
| `essence:<…>` | `essence_run` | — | `--want` |
| `fire:<bûches>` | `firemaking` | `<bûches>` | `--level firemaking N` ; briquet porté |
| `bury:<os>` | `bury_loot` | `<os>` (`bones`, `big_bones`) | `--level prayer N` (retire les os en banque, enterre, recommence) |
| `dye:<teinture>` | `dye` | `<teinture>` (`yellowdye`…) | `--want`, `--site` (Aggie) |
| `ground:<objet>` | `ground_pickup` | `<objet>` | `--want`, `--site` (apparition choisie) (ne ramasse que cet objet) |
| `telegrab:wine_of_zamorak` | `telegrab_wine` | — | `--want`, `--site` |
| `pick:<champ>` / `gather:wheat/<r>` | `picking` | `<champ>` | `--want` |
| `mill:<produit>` | `mill_grain` | — | `--want`, `--site` (trémie ; `--wear chefs_hat` si guilde) |
| `milk:<produit>` | `milk` | — | `--want` ; seau consommé |
| `fill:<récipient>` | `fill_water` | `<récipient>` | `--want` |
| `gather:sheep/wool` | `shear` | — | `--want`, `--site` |
| `enchant:<amulette>` | `enchant_teleport` | `<amulette>` | `--want` (amulettes et runes retirées de la banque, produits banqués entre charges) |
| `teleport:<sort>` (`varrock_teleport` 25, `lumbridge_teleport` 31, `falador_teleport` 37) | `enchant_teleport` | `<sort>`, puis `<lot>` = lancers si tranche au prix | `--level magic N` ; bouton du grimoire (`IF_BUTTON`), lancé en boucle quand l'ordre a un but (niveau, nombre, budget), une fois sinon (voyage) ; XP par lancer (unité S) |
| `curse:<sort>/<spot>` (`confuse` 3, `weaken` 11, `curse` 19, `bind` 20) | `curse` | `<spot>` (camp de la tranche : poulets puis vaches), `--spell <sort>`, puis `<lot>` | `--level magic N`, `--site`, `toll` ; cible fraîche (refus « already been lowered / held » = PNJ écarté jusqu'à sa régénération), agresseur abaissé achevé en mêlée ; XP par lancer, raté compris |
| `bananas:bones` | `enchant_teleport` | `bones_to_bananas`, puis `<lot>` | bouton du grimoire, un lancer convertit tous les os portés (24 par lancer au prix) |
| `fight:magic/crumble_undead/<spot>` | `magic` | `<spot>` mort-vivant | spots dont chaque PNJ porte `param=undead` (serveur) : `wild_skeletons` en surface ; refus `Usage` sur tout autre camp |
| `alch:<objet>` | `alchemy` | `<objet>` (le script choisit haute/basse selon son niveau), puis `<lot>` = lancers (tranche de magie au prix, `node.batch`) | `--level magic N` (tranche) ; runes : 1 nature + feu (bâton de feu accepté), exigées par `required_tool` ; méthode d'argent publiée par `brain_alch` (2026-11-14) ; le script lit aussi `--spell lowlvl_alchemy\|highlvl_alchemy`, `--want` ; cible retirée en billets, pièces banquées en fin de run |
| `market:buy` / `market:sell` | `market` | `buy|sell <objet> <N>` (N = manquant) | — |
| `shop:buy` | `market` | `shopbuy <objet> <N>` | `--shop <clé>`, `--site` (marchand), `--wear`, `toll` |
| `dialogue:buy` | `dialogue_buy` | `<objet>` | `--want <objet> N`, `--site` (vendeur), `toll` |
| `market:make` | `market` | `make --post x,z,niveau` | budget (poste = site du nœud, `ReadyCtx::maker_post`) |
| `space:cleanup` | `market` | `cleanup` | — |
| `quest:<clé>` | `quest_<nom>` (table `quest_script`) | — | clé inconnue = aucun script (abandon typé) |
| `access:<zone>` | `walkto` | `<x> <z>` | — |
| `equip:<style>` | `equip` | `<style>` | — |

Achats en dialogue : `dialogue:buy` ⇒ `dialogue_buy <objet> --want <objet> N --site <vendeur> [toll]` (une unité par conversation, réponses lues dans `gamedata::dialogue_shops`, pièces tirées de la banque d'abord, sac plein ⇒ banque).

## Guildes, nourriture, poisson de Karamja (2026-11-15)

- Site : parmi les sites ouverts, une guilde que le bot passe l'emporte sur
  les sites publics, puis le plus proche (`combat::pick_site_gated`) : un
  mineur ≥ 60 mine le charbon et le mithril de la Mining Guild (porte
  serveur `mining_guild.rs2` : mining ≥ 60), sous 60 jamais.
- Nourriture demandée par un bot de mêlée sans nourriture : le plus gros
  soin de `melee_ladder::FOOD_LADDER` jusqu'à la moitié de ses PV max
  (`food_to_ask` : 10 PV → hareng, 20 → thon, 24 → homard, 28+ →
  espadon ; crevette au minimum). Avant : toujours `shrimp`, d'où la
  demande de crevettes en tête du marché.
- Pêcheur : homard, thon et espadon sont des paliers (40 : homard/thon,
  50-60 : espadon/homard ; bateau routé). Cuisinier : homard à 50,
  espadon à 60.

## Ambition « overall » : premier au classement (2026-11-15)

Décision utilisateur : une ambition opératrice `overall` vise la première
place du classement overall, classé au total des niveaux puis à l'XP
totale à total égal. `create-user <pseudo> --goal ambition:overall`
(comptes Zezima, N0valyfe, Yogosun). **Toujours overall** (2026-11-21) :
un compte protégé sans overall (vie neuve, ancienne vie, ambition atteinte)
reçoit `overall` (`why` `protected:overall`, `ambition_fleet::draw_kind`,
`neediest_kind`, `assign_fleet`, `brain_ambition::before_draw`) ; une
ambition overall d'un compte protégé n'est remplacée par aucun chemin
(`ambition::revise` la refuse) et `ambition <compte> natural` est refusée
pour eux. Les teneurs de marché gardent seuls le tirage par niveau. Sans échelle (`ambition::ladderless`,
comme le généraliste) : chaque but tiré est la compétence F2P la plus
basse de `goals::OVERALL_SKILLS` (14 compétences ; les PV montent avec le
combat), jusqu'au prochain multiple de 5 au moins 5 niveaux plus haut —
une seule courbe d'XP, donc le niveau le moins cher du compte est
toujours celui de la compétence la plus basse ; égalités tournées par la
graine. Tenue pour épinglée (`pinned`) : jamais retirée, changée ni
réparée ; jamais « achevée » ; jamais tirée par la flotte
(`ladderless` exclu de `draw_kind` et `kind_weights`).

## Magie, ranged, demande servie, affluence (2026-11-15)

Demande utilisateur : « assure-toi que les gens entraînent leur magic et
range, et que le système motive les bots à produire les ressources
manquantes sur le marché et à acheter les trucs pas chers ».

- Échelle du mage : des paliers de niveau seulement (10, 20, 25, 30, 35,
  40, 45, 50, 55, 60 ; équipement à 10, 30, 50). Les quotas d'amulettes
  de magie sont retirés : leur chaîne crafting 13 (amulette de saphir,
  enfilage) bloquait six mages à magic 20, qui faisaient du melee en
  remplissage. Chaque montée choisit sa méthode la plus rentable
  (`training::best_pick` : sorts de combat, alchimie dès 21).
- Synchronisation du catalogue (`ambition::drop_stale_products`, appelée
  par `brain_ambition::sync_catalog` à chaque passe de lancement) : un
  jalon de production ouvert que l'échelle n'offre plus (origine
  `ladder`), ou un quota de demande que le métier ne sert plus, quitte
  l'ambition. Si l'objectif en cours venait de ce jalon, il est clos
  « done » (aucun ban) et le suivant est tiré.
- Demande servie hors échelle (`ambition_catalog::servable`) : un ordre
  d'achat rentable (`margin > 0`) devient un jalon de demande pour un
  métier dont une compétence de groupe produit l'objet en une étape, à
  partir de rien ou d'intrants que ses propres compétences récoltent, au
  niveau actuel du bot (forgeron : barres d'acier à smithing 30 ; jamais
  une amulette de magie pour un mage). L'échelle reste prioritaire.
- Entraînement au prix étendu (`training_production.rs`) : cooking,
  crafting, firemaking et prayer choisissent au lancement la recette
  ouverte la plus rentable (mêmes score et tri que la forge, intrants
  détenus/achetés/fabriqués au prix du bot, produit au prix que le marché
  absorbe ; seules les recettes qu'un script lance en montée de niveau).
- Choix de site selon l'affluence (`combat::pick_site_crowded`, registre
  `orchestrator/src/site_crowd.rs`) : chaque autre bot dont le dernier
  lancement travaille à moins de 15 cases d'un site ajoute 40 cases à
  sa distance (12 mineurs sur les 5 rocs de charbon de Barbarian Village :
  « no coal mined » 98 fois en 2 h). Les guildes restent prioritaires.


## Santé des méthodes, connaissances corrigées (2026-11-15)

Registre de santé (`orchestrator/src/method_health.rs`) : chaque rapport
replié compte pour sa méthode du planner (fin de travail = ok ; échec,
contrat, trajet = échec ; un blocage sur intrants ne compte pas). Sous
10 % de réussite sur au moins 20 lancements, la méthode est mise en
quarantaine (`run/method_health.json`) : le planner ne la résout plus
(`ReadyCtx::quarantined`, `method_feasible`, choix de méthode
d'entraînement), le log le dit une fois avec la dernière erreur, aucun
substitut silencieux. Un `check-method` live réussi après la quarantaine
la lève et remet les compteurs à zéro. Comptes staff hors registre.

Connaissances corrigées après les tests live :
- boutique : `source_flags` passe par `obtain::merchant_route` (une seule
  source de vérité) ; invariant testé sur tous les objets F2P : une jambe
  `shop:buy` a toujours un vendeur F2P au prix connu ; le multiplicateur
  passé à `~openshop` (Zaff) est dumpé (`shop_access.json`
  `sell_multiplier`).
- pêche : la seconde prise d'un palier s'ouvre à son propre niveau
  (hareng 10, saumon 30, thon 35 : `obtain::catch_level`) ; échelle
  pêcheur palier 10 = crevettes, sardines.
- cuisson composée : un composant que l'étape rend (`config_recipes`
  leftover : le moule du gâteau cru) est un outil, pas un intrant
  (`method_way::returned_by`) ; mélange du gâteau avec son moule.
- enchantement : `method_way` connaît `enchant:<objet>` (lignes spell des
  données serveur) ; le script dérive sa table des mêmes données (sort F2P,
  produit non membres : le collier de saphir devient un collier de jeux).

## Échelles d'ambition calculées (2026-11-15)

Les échelles ne sont plus des listes écrites à la main
(`planner/src/ambition_ladder.rs`, catalogue `ambition_catalog::specs()`) :
- métier producteur (bûcheron, pêcheur, mineur, cuisinier, artisan,
  forgeron, runier) : paliers tous les 10 niveaux jusqu'à 90 ; chaque
  palier propose les produits que la compétence a ouverts dans les 15
  derniers niveaux (sinon les meilleurs qu'elle fait), au plus 4, chacun
  une jambe de production de cette compétence sans quête, sans ramassage,
  sur un site F2P qu'un niveau de combat rend sûr (pas d'érable F2P, pas
  de blurite) ; quota = actions pour monter 5 niveaux au palier (XP du
  dump, médiane des produits), borné 10..200, arrondi à 5 ; le produit
  RETENU au palier suit le stock de la flotte (2026-11-20,
  `ambition_supply::pick_supplied`, docs/economy.md § Stock de la
  flotte) : un puits (produit ouvert au niveau du palier qui mange une
  pile couvrant son quota : poulet, bœuf, pêche à la mouche, cuir,
  fonte, runes) d'abord, jamais un produit dont l'outil est hors
  d'atteinte (talisman d'eau) ; règle appliquée aussi aux jalons déjà
  écrits (`revise_supplied`) ;
- métier consommateur (allumeur, prêtre) et combattants (archer, mage) :
  paliers de niveau, l'équipement un palier sur deux (prêtre : mêlée) ;
  guerrier : attaque, force, défense + équipement de 20 à 90 ;
- teneur de marché (phase de jeu) : 5 000 → 1 000 000 pièces.
La demande ouverte (jalon `demand:`) pèse tous les produits des
compétences de l'échelle à 10 niveaux près, pas seulement les 4 montrés
par palier. Invariants testés : chaque produit d'un palier est fait par la
compétence du palier à ce niveau ; chaque quota a une jambe praticable.

## Achats aux marchands (2026-11-15)

Règle utilisateur, remplace D-A : tout objet F2P qu'un marchand F2P stocke
peut lui être acheté (fabricable ou lâché par un monstre), la vente aux PNJ
reste interdite (`obtain::merchant_buy_allowed`). À la résolution d'un
obtain, la boutique passe d'abord quand elle ne coûte pas plus que l'offre
du marché et que le portefeuille paie (`resolve_obtain`, étape 0, prix du
serveur `merchant_price`) ; les étapes d'équipement prennent une offre
couverte, sinon la boutique. Le prix du marchand plafonne ainsi le marché
et retire des pièces du monde.

Boutiques rares (décision utilisateur Q7, 2026-11-16 : permises à tous les
bots). Champions' Guild : Scavvo (`runiteshop`, (3191,3351,1) : rune
longsword, sword, chainbody, platelegs, plateskirt, mace, coif) et Valaine
(`championshop`, (3193,3361,1) : adamant platebody, black full helm), à
l'étage derrière la porte 1805 (`champions_guild.rs2` : `%qp < 32`). Le
`Way` boutique porte `quest_points` = 32 ; `merchant_route` reste « ouvert à
tout bot », `merchant_route_for(objet, points)` et `shop_quest_points`
lisent la porte. Points d'un bot = somme des `reward_points` de ses quêtes
faites (`quest_requirements::quest_points_of`) ; `combat::Pass` ouvre
`Gate::QuestPoints` avec eux (`pick_shop`, sites), le nav avec `qp:32` (varp
101 transmis). Résolution : boutique listée seulement si le bot a les
points (`expand::shop_gate`) ; sinon, et rien d'autre, blocage
`missing:access:champions_guild` (échelle de points de quête). Un écart de
niveau d'une production passe avant (même règle que la boutique d'Oziach).
Plan d'équipement : une pièce que seule la guilde vend n'est voulue
qu'avec les points (`gear_plan::reachable_with`), `gear_plan::shop_route_open`
dit au plan si la pièce part en boutique. `market shopbuy` marche jusqu'à
l'étage du marchand (`walk_checked_at`). Lundail (runes de nature, cave de
l'arène des mages) : ni la cave (2534,4719) ni l'échelle (~3083,3963) ne
sont dans `free2play.csv` (Wilderness F2P jusqu'à z 3903) : un monde F2P ne
le fait pas apparaître (`GameMap.loadNpcs`) et refuse le telejump ; aucune
voie (goldens nav `lundail_hors_f2p`, `arene_mages_hors_f2p`). Davon
(`amuletshop`, Brimhaven) : stock 0, jamais une source.

## Nourriture abordable, quarantaine nommée (2026-11-17)

- Un combat (ou une ferme de drop) bloqué sur une nourriture
  (`Blocked: swordfish`) obtient la meilleure nourriture dont la bourse
  couvre une charge (`FOOD_BATCH`) au marché (meilleure offre, sinon prix
  courant), jamais au-dessus du soin adapté aux PV (`brain_food`,
  `melee_ladder::affordable_food`) ; rien d'abordable et coté : la
  nourriture demandée reste (production ou ordre posé). `need_count`
  donne la charge aux fermes de drop aussi (`combat::is_fight_method`).
- Une méthode en quarantaine (`method_health`) ne fait plus nommer son
  outil tenu comme manquant : le blocage dit `quarantined:<méthode>`
  (avant : `missing:harpoon` harpon en main, quatre obtentions « déjà
  tenu », abandon, pêche et cuisine bannies — kevin_ellis, espadon).

## Cycles de dépendances, nourriture produite (2026-11-17)

Cas live jim_ellis (10 PV, 29 pièces) : pas d'argent d'une hache financée →
`fight:melee/men` (max hit 1 = 10 % de ses PV : nourriture requise) →
`Blocked: herring` → obtention de hareng achat seul → `missing:coins:240` →
nouveau `have_coins` → hommes… 680 nœuds en 50 min, rien gagné.
- Choix de méthode (`planner::food_gate::ready_now`, argent, obtention,
  tranche) : d'abord une méthode faisable MAINTENANT (outil tenu, nourriture
  en main si le spot mord pour ses PV de base :
  `gamedata::melee_ladder::spot_food_required`, la règle des scripts),
  puis une faisable dont un prérequis reste à obtenir, puis le reste ; à
  classe égale le profit publié (taux saturés). `have_coins` : les drops de
  pièces ne départagent plus qu'à profit égal (avant : hommes/gobelins
  toujours en tête).
- Nourriture d'un combat bloqué (`food_gate::obtainable_food`, via
  `brain_food`) : le meilleur poisson ≤ soin adapté dont la bourse paie une
  charge ; sinon l'aliment (tout comestible) que le bot PRODUIT lui-même
  (`production_steps` : niveau, outils tenus ou achetables, intrants
  tenus/achetables/produits, chasse seulement sur un spot qu'il combat sans
  nourriture), le plus de PV par étape de production ; sinon l'aliment
  demandé. L'obtention de l'aliment choisi prend sa voie comme toute
  obtention.
- Cycle (`planner::lifecycle_cycle`, appelé par `record_block_from`) : un
  besoin qu'un nœud qui ATTEND le nœud bloqué vise déjà (même objet, pièces,
  niveau, quête, zone ; nœud bloqué lui-même, sauf montée de combat) est un
  cycle. Cassé au nœud le plus extérieur (jumeaux et répétitions du nœud
  bloqué, méthode bloquante connue d'abord) : sa chaîne jusqu'au nœud bloqué
  quitte le graphe, sa méthode bloquante (`Node::blocked_by`, ou celle du
  nœud bloqué pour une répétition) entre dans `Node::excluded` (jamais
  re-résolue), il se résout autrement. `FailureFold::CycleBroken(ligne)`,
  log `cycle broken: …`, historique `cycle_broken`. Un nœud d'argent qui se
  résout sous un nœud d'argent bloqué (graphes déroulés avant) s'effondre
  pareil (`coin_regress`, `brain_launch`).
## Refroidissements par méthode et site (P1, 2026-11-17)

Un nœud abandonné (échecs, contrat, trajet) refroidit, pour CE bot, ce qui
a échoué : `méthode@x,z,niv` (méthode seule sans site, ou quand un second
site de la même méthode échoue dans la fenêtre), entrée d'historique
`cool:<clé>|<ticks>|<cause>` (nœud `cool`, `COOL_TICKS` = 6000), lue dans
`ReadyCtx::cooled` (`brain_cool::active`). Une méthode refroidie est
inexploitable comme une méthode en quarantaine (`ReadyCtx::sick`, blocage
`cooled:<méthode>`), un site refroidi est sauté par `combat::pick_site_for`
(tous refroidis : `SitePick::Cooled`, jamais un besoin de combat). La même
erreur (chiffres à part) deux fois de suite sur un run < 100 ticks abandonne
tout de suite (`record_failure_timed`). Une racine pièces/argent ne bannit
que la compétence de sa propre méthode (`banned_scope`). Les buts d'appoint
et les réponses aux offres ne sont tirés que si une jambe de production
n'est ni en quarantaine ni refroidie (`side_goal_with`, `goal_feasible`, taux
0 dans `brain_bids`). Capacité de site et guildes : voir décisions D2/D4.
Les jalons d'ambition suivent la même règle (tour 3, point 5) :
`goals::draw_goal_origin_with` (quarantaine de la flotte, refroidissements
du bot lus dans son historique) tire `ambition::next_feasible_milestone`,
qui saute un jalon de production dont chaque jambe est malade ou entraîne
une compétence bannie ; le jalon sauté reste ouvert, tous sautés : voie
du but d'appoint (`MilestonesBanned`).

## Stock futur (plan marché M6, 2026-11-17)

`planner::stock_target` (pur) lit dans la vie les tranches de travail
du stock futur (Q5 : la courante et la suivante, `STOCK_SLICES`) :
nœuds ouverts `reach_level` (l'actif d'abord ; départ au niveau live
ou à la cible de la tranche ouverte précédente du même skill ; méthode
du nœud, sinon sa première alternative, sinon `tranches::reach_method`
du niveau de départ), `make_money` (engagement tiré, sinon moyenne
du tirage [6000, 18000]) et `have_coins` (une fenêtre de 6000). Durée
d'une tranche = `sizing::xp_between` ÷ XP /6000 de la méthode × 6000 ;
consommation = flux sortants des causes consommables du rendement
publié. `orchestrator::brain_stock` lit les rendements (`rates_read`),
le carnet et le portefeuille et pose les bids `stock` (voir
`economy.md` § Stock futur). `brain_sales::working_keep` garde l'appât
d'une récolte planifiée (plumes, appât de pêche :
`tools::required_tool(...).consumables`).

## Exécution 100 % market : `trade:craft/<recette>` (R5, 2026-11-17)

`planner/src/trade_craft.rs` (pur), côté démon `brain_trade.rs`,
`trade_report.rs` ; décision datée dans `docs/decisions.md`.
- Méthode d'argent `trade:craft/<recette>` (recette = activité d'une ligne
  market du guide : `smelt:bronze_bar`, `cook:trout`…). Les données du jeu
  la lisent par sa recette (`trade_craft::inner` : `method_way`, sites
  `combat::method_site_table`, `quests::method_quest`, activité mesurée
  `activity_of_run`) ; `mapping` lance le script de la recette avec
  `--want <produit> <lot>` avant les drapeaux du cerveau.
- Offres (`ReadyCtx::trades`, `trade_wallet`) : `brain_trade::publish`
  lit `guides_store::current()` (vue money de la tranche du bot dans le
  skill de chaque recette, lignes market seules : `market_rows`), les
  asks du cycle (`GuideSet::quotes`), les unités détenues et la bourse
  libre (`brain_gear::free_coins`). Une ligne n'est offerte que cotée
  (`priced`), rentable, produit ≠ pièces et intrants tous achetés au
  marché (`offer_from_row`), et à la portée du bot (`eligible` =
  `guide_feasible::feasible_for(ligne, bot).within_reach()`, D22 : bot =
  niveaux et combat live, quêtes, avoirs, bourse libre, asks du cycle
  `brain_trade::cycle_asks` ; faisable, ou après un outil ou des pièces
  seulement — le lot est borné par la bourse ; un intrant détenu compte
  dans sa disponibilité). Jamais pour un Ironman (ni offre, ni méthode, ni
  lancement : `space::iron_may_use`).
- Faisabilité d'une ligne de guide (D22, `planner::guide_feasible`) : les
  guides ne bloquent rien ; chaque ligne porte ses conditions (niveaux,
  combat, site à portée de banque, quêtes, points, outils, stock d'un
  lot, capital, Ironman, disponibilité de chaque objet acheté, sources
  manquantes) et `feasible_for` les juge pour un bot : `Feasible` |
  `AfterPrereqs(niveau, combat, quête, points, outil, pièces)` |
  `Infeasible(raisons)`. Seuls lecteurs du cerveau aujourd'hui : les offres
  `trade:craft` ci-dessus (R7 : brain_money et brain_train lisent les
  guides, § « Choix des bots dans les guides »).
- Choix (R7) : chaque offre est une ligne argent du guide (`brain_money` :
  la ligne market mappée vers son lot), tirée par `money_pick` comme toute
  méthode d'argent (classe « faisable maintenant » quand un lot tient :
  `workable`, outils détenus, ni quarantaine ni refroidissement) ;
  `pick_offer` supprimé. Sans guide : `money_methods` la classe sur son
  profit/6000 comme avant.
- Lot (`lot_size`) : min(volume d'une fenêtre de la ligne, demande
  partagée du produit — plan équilibre étape 2, `TradeOffer::demand` :
  bids + volume récent moins annonces au repos au prix et réclamations
  des autres —, profondeur des asks + unités détenues, bourse libre au
  coût du carnet — chaque unité au moins au prix courant) ; borne
  affichée `capital` | `depth` | `window` | `demand`. Aucune demande
  restante : aucune offre. Profit et prévu sur ce lot
  (`TradeOffer::lot_profit`) ; classement = net du lot par 6000 ticks de
  son craft plus UN achat (trajet + attente du marché, `lot_rate`), sur le
  lot que la bourse, les asks et la demande permettent ; docs/economy.md
  § Exécution 100 % market.
- Séquence : `lock` bloque `missing:<intrants>` (lot dans `node.batch`) →
  `record_block_from` dépose une obtention par intrant à jambe `market:buy`
  SEULE (`replan`, `trade_craft::buys_input` ; un outil manquant garde les
  jambes usuelles), comptée par `input_count` (lot × intrants ÷ succès :
  2 minerais par barre de fer) → intrants détenus : `sticky` reprend le lot
  acheté avant tout classement (`blocked_by`), lot = ce que le stock fait ;
  une offre nue de la recette (`recipe_offer`) le garde même si la ligne a
  quitté le guide → le run s'arrête au `--want`, ou sur ses intrants épuisés
  avec des produits faits (`lot_finished` : clôture, jamais un rachat) →
  nœud fait, vente par défaut (motif `trade`). Les intrants d'un lot ouvert
  ne sont ni vendus (`brain_sales::planned_methods`) ni jetés
  (`brain::cleanup_keep`, `trade_craft::open_inputs`) ; un pilotage hors
  bans ne vise jamais un lot (`steer_off_bans`), ni le repli de site.
- Mémoire : `LifeMemory::trades` (20 derniers `TradeLot` : méthode, lot,
  achats et coût planifiés, borne, profit prévu du guide, produits/6000 du
  guide, ouverture, produits faits et ticks du craft), écrite au verrou
  (`brain_trade::note_lot`, log `trade lot …`) et au rapport du craft
  (`note_made`).

## Contention : site = zone, méthode pleine (tour 2, P2-3, D6)

- Un refroidissement `méthode@site` couvre toute la zone de travail
  (`cooldown::same_area` : même niveau, `CROWD_RADIUS` = 15 cases, le rayon
  de la capacité et de la foule) : refroidir un roc du marais écarte tout le
  marais, et un second roc de la même zone ne refroidit pas la méthode.
- Capacité d'un site (`combat::site_capacity`) : ses nœuds de ressource
  dans `CROWD_RADIUS` ; pour un champ de rocs, les mineurs que ses
  repousses nourrissent (`method_rate_trip::miners_fed`, 2026-11-19, lot
  3 : 39 % du remplissage au charbon, Barbarian Village « contended » 38
  fois en 2 h, le deuxième mineur envoyé par une foule de 1 à 3 bots) :
  rocs × ticks par minerai d'un mineur au niveau du roc avec la meilleure
  pioche que ce niveau manie (`guide_methods::tool_at`,
  `pickaxe_checker.rs2` : rune dès 41 ; `stat_odds`, roll `rate + 1`,
  délai de succès) / `rock_respawn` (`mine.dbrow` : charbon 100, or et
  argent 200, mithril 400), arrondi, au moins 1, au plus le nombre de
  rocs. Charbon : Barbarian Village, Wilderness, Al Kharid 1, marais 2,
  Dwarven Mine 7, guilde 8 et 11 ; fer 1 à 4 ; étain 1 à 8 ; mithril et
  or 1. Arbres et pêche : leurs nœuds, inchangé.
- Places prises (revue lot 3) : seuls les bots sur la MÊME ressource
  (`combat::same_resource` : même genre et même ressource, `farm:coal`
  comme `gather:mining/coal`) remplissent la capacité d'un site
  (`ReadyCtx::seats`, registre `site_crowd` : site ET méthode du dernier
  lancement de chaque bot ; `pick_site_seated`) ; un combattant des
  barbares à côté des rocs de charbon allonge la marche (pénalité de
  foule, tous les bots) mais ne remplit jamais le charbon.
- `pick_site_avoiding` rend `SitePick::Full` quand TOUS les sites ouverts
  sont pleins (`site_full`) : jamais un site plein forcé ; un site gratuit
  plein ne bat plus un site payant libre. `place_on_site` passe alors à la
  méthode suivante du nœud (ordre des alternatives, rendement mesuré, D6) ;
  aucune libre : `Blocked full:<méthode>`.
- Un blocage de résolution `full:<m>` / `cooled:<m>` n'abandonne JAMAIS le
  nœud (tour 3, point 1 ; avant : `blocked at resolve (full:…) ->
  abandoned`) : `brain_cool::on_resolve_wait` refroidit `<m>` pour ce bot
  (`cool:<m>`) et le nœud reste prêt (`lifecycle::wait_open`) ; la
  résolution suivante prend la méthode suivante (D6). `<m>` déjà refroidie
  (toutes les méthodes le sont) : historique `cool_wait:<m>`, le nœud
  attend ouvert (`cooldown::waiting_nodes`, sauté par `next_ready_except`
  comme un achat en attente, un filler travaille) jusqu'à la fin du
  refroidissement.
- Scripts : pêche (« fish failed ») et sort de malédiction (« no foe took
  … ») rendent `contended:<site>` quand d'autres joueurs travaillent le site.
- Site épuisé (2026-11-20, décision utilisateur, détail `kit.md` § Site
  épuisé) : un nœud placé sur un site de roches ou d'arbres porte aussi
  `alt_site` (`alt_site::alternative_site`, même choix que `pick_site_for`
  depuis le site, lui-même évité ; remis à zéro à chaque résolution,
  écrit dans le fichier de vie), lancé en `--alt-site`. Le script attend
  la repousse si elle ne dépasse pas la marche vers ce site (plus son
  attente connue), sinon rend `depleted:<x>,<z>,<niv>,<ticks>` : le site
  est refroidi `<ticks>` pour ce bot, sans échec ni ban
  (`brain_cool::on_depleted`), historique `depleted`.
- Places engagées (2026-11-20) : les places d'un site comptent aussi les
  bots qu'une réponse à une offre y a engagés (`site_crowd::engage` au
  tirage, `disengage` au tirage suivant ; un bot dont le dernier
  lancement travaille déjà la ressource compte une fois) ; une réponse ne
  se choisit que sur un site à place libre (`bid_response`,
  `market_value::leg_site_seated`). Un but `bid:` dont un nœud attend
  ouvert plus de `BID_WAIT_RELEASE_TICKS` (6000) est rendu, sans ban,
  retour à l'ambition (`planner::bid_release`, `brain_bid_release` ;
  voir `economy.md` § Producteurs qui répondent aux offres). Un but de
  jalon attend toujours ouvert.
- Réponse dans le métier (2026-11-20, `bid_trade::answers_trade`) : une
  ambition tirée au besoin ne répond qu'aux objets que sa compétence
  principale fabrique (`ambition_catalog::serves` = 1) ; exceptions D3
  (`LEVEL_LED_ACCOUNTS`, sortes sans groupe principal, vie sans
  ambition) : tout le marché. `bid_trade::off_trade_answer` nomme un but
  `bid:` ouvert hors métier (état persisté avant la règle), rendu par
  `brain_bid_release::release_off_trade`.
  Une réponse tirée pendant que l'ambition attend un achat
  (`brain_wait_answer`, historique `bid_trade::WAIT_ANSWER_TAG`) pèse
  tout le marché et n'est jamais rendue comme hors métier.

## Passe de lancement : entrées typées et liste d'étapes (refonte neutre, 2026-11-23)

`brain_launch::maybe_launch` n'est plus une fonction de 840 lignes : c'est
une liste ordonnée d'étapes nommées sur un `LaunchPass` (handles, vie,
instantané live, entrées, `dirty`, `quoting`, attentes). Même décisions,
même ordre, mêmes lignes de journal, mêmes libellés `cpu_profile` qu'avant.

**Entrées (`planner::launch_inputs::LaunchInputs`).** Données simples, sans
type de l'orchestrateur : position, niveau de combat, `inv_streamed` /
`bank_streamed`, nombre de types en banque, piles (sac, banque, porté),
verrous du marché, quantités déjà mises en vente, pièces engagées par les
achats posés, foule et places des sites, marque de lancement récent, rapport
en vol ou en attente. Les aides de comptage (`raw_units`, `held`,
`listed_units`, `free_of_listings`, `family_held`, `free_now`,
`carried_tools`, `committed`) reprennent mot pour mot les formules des
anciennes fonctions (`live_units`, `holdings_count`, `listed_units`,
`free_tool_names`, `committed_coins`) ; les tests de `launch_inputs_tests.rs`
le prouvent. `brain_launch_inputs::gather` remplit la partie vivante une fois
au début de la passe ; `gather_market` relit ce que les actions de la passe
changent.

**Relectures conservées (`// lot 3:`).** Une action de la passe (inscription
en vente, enchère, retrait d'annonces) change les verrous, les annonces et
les pièces engagées : ils sont relus à la construction du contexte, avant le
test « déjà détenu », et après `release_own_locks`. Les drapeaux d'autres
fils (marque de lancement, rapport en vol) sont relus là où le détecteur
d'inactivité les lit. `take_operator_stop` et `first_idle_since_start`
consomment leur marque : ils restent appelés à leur place, jamais en avance.
La porte des intrants (`brain_input_gate`) garde ses lectures directes, car
elle retire des annonces entre deux comptages.

**Étapes (`brain_launch_pass.rs`).** Chaque étape rend `Flow::Continue` ou
`Flow::Stop(Save)` ; `Save` dit comment la vie est écrite : `None` (rien),
`IfDirty` (si la passe l'a changée), `Always` (`save_or_log`), `Silent`
(`save_life` sans journal d'erreur).

1. garde-fous : worker libre, vie, instantané live, bot échoué (`steps::open`)
2. entretien de la vie : opérateur, makers, ambition, libérations d'enchères,
   buts périmés, ré-engagement après redémarrage
3. espace en banque, vente PNJ, `normalize`
4. nœud en cours : attente de remplissage, rapport en vol, grâce après
   redémarrage, rapport perdu
5. porte du tutoriel
6. garde « sac streamé », passes de marché (`COMMERCE_STEPS`, lot 5 ci-dessous :
   réserve de quêtes, surplus, cotes du maker ou souhaits, alchimie, arbitrage,
   stock, outils, équipement), puis les attentes : besoins orphelins, achats
   parqués, quêtes, méthodes refroidies
7. état du graphe : en cours / bloqué / fini (succession)
8. nœud suivant (`brain_launch_node.rs`) : choix, déjà atteint, quête
   faite, construction du contexte de résolution
9. décision, effets, fin (lot 3, ci-dessous) : `brain_launch_drive::drive`

## La décision de lancement dans le planner (lot 3, refonte neutre, 2026-11-23)

Les étapes 8 et 9 mélangeaient décider et agir (appels au marché,
`release_own_locks`, `run_job`, journaux, sauvegardes). La décision est
maintenant dans le planner, pure ; l'orchestrateur ne garde que les entrées,
les effets et la sauvegarde.

**`planner::launch_decide::decide`** : une fonction pure (pas d'E/S, pas de
global, rien n'est modifié : la résolution tourne sur une copie du nœud et du
contexte) qui rend un `Decided` : le nœud, la prélude de résolution
(`Boundary` : nœud résolu et mémoire après le choix et le lot noté), le plan
d'expédition (engagement, progression, budget, instantané), les `Emit` (lignes
de journal et effets, dans l'ordre) et une `LaunchDecision` :
`AlreadyHeld`, `GearStepWithdrawn`, `CoinTargetMet`, `CoinCycle`,
`WaitUncounted`, `CooledWait`, `Abandon`, `Blocked{stage, missing}` (résolution,
intrants, nourriture), `RefuseIron`, `PlaceMarketBuy`, `NoScript`, `RunScript`
et les demandes `Need(Recount | Gate | Food | Loot | Cleanup)`,
`WithdrawOwnListings`, `WithdrawInputListings`.

**Demandes.** Ce que seul l'orchestrateur sait lire (annonces à retirer,
nourriture abordable, valeurs du butin, sources du « garder » du nettoyage,
comptes frais du marché) est demandé : l'appelant répond dans `Known` et
rappelle `decide`. La décision est déterministe ; `Known::resumed` reporte la
résolution d'un appel à l'autre (jamais refaite) et chaque `Decided::emits`
est un préfixe du suivant (l'appelant n'imprime que ce qu'il n'a pas encore
imprimé). Retirer ses annonces n'est donc plus un « retirer puis recompter »
caché dans une étape : c'est la décision `WithdrawOwnListings`, effectuée puis
re-décidée DANS LA MÊME PASSE (pas de passe chaînée en plus : `brain_chain`
n'enchaîne que si la vie a été sauvée, ce qu'une simple demande ne fait pas ;
re-décider dans la passe garde le même résultat final et aucun délai de lancement).

**`planner::launch_apply`** : `apply_to_life(&mut Life, &Decided, &ApplyEnv)`
écrit les changements de vie (`complete_node`, `record_wait_block`,
`park_for_fill` / `settle_market_buy`, `mark_launched`, `retry_uncounted`,
historique) et rend les lignes et effets qui suivent. Testable sans démon.
`apply_pre` (prélude + engagement + progression) est idempotent : l'orchestrateur
l'applique avant un effet qui lit la vie (enchère permanente, sources du nettoyage).

**Feuilles partagées** (une seule copie, dans le planner) : `launch_leaf`
(verdict de détention, besoins de pièces, butin laissé, liste « garder »),
`launch_note` (choix du guide et lot marché), `launch_inputs` (règle de
comptage : `entry_is`, `live_units`, `locked_in`, `free_count`,
`free_tool_names`, `node_counts`). `live_items`, `brain_locks`, `brain_plan::merge_keep`
y délèguent ; il ne reste plus de copie des formules de comptage côté
orchestrateur (sauf `market_audit::base_of`, hors périmètre : fichier `market_*`).

**Orchestrateur.** `brain_launch_node` (choix, contexte), `brain_launch_drive`
(boucle décider -> effets -> fin, générique sur `Fx`), `brain_launch_fx` (trait
`Fx` et `LiveFx` : marché, retrait d'annonces, `run_job`, créances, journaux,
sauvegarde). Libellés `cpu_profile` inchangés (`resolve` passe par le crochet
`Timed` de `decide`).

**Preuve d'équivalence** (`brain_launch_reference` + `brain_launch_equiv_tests`,
tests uniquement) : l'ancien chemin (étapes nœud et expédition) est recopié sur
un faux enregistreur des effets ; le nouveau chemin (`drive` sur le même faux)
est comparé à lui sur des vies croisées avec des entrées générées (détentions,
annonces qui verrouillent, porte-monnaie, pièces engagées, bans, Iron, banque
vue ou non, sites encombrés, jambes de combat, nettoyage, achat). On compare
la sauvegarde demandée, la vie obtenue, chaque effet et chaque ligne de journal
dans l'ordre, et le contexte vu par les effets. Neuf vies committées
(`tests/fixtures/lives`) tournent en CI ; les 84 vies réelles tournent avec
`LIFESCAPE_LIVES_DIR=run/lives cargo test --release --include-ignored the_decision_equals -- --nocapture`
(copiées en répertoire temporaire, jamais écrites).

## L'entretien de la vie et l'état du graphe dans le planner (lot 4, refonte neutre, 2026-11-23)

Les étapes 2 (entretien de la vie) et 7 (état du graphe) de la passe de
lancement mêlaient décider et agir. Chaque règle DÉCIDE maintenant dans le
planner, par une fonction pure sur la vie et des faits typés, et rend un
changement typé ; l'orchestrateur ne garde que la collecte des faits, les
effets (annuler des cotes, libérer un tableau, tirer un but, écrire une ligne)
et la sauvegarde. Même ordre des règles, mêmes lignes de journal, mêmes
libellés `cpu_profile` (`ambition.ensure`, `ambition.coverage`,
`stuck.effective_state`,
`rescue.market`, `stuck.on_stuck`, `succession.redraw`).

**Planner (quatre fichiers, aucun accès au démon, au magasin ni au SDK).**

- `upkeep.rs` : `Clock { now, tick_ms }` (l'horloge qu'une règle lit, mêmes
  ticks que `brain_stuck::seconds_to_ticks`) et la famille des rendus de buts,
  type `GiveBack` (cause d'abandon, tête du journal, entrée d'historique,
  oubli de l'origine, lignes du tirage qui suit) : `blocked_bid_give`
  (2026-11-20, réponse d'enchère dont tous les sites refusent), `off_trade_give`
  (2026-11-20, hors métier), `beyond_reach_give` (2026-11-20, au-delà de la
  montée), `unpaying_give` après `serve_query` (2026-11-21, but de demande qui
  ne paie plus le temps du bot), `milestone_query` puis `milestone_aside`
  (2026-11-21 et 2026-11-23, jalon de demande mis de côté et refroidi),
  `stale_goal_give` (D3, 2026-11-19, but tiré par une ambition antérieure) et
  `stale_work_line` (2026-11-22, travail ouvert de buts antérieurs). Deux
  temps : `apply_give_back` écrit le changement (abandons, normalisation,
  historique) et rend la ligne ; l'orchestrateur tire le but suivant ;
  `finish_give_back` normalise et rend la ligne du tirage.
- `upkeep_role.rs` : makers (`switch_cancels`, `apply_switch`, `retire_due`,
  `retire_cancels`, `apply_retire`, sur des `OrderFact`) et bascule de
  l'opérateur (`lift_role_bans`, `operator_step` -> `Done` / `Assign` sur un
  brouillon / `Alcher`, `operator_assigned`, `operator_drawn`,
  `operator_alcher`, `alcher_drawn`). Le brouillon ne remplace la vie que si le
  pas d'attribution a réussi (comme avant).
- `upkeep_ambition.rs` : `lacks_ambition`, `review_mark` (marque D3 unique),
  `sync_catalog` / `catalog_drawn` (échelle révisée, règle d'approvisionnement
  2026-11-20), la ligne du rééquilibrage A4 (`RebalanceRow`, `row_verdict`,
  `row_applied`) et `reengage_candidate` / `reengage_lot`.
- `upkeep_graph.rs` : `Spell` / `spell`, `goal_met`, `effective_state`,
  `stuck_summary`, `open_spell` / `spell_drawn`, `retry_drawn`, `revive`,
  `finished_closing` / `finished_drawn`, et le sauvetage par le marché
  (`market_rescue`, décision 2026-11-13) qui lit le marché à travers
  `RescueReader` (portefeuille, cote, détenu, pièces engagées : lus un nœud à
  la fois, comme avant) et écrit ses lignes par un puits `FnMut(String)`
  (elle lit entre deux lignes : l'ordre exact est conservé).

**Faits typés rassemblés par l'orchestrateur.** Pas un `UpkeepInputs`
monolithique : chaque règle a ses faits, lus au moment où elle les lisait
(les lectures coûteuses restent paresseuses : la marge et la valeur du temps
ne sont lues qu'après une requête non vide, les cotes du marché qu'un nœud à
la fois). Niveaux vivants (`level_of`), `BotNow` (niveaux, pièces, détenu),
`Clock`, `Stamp`, `ServeFacts` (marge, rendement par tick, valeur du tick du
bot), verdicts des jalons, `OrderFact` (carnet du maker), `RouteFacts`,
surplus de la flotte, ligne du plan A4. Le trait `UpkeepFx`
(`brain_upkeep_fx.rs`) les donne, `LiveUpkeepFx` les lit comme avant (les
chemins de prix ne prennent jamais le verrou du magasin ; aucun appel de prix
n'est fait sous `lock_store()`).

**Orchestrateur.** `brain_upkeep_drive.rs` : `operator_and_maker`,
`ambition_upkeep` et `graph_state`, génériques sur `UpkeepFx`, dans l'ordre
exact des anciennes étapes (les commentaires datés des règles suivent leur
logique). `brain_launch_steps` ne fait plus que les appeler. Les anciens
points d'entrée des modules (`release_*`, `apply_pending`, `review_at`,
`apply_row`, `goal_met`, `effective_state`) restent, en test seulement, comme
minces enveloppes de la même voie.

**Ce qui reste côté orchestrateur, et pourquoi.** Les tirages
(`brain_succession::{succeed_goal, close_goal, draw_next, redraw}`,
`brain_ambition::{assign_kind, assign_natural, redraw_neediest}`) lisent toute la flotte, le marché, les verrous et les
réclamations : un tirage n'est pas une décision de vie seule (lot ultérieur).
`brain_ambition::ensure` (attribue toute la flotte ; sa décision par vie,
`lacks_ambition`, est dans le planner), `coverage_pass` (table de
limitation par compte, ne change jamais la vie) et
`reengage_after_restart` (table mémoire des places ; la candidature par vie est
dans le planner). Les migrations uniques (marque D3, A4) ont quitté la boucle
au lot 5 (section « Migrations uniques », ci-dessous) : le plan A4 et son
marqueur (`brain_rebalance::pending_row`) ne sont plus lus à chaque passe.

**Preuve d'équivalence** (`brain_upkeep_reference` + `brain_upkeep_equiv_tests`,
tests uniquement) : l'ancien code (opérateur, makers, ambition, rendus, état
du graphe) est recopié sur un faux enregistreur du monde (`RecUpkeep`) ; la
nouvelle voie (`brain_upkeep_drive`) tourne sur le même faux. On compare le
drapeau « modifiée » et « maker en cotation », l'arrêt et la sauvegarde demandée
(état du graphe), la vie obtenue (nœuds, statuts, historique, mémoire,
ambition et jalons, engagement) et chaque lecture, effet, libellé de phase et
ligne de journal dans l'ordre. Vies croisées avec des entrées générées :
bascule d'opérateur en attente ou non, makers (rôles, capital, ordres),
ambitions absentes, de l'ancienne règle, à but périmé, révision du catalogue,
réponses d'enchère bloquées, hors métier ou hors de portée, buts et jalons de
demande qui paient ou non, travail périmé, graphe en cours, bloqué ou fini,
sauvetage possible ou non, interdictions et horloges d'expiration. (Lot 5 : la
ligne A4 et la marque D3 ne sont plus dans la boucle ; voir « Migrations
uniques ».) Neuf vies committées tournent en CI ; les
vies réelles tournent avec
`LIFESCAPE_LIVES_DIR=run/lives cargo test --release -p lifescape-orchestrator the_upkeep_equals_the_reference_on_the_real_lives -- --ignored --nocapture`
(copiées en répertoire temporaire, jamais écrites).

## Les passes de marché en liste explicite (lot 5, refonte neutre, 2026-11-23)

`brain_launch_steps::market_passes` enchaînait neuf appels au marché dans la
passe de lancement. Ils forment maintenant la liste nommée `COMMERCE_STEPS`
(`brain_commerce.rs`), appelée au MÊME point de la passe (après la garde « sac
streamé », avant les attentes), dans le même ordre, sous les mêmes libellés
`cpu_profile` et avec les mêmes lignes de journal. Rien ne change de
calendrier : ni nouvelle passe, ni exécution pendant que le worker travaille.

| # | étape (libellé `cpu_profile`) | garde |
|---|---|---|
| 1 | `quest_gate.publish` : objets des quêtes ouvertes réservés | toujours |
| 2 | `sales.list_surplus` : surplus mis en vente | toujours |
| 3 | `market_maker.quote_pass` : cotes de la tranche | maker en cotation |
| 4 | `wishlist.bid_wishes` : souhaits d'amélioration | pas en cotation |
| 5 | `alch.market_pass` : runes de nature, achats sous le plancher d'alchimie | pas en cotation |
| 6 | `arbitrage.pass` : offres sous le prix d'un marchand | pas en cotation |
| 7 | `stock.future_pass` : stock futur | pas en cotation |
| 8 | `tool_shop_pass` : kit permanent (hache, pioche, marteau, briquet) | pas en cotation |
| 9 | `gear_pass` : équipement | pas en cotation |

La garde « sac streamé » (`pack_streamed`) reste une étape de passe distincte
(`PASS_STEPS`) : liste de commerce et attentes ne la contournent pas. Les
attentes (besoins orphelins, achats parqués, quêtes, méthodes refroidies)
restent leur propre étape `waits`, juste après.

**Règle de l'utilisateur, écrite en code et en test : le commerce ne
préempte jamais le jalon actif de l'ambition** (jamais d'enchère
lucrative avant l'XP). Aucune étape de la liste ne change `life.goal`,
l'ambition et le statut de ses jalons, ni le statut, le genre ou la méthode
d'un nœud existant. Deux familles : les passes en lecture seule prennent
`&Life` (réserve, surplus, cotes, souhaits, alchimie, arbitrage, stock) : le
compilateur interdit de toucher la vie, elles ne font que poser, déplacer ou
retirer des ordres de marché (la comptabilité du marché, hors vie) ; les deux
qui classent des pas (`tool_shop_pass`, `gear_pass`, `&mut Life`) n'ont le droit
que d'AJOUTER des nœuds (kit permanent, équipement complet : règle de
l'utilisateur « chaque bot garde un kit de combat complet et la hache, pioche,
marteau et briquet du meilleur niveau ») et de retirer la chaîne d'un pas
d'équipement qu'ils avaient eux-mêmes classé et que personne n'a commencé
(`withdraw_gear_step`).

**Preuve** (`brain_commerce_tests`, invariant sur toutes les vies réelles
croisées avec des entrées générées) : `COMMERCE_STEPS` tourne pour de vrai sur
un `Job` portant un instantané généré (niveaux, pièces, sac, banque, porté ;
banque streamée ou non ; Iron ; makers en cotation ou non), trois tours de suite
sur la même vie (le deuxième voit le pas classé par le premier). Vérifié après
chaque tour : `life.goal`, `goal_origin`, l'ambition entière (jalons compris)
inchangés ; aucune entrée d'historique du but ; chaque nœud existant a le même
genre, statut et méthode (sauf la chaîne d'un pas d'équipement, retirable) ; tout
nœud ajouté est un pas d'équipement ou l'obtention d'un objet. Limite : un test
unitaire n'a pas de service de marché : les branches qui lisent le carnet (ask
couvert, enchère permanente) répondent « marché vide ». Les 9 vies committées
tournent en CI ; les vies réelles avec
`LIFESCAPE_ROOT=$(mktemp -d) LIFESCAPE_LIVES_DIR=run/lives cargo test --release -p lifescape-orchestrator commerce_never_preempts_the_ambition_on_the_real_lives -- --ignored --nocapture`
(copiées en répertoire temporaire, jamais écrites ; `LIFESCAPE_ROOT` détourné
pour que rien n'aille dans `run/`). Résultat réel : 74 vies, 2 960 scénarios, 8 880 tours des 9 étapes (847 en maker en
cotation), 4 979 nœuds classés, 0 violation (`LIFESCAPE_COMMERCE_VARIANTS`, 40
par défaut ; les comptes de test n'entrent pas dans la table de niveaux de la
flotte, `brain_gear_fleet`, pour ne pas fausser les autres tests).
Les harnais de vies réelles (lots 3, 4, 5) répartissent les vies sur 6 fils
(`par_harness.rs`, `LIFESCAPE_HARNESS_THREADS`), tallies fusionnés.

## Migrations uniques sorties de la boucle (lot 5, 2026-11-23)

Deux règles uniques tournaient à chaque passe de chaque bot :

- **Marque D3** (`upkeep_ambition::review_mark`, A2 2026-11-21 : « une
  ambition de l'ancienne règle est seulement marquée `need`, jamais
  retirée ») : ne lit que la vie. Appliquée hors ligne.
- **Rééquilibrage A4** (`row_verdict` / `row_applied`, marqueur
  `run/ambition_rebalance_a4c.json`) : la ligne de plan de chaque bot,
  appliquée une fois. Donner un nouveau genre tire une ambition sur la
  flotte, le marché et les niveaux vivants (`assign_rebalanced`) : impossible
  hors ligne.

**État réel (lecture seule, 74 vies, marqueur `a4c`).** Marque D3 : 0 vie la
demande (71 `need` marquées, 3 `operator` sans marque et hors règle). A4 :
71 lignes sur 71 appliquées, 0 en attente ; les trois comptes protégés ne sont
pas au plan. Les deux règles sont donc sorties de la boucle : plus de lecture
du marqueur (20 ko, relu et analysé à chaque passe de chaque bot), plus
d'appel à `review_mark`. Disparus : le plan de la flotte
(`brain_rebalance::{pending_row, plan_fleet}`), `brain_ambition::{fleet_bots,
fleet_names}`, les libellés `ambition.review` et `ambition.rebalance`, les
méthodes `rebalance_row` et `assign_rebalanced` du trait `UpkeepFx`
(`assign_rebalanced` ne reste qu'en test). Restent, purs : `review_mark`,
`row_verdict`, `row_applied`, `RebalanceRow`, le marqueur et ses décisions
(`step`, `covers`, `rows_of`, `write_marker_at`), lus par le tableau de bord.

**Commande** `orchestrator migrate-lives [--check]` (`migrate_cmd.rs`) : refuse
de tourner pendant que le démon tourne (comme `store-compact`), applique la
marque D3 hors ligne à toute vie qui en aurait besoin (une sauvegarde
restaurée) et sauve chaque vie changée ; `--check` liste sans écrire. Pour A4
elle RAPPORTE seulement les lignes encore en attente dans le marqueur (compte et
effet : rien, gardée, ou « donne », ce dernier demandant la flotte vivante) ;
un marqueur absent ou illisible est signalé : le plan n'est plus jamais refait.
À lancer après une restauration de sauvegarde, démon arrêté.

**Preuve d'équivalence** (harnais du lot 4, `brain_upkeep_equiv_tests`) : pour
chaque vie, (migration hors ligne puis boucle sans les règles) == (ancienne
boucle avec les règles), sur la première passe puis sur la suivante : mêmes
vies, mêmes lectures, effets, libellés et lignes (hors les deux libellés et la
lecture A4 retirés de la référence ; la ligne D3 de la référence est celle de la
migration) ; « modifiée » égal une fois la sauvegarde de la migration comptée.
Résultat réel : 74 vies, 99 900 scénarios d'entretien (deux passes chacun) et 44 400 d'état du graphe identiques. Le faux monde ne porte aucune ligne A4 (le marqueur réel n'en a plus). Deux
cas où la boucle ne marque pas ce que la migration marque, vies égales à
l'arrivée : l'ambition est remplacée dans la passe avant la marque (bascule de
l'opérateur, retrait ou bascule d'un maker). Hypothèse du test : tout tirage
marque l'ambition qu'il donne (`mark_rule`), et l'ambition d'un maker porte la
marque `operator` ; une ambition de maker sans marque (jamais dans l'état réel)
serait marquée par la migration alors que la boucle l'aurait laissée en cotation.
