# Planificateur (état cible)

But : transformer un but long terme en graphe de tâches exécutables et
choisir la méthode la moins chère, sans toucher au socle une fois posé.

## Choix

Réseau de tâches hiérarchique (HTN) avec sélection de méthode au coût :
prévisible au dashboard, simple à implémenter, adapté au F2P (recettes
« pour avoir X, faire A puis B »).

## Nœuds

`reach_level`, `obtain_items`, `have_coins`, `make_money`, `complete_quest`,
`equip_best`, `access_area`, `space_cleanup`. Statuts : `pending` →
`ready` → `active` → `done` | `blocked{reason}` | `failed{error}` |
`abandoned`. Fichier : `run/lives/<compte>.json`, écriture atomique,
version de schéma.

## Batterie de tâches (règle D21)

Le brain fixe objectif (XP, niveau, quantité) et ressource exacte ; le
script trouve seulement *où* l'exploiter (choix du spot). Squelette complet
à la création, feuilles concrètes résolues au plus tard (prix et rendements
bougent). Table `méthode → (script, args)` en Rust, jamais en JSON.

## Coûts (ticks équivalents)

`market:buy` = aller-retour place de Varrock + prix × quantité / taux de
gain ; `farm` = quantité / rendement × 6000 + trajets ; `craft` = intrants
+ quantité / rendement. Le trait `market_affinity` pondère market contre
farm. Engagement `make_money` : 6000 à 18000 ticks, dépendances exclues,
interruptions : mort, rescue, deal market. Replanning sur `blocked`,
`failed`, mort, écart de prix, fin d'engagement.
