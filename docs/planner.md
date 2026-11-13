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
20), magie [10,20,30,40] (le meilleur sort de combat ouvert au départ :
`fight:magic/<sort>/<spot>`). Chaque
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
tranche depuis le niveau live (jamais 1 ni 27).

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
objets non productibles en F2P (D-A, PRD3). `market:flip` = engagement sans objet (le
script `market` existant : achète sous la référence, vend le stock au-
dessus, annule les restes, sort sur budget).

## Contrat v2 (TaskSpec / TaskReport)

Inchangé (voir §5.3 du PRD) ; `cook` et `market flip` portent désormais
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
argent/quête/zone du but) `done` — est traité `Finished` (clos `done`, but
suivant), jamais une période de blocage (live : 540 `graph stuck`/h ;
bb4v5d9m bouclait « equip melee » fait toutes les 4 s, clos `stuck`, jalon
bloqué ×3, ambition guerrier ↔ prêtre toutes les 30 s).

Graphe bloqué (`brain_stuck.rs`) : une période de blocage = une ligne de
log + `history: graph stuck`, une seule fois. Le but courant est clos
une fois : `done` si les niveaux live l'atteignent (des nœuds abandonnés
plus anciens n'en font pas un échec), `stuck` sinon ; puis but suivant.
Plus aucun but ⇒ règle de relance (`lifecycle::revive_stuck`) : après
`REVIVE_COOLDOWN_TICKS` = 6000 ticks de blocage (1 h en prod, 6 min en
dev), chaque nœud abandonné (sauf les concierges `space_cleanup`, qui ont
leur propre arrêt) repart avec des budgets neufs et des jambes re-dérivées
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
non compté ; fenêtre finie avec but atteint ⇒ `done`. Progrès
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
commande market non remplie attend au plus 6000 ticks puis échoue.

