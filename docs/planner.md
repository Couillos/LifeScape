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
[10,15,20,30,40,55], cuisine [10,20,30,45], mêlée [10,20,30,40].

## Coûts (ticks équivalents)

`market:buy` = aller-retour place de Varrock + prix × quantité / taux de
gain ; `farm` = quantité / rendement × 6000 + trajets ; `craft` = intrants
+ quantité / rendement. Le trait `market_affinity` pondère market contre
farm (×(1,5 − affinité) : 0,78 à 0,72). Engagement `make_money` : tirage
[6000, 18000] au démarrage du script, dépendances exclues, pauses market
suspendues par le compteur (cycle + session). Replanning sur `blocked`,
`failed`, mort, perte, écart de prix > 25 %, fin d'engagement.

Pas de méthode `shop:buy` exécutable : les magasins PNJ bornent les prix
(plafond = prix magasin, repli `price_estimate`), les trajets d'achat
restent dans les scripts. `market:flip` = engagement sans objet (le
script `market` existant : achète sous la référence, vend le stock au-
dessus, annule les restes, sort sur budget).

## Contrat v2 (TaskSpec / TaskReport)

Inchangé (voir §5.3 du PRD) ; `cook` et `market flip` portent désormais
un `TaskStart` (budget + nœud), les jambes d'ordre (`buy`/`sell`) et les
quêtes restent minimales.

## Cerveau

`on_supervise` (un appel par passe) : plie le dernier rapport dans le
graphe (progrès, `done`/`blocked`/`failed`, sous-tâches `obtain` sur
`missing`, abandon au 3ᵉ échec), puis lance le prochain nœud `ready`
quand le worker est idle. Sans fichier de vie : aucune action (parité
R1-R7 des bots manuels). `create-user [pseudo] [--iron] [--goal kind]` :
pseudo (`Iron`+8, 12 max ; 20 Iron max), profil graine, squelette,
`open` + `tutorial`, puis le cerveau prend la main dès le premier idle.
`Iron` ne marchande jamais (ni `market:*` ni `flip`, même en `rich`).

Buts : `max:a,b:40` | `obtain:item:n` | `money` | `quest:clé` |
`equip:style` | `area:zone` | `rich[:merchant|:producer]`.

## Contournement tutorial (R8)

Le tutorial casse à l'étape `chop` (découpe live non rejouée ici) : les
runs R8 partent d'un compte existant post-tutorial (ou d'un tutorial
passé via `admin`), jamais d'un compte neuf. Le cerveau démarre au
premier idle : graphe, achats/ventes market, `blocked{hache}` →
sous-tâche, marchand, bonus de tendance — voir le rapport de lot.

## L7 : précisions d'implémentation

- Alternatives source-aware (`item_sources`, L1) : `farm` si la source est
  woodcut/mine/fish/drop (socle), `shop:buy` si un magasin PNJ stocke,
  `market:buy` si échangeable et non-Iron, plus repli market générique
  pour les objets sans source. Les outils (hache, filet) ne farm jamais.
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
