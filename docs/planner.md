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

## Entraînement au prix (2026-11-14)

`planner/src/training.rs` (pur) + `orchestrator/src/brain_train.rs`.
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
d'appoint) ; sinon le tirage ci-dessous.

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
sains. Une passe qui sort sur budget avec la banque encore ≥ 55 types
est close `done` mais notée `cleanup_short` (`brain_fold::done_outcome`) :
aucun nouveau concierge pendant `CLEANUP_RETRY_SECS` = 900 s
(`space::cleanup_cooling`), jamais une relance toutes les 7 s (live
bdr53jee 2026-11-14 : quinze passes en deux minutes à 56 types). Le repli `await_fill` persiste son `done` (sinon l'obtention
ferme ne se termine jamais). L'activité worker (`activities/market_cleanup.rs`, sous le
plafond 200) : marché pour le surplus échangeable (lot et prix du
daemon, `seller_quote` : réserve = coût propre du bot, voir
`docs/economy.md` § Réserve vendeur ; sans coût connu, rien n'est
listé), magasin
général, jet du reste, D17 jusqu'à repasser sous la limite : le surplus
que le carnet refuse (sans coût propre connu, hors bande) toujours — il
ne se vend jamais, seul son jet libère la case ; il n'est plus retiré de
la banque pour rien (il bouchait le sac et affamait les lots suivants) —
et le stock listé quand rien de neuf n'a été listé (le non-listé reliste
à la passe suivante, D12). Une pile non notée plus grosse qu'un sac part
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
| `alch:<objet>` | `alchemy` | `<objet>` (le script choisit haute/basse selon son niveau), puis `<lot>` = lancers (tranche de magie au prix, `node.batch`) | `--level magic N` (tranche) ; runes : 1 nature + feu (bâton de feu accepté), exigées par `required_tool` ; méthode d'argent publiée par `brain_alch` (2026-11-14) ; le script lit aussi `--spell lowlvl_alchemy\|highlvl_alchemy`, `--want` ; cible retirée en billets, pièces banquées en fin de run |
| `market:buy` / `market:sell` | `market` | `buy|sell <objet> <N>` (N = manquant) | — |
| `shop:buy` | `market` | `shopbuy <objet> <N>` | `--shop <clé>`, `--site` (marchand), `--wear`, `toll` |
| `dialogue:buy` | `dialogue_buy` | `<objet>` | `--want <objet> N`, `--site` (vendeur), `toll` |
| `market:flip` | `market` | `flip` | budget |
| `space:cleanup` | `market` | `cleanup` | — |
| `quest:<clé>` | `quest_<nom>` (table `quest_script`) | — | clé inconnue = aucun script (abandon typé) |
| `access:<zone>` | `walkto` | `<x> <z>` | — |
| `equip:<style>` | `equip` | `<style>` | — |

Achats en dialogue : `dialogue:buy` ⇒ `dialogue_buy <objet> --want <objet> N --site <vendeur> [toll]` (une unité par conversation, réponses lues dans `gamedata::dialogue_shops`, pièces tirées de la banque d'abord, sac plein ⇒ banque).