Décision sans données interdite : sans position live, rien ne part.
Tuto (`brain_tutorial.rs`) : sur Tutorial Island (rectangles, pas les
labels de zone) seul le tuto part, en nœud rapporté ; clôture sur son
rapport ou sur preuve positive (bot vu hors de l'île sans tuto en cours) ;
tuto abandonné sur l'île ⇒ aucun lancement, l'opérateur décide.
`create-user [pseudo] [--iron] [--goal kind]` : pseudo (`Iron`+8, 12 max ;
20 Iron max), profil graine, squelette, `open` ; le verrou tuto lance le
tutoriel au premier passage, puis le but. `Iron` ne marchande jamais
(ni `market:*` ni `flip`, même en `rich`).

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
banni. Sinon le but clos bloqué bannit TOUT le sous-arbre de l'objet
(`banned_scope`) : compétences de la racine et de chaque nœud qui
l'attend, plus celles dont l'objet est l'outil (barres de bronze ratées
⇒ forge, bûcheronnage — la hache —, allumage s'il brûlait ces bûches).

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
une erreur `Usage` (avant : repli silencieux sur l'auto-pick). Ferme
par drop (`farm:<objet>`, `mapping::spot_for_drop` →
`gamedata::combat_spots::drop_spots`) : le spot NON GARDÉ le plus faible
dont un PNJ lâche l'objet (os ⇒ poulets), un spot à péage seulement s'il
est le seul (péage budgété, voir ci-dessous) ; jamais souterrain, clé ou
bateau (bâton de magie : moss giants ⇒ pas de jambe `farm:`). Les spots
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
la porte (3268,3227), cible de l'autre côté de x = 3268) : un site du
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
`sailors.rs2`), Crandor, souterrain, Wilderness. `combat::pick_site_gated`
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
`equip:style` | `area:zone` | `rich[:merchant|:producer]`.

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
`crafter`, `smith`, `miner`, `runecrafter`, plus `merchant` et
`generalist` (opérateur seulement, jamais tiré). Chaque entrée est une
échelle de paliers : niveau à atteindre puis quota de production
(plusieurs produits candidats par palier, choisis par la demande ouverte,
sinon par la graine : les bots d'un même type ne fabriquent pas tous la
même chose), tier d'équipement (`equip_best`) ou cible de pièces
(marchand). Exemple forgeron : minage 10, forge 10 puis 20 dagues/épées/
casques bronze, forge 18 puis 10 plastrons/jambières/boucliers bronze,
fer, acier, mithril. Les paliers dépassés de 10 niveaux sont sautés
(frank_lane, bûcheronnage 64, commence aux 150 bûches d'érable/saule).
Tous les produits ont une jambe de production (testé).

Jalons (`milestones`, 3 ouverts matérialisés d'avance) : `level` (clos
quand le niveau live l'atteint), `produce` (but `GoalKind::Produce
{item,count}` = un nœud `obtain` à jambes de PRODUCTION seulement, jamais
acheté ; intrants achetables via les sous-tâches ; la vente par défaut
liste le résultat une fois le nœud fini), `equip`, `coins` (merchant :
`GetRich{merchant}` jusqu'au montant détenu, pack + banque). Demande :
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
`blocked_count + 1` ; à `MILESTONE_MAX_BLOCKS` = 3 ⇒ nouveau tirage
immédiat de l'ambition (politique approuvée, log `milestone mN blocked 3
times, redrawing ambition`), jamais vers le même type. Échelle épuisée
(`achieved`) : nouveau tirage seulement si un autre bot tient le groupe
ET que 72 000 ticks sont passés depuis le dernier changement ; sinon
l'échelle se prolonge (`ambition::extend`, origine `extend` : les 3
derniers paliers tournent, niveau live + 5 plafonné à 99, quotas et
équipement répétés, cible de pièces doublée) ; rien à prolonger ⇒ nouveau
tirage (loggé). Un tirage qui ne donne aucun jalon ouvert est refusé
(`AmbitionError::NoMilestone`, loggé) et retiré (jamais une ambition
atteinte à la naissance, retirée à chaque passe) ; une ambition de
couverture se prolonge à la place (`new_ambition_extended`). Une nouvelle
ambition lit les pièces détenues (un marchand riche ne repart pas à 5 000).
Changement facultatif (après un jalon fait, à la clôture d'un but OU
quand `refresh` clôt un jalon niveau/pièces) : au plus une fois toutes
les `AMBITION_CHANGE_TICKS` = 72 000 ticks (temps mur converti par la
durée de tick mesurée) ; un bot redondant comble d'abord un trou de
couverture, sinon une chance sur quatre d'un nouveau tirage ; le seul
détenteur d'un groupe ne le quitte jamais. Réparation de couverture
(`brain_ambition::coverage_pass`, toutes les 300 s par compte au passage
de lancement) : les trous de la flotte sont loggés à chaque changement
(`fleet coverage gap: …` ; un bot fermé laisse son groupe) et un bot
redondant dont le dernier changement a 72 000 ticks prend le trou tout de
suite (`coverage_repair`, révision `repair:coverage:<groupe>` ; son but en
cours continue, délié).

Couverture de flotte (`assign_fleet`, pur) : au premier passage qui
rencontre une vie sans ambition, toute la flotte est assignée (fichiers
de vie ; niveaux du snapshot live, sinon de la sauvegarde moteur). Trous
= groupes (et `merchant`) qu'aucune ambition ne couvre en groupe
PRIMAIRE ; glouton global : la paire (bot, trou) de meilleur score part
d'abord (score = adéquation `(1 + avance/5)²` de la meilleure compétence
du groupe sur la moyenne du bot ; marchand : archétype marchand d'abord,
puis affinité marché ; jamais un Iron ; PV exclus de l'adéquation : ils
partent de 10). La flotte = les comptes ayant un job dans le daemon
(hors `admin`/`bite`), relue à chaque lancement ; les vies de `run/lives`
sans job ne comptent pas. Au-delà de la couverture : tirage aléatoire
(graine, splitmix64) pondéré par demande × √adéquation (archétype marchand
×5) ; les niveaux penchent le tirage sans le figer (décision utilisateur
« au-delà aléatoire » ; l'ancienne règle `levels:<groupe>` faisait 9
guerriers sur 12). Log par compte : `brain <acct>: ambition <kind>
(<why>) — milestones …`.

Contrat du fichier (rendu par la page `life`, lu par les ventes) :
`ambition = {kind, label, groups, why: coverage:<groupe>|
random|demand:<objet>, milestones: [{id, label, kind: level|produce|equip|coins,
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
min(âge/6000 ticks, 5))), `chain_profits()` (chaînes D58 de `chains.rs`
évaluées aux prix marché, jamais magasin), `kind_profits()` (chaînes +
profits mesurés par méthode), `kind_demand()` (entrées du planner :
`DemandItem`, `KindDemand` = 1 + part de la demande servie + part du
meilleur profit).

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
- `market flip` auto-sélectionne le stock échangeable le plus valuable
  (pack + banque, hors équipé) et vend les banknotes en un slot.
- Abandon en cascade : un nœud dont une dépendance est abandonnée est
  abandonné à son tour (jamais de pending silencieux infini).
- Robustesse superviseur (découverte L7) : un `job.inner` qui ne répond
  plus ne parque ni le superviseur (try_lock + report), ni le dispatch
  (`describe` contended), ni le market (pas de jobs verrouillé pendant
  les lectures). Cause racine non isolée (verrou sans détenteur visible,
  voir rapport de lot) ; le système dégrade au lieu de geler.
- R8 (live, partiel) : autopilote vérifié sur petits buts (obtain 50
  plumes, woodcut mon tour R1 en parité, shopbuy 5 plumes avec provenance,
  flip marchand 10913 ticks jusqu'à budget_exhausted) ; blocked{hache} →
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
sains. Le repli `await_fill` persiste son `done` (sinon l'obtention
ferme ne se termine jamais). L'activité worker (`activities/market_cleanup.rs`, sous le
plafond 200) : marché pour le surplus échangeable (plancher de réserve
aussitôt, descente par pas différée — `my_orders` sans âge), magasin
général, jet du reste, D17 sur stock listé seul (le non-listé reliste à
la passe suivante, D12). Jambes alchimie décidées mais non exécutées
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
`market_affinity` (pondère market vs farm, D15), `price_patience`,
marge initiale `0.05 + patience × 0.15`, archétype `get_rich`
producteur/marchand (10 % marchand, jamais Iron). Planner sans aucune
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
| `--site x,z,niveau` | le planner a choisi un site (`pick_site_gated`) ; tout script sauf `walkto`, `equip`, `quest_*` et `market buy|sell|flip|cleanup` (seul `shopbuy` marche vers un marchand) | travailler CE site (poste, rocher, eau, spot, magasin, apparition au sol), jamais un autre |
| `--wear <objet>` | le site est dans une guilde à uniforme (artisanat : `brown_apron`, cuisiniers : `chefs_hat`) | porter l'objet avant la porte (le planner l'a fait obtenir) |
| `toll` | site au-delà du péage d'Al Kharid, péage budgété ou gratuit (Prince Ali) | le script peut payer 10 po à la porte (`kit::toll::arm_fares`) |
| `boat` | site à Karamja (`node.boat`), aller-retour budgété (60 po) ; seulement pêche, minage, bûcheronnage, combats | le script garde 60 po dans le sac et peut prendre le bateau (`kit::toll::arm_fares`, `bot.fare_reserve`) |
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
| `fight:ranged/<spot>` | `ranged` | `<spot>` | `--level ranged N`, `--site`, `toll` |
| `fight:magic/<sort>/<spot>` (ancien `fight:magic/<spot>` = `wind_strike`) | `magic` | `<spot>` | `--spell <sort>` juste après le spot, `--level magic N`, `--site`, `toll` |
| `cook/<cru>` | `cook` | `<cru>` | `--level cooking N`, `--site` (fourneau), `--wear chefs_hat` en guilde |
| `cook:<produit>` | `cook` | `<cru>` (résolu depuis la recette) | `--want`, `--site`, `--wear` |
| `bake:bread` | `bake_bread` | — | `--want`, `--site` |
| `bake:<repas>` (`redberry_pie`, `meat_pie`, `stew`, `apple_pie`, `plain_pizza`, `cake`) | `cook_meals` | `<repas>` | `--want`, `--site`, `--wear` ; intrants en banque = composants de base (farine, eau, plat à tarte, baies…) |
| `smelt:<barre>` | `smelt` | `<barre>` | `--level smithing N`, `--want`, `--site` (fourneau), `toll` |
| `smith:<produit>` | `smith` | `<produit>` | `--want`, `--site` (enclume) |
| `superheat:<barre>` | `superheat` | `<barre>` (le script lance sur son minerai, charbon porté) | `--want`, `--level magic N` (minerais + runes de feu + nature dans le sac ; bâton de feu porté accepté) |
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
| `enchant:<amulette>` | `enchant_teleport` | `<amulette>` | `--want` |
| `alch:<objet>` | `alchemy` | `<objet>` | — |
| `market:buy` / `market:sell` | `market` | `buy|sell <objet> <N>` (N = manquant) | — |
| `shop:buy` | `market` | `shopbuy <objet> <N>` | `--shop <clé>`, `--site` (marchand), `--wear`, `toll` |
| `dialogue:buy` | `dialogue_buy` | `<objet>` | `--want <objet> N`, `--site` (vendeur), `toll` |
| `market:flip` | `market` | `flip` | budget |
| `space:cleanup` | `market` | `cleanup` | — |
| `quest:<clé>` | `quest_<nom>` (table `quest_script`) | — | clé inconnue = aucun script (abandon typé) |
| `access:<zone>` | `walkto` | `<x> <z>` | — |
| `equip:<style>` | `equip` | `<style>` | — |

Achats en dialogue : `dialogue:buy` ⇒ `dialogue_buy <objet> --want <objet> N --site <vendeur> [toll]` (une unité par conversation, réponses lues dans `gamedata::dialogue_shops`, pièces tirées de la banque d'abord, sac plein ⇒ banque).
