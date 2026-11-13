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
non compté ; fenêtre finie avec but atteint ⇒ `done`. Un nœud actif sans
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
une erreur `Usage` (avant : repli silencieux sur l'auto-pick).

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
next:<libellé>`. Sans ambition : AUCUN but (`NoGoal::NoAmbition`, typé et
loggé) ; le plus-bas-skill ne sert que le `generalist` explicite. Clôture
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
