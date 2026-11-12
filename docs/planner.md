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
[10,15,20,30,40,55], cuisine [10,20,30,45], mêlée [10,20,30,40]. Chaque
tranche porte la méthode du palier ouvert à son niveau de DÉPART (1→5
crevettes, 5→10 sardines ; jamais le homard pour 35→40) : le contrôle
d'outils du planner porte sur ce que le bot pêchera vraiment. Pas de
méthode de substitution : outil manquant ⇒ `Blocked{missing}` ⇒
sous-tâche.

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
statut). `normalize` est appelé après chaque mutation et garantit :
dépendance abandonnée, inexistante ou en cycle ⇒ abandon transitif ;
`Ready` ⇔ toutes les dépendances `Done` ; un seul `Active`, égal à
`life.active` ; vivacité (graphe non fini ⇒ un nœud actif ou prêt).
`graph_state` : `Running`, `Launchable`, `Finished`, `Stuck` (une ligne
de log et `history: stuck`, l'opérateur décide : jamais de relance
automatique). Compteurs persistés par nœud : `failures` (max 3),
`blocks` (max 3), `last_error`.

Rapports (contrat v2) : tout lancement `--node` produit un `task_report`
(tuto, walkto, quêtes compris, échec de parsing compris) portant
`error: {type, message}`. Classement (`brain_fold::verdict`) : `Usage`
(arguments, dispatch impossible) ⇒ abandon immédiat ; `Failed`/`Sdk`/
mort ⇒ échec compté puis relance ; échec de voyage ⇒ méthode bannie +
échec compté ; `blocked` ⇒ sous-tâche `obtain` par objet manquant
(réutilisée si déjà ouverte) ; `stopped` (unload, redémarrage) ⇒ prêt,
non compté ; fenêtre finie avec but atteint ⇒ `done`. Un nœud actif sans
rapport (worker repassé idle, crash) compte un échec après 30 s. Une
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

Succession (`planner/src/goals.rs`, `brain_succession.rs`) : un graphe
fini tire le but suivant (compétence entraînable la plus basse, jusqu'au
prochain multiple de 5 au moins 5 niveaux plus haut) ; un graphe bloqué
écrit `goal: stuck:<compétences>` dans l'historique, garde les causes des
nœuds abandonnés, et passe à un but qui n'entraîne pas ces compétences ;
les buts d'argent relancent un engagement. Plus rien à tirer ⇒ attente
opérateur. Le libellé affiché est `fishing 10` (but `max:fishing:10`).

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
une erreur `Usage` (avant : repli silencieux sur l'auto-pick).

Buts : `max:a,b:40` | `obtain:item:n` | `money` | `quest:clé` |
`equip:style` | `area:zone` | `rich[:merchant|:producer]`.

Banc hors ligne : `brain_sim_tests.rs` rejoue 30 comptes (graphes réels,
issues aléatoires : succès, échec, Usage, blocage, arrêt, rapport perdu)
et vérifie vivacité, terminaison et scripts enregistrés.

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
