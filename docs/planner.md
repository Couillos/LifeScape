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

## Contrat v2 (TaskSpec / TaskReport)

`kit::cycle::parse_cli` lit la CLI (`--xp SKILL N`, `--level SKILL N`,
`--quest CLÉ`, `--budget-min/max N`, `--keep NOM`, `--node ID`,
`--allow-market`, `--no-shop` ; positionnels inchangés) en `TaskSpec` :
activité, ressource exacte, cibles (XP, niveau, quantité, quête), budget
ticks min/max, liste à garder, drapeaux market et achats PNJ, `node_id`.
`kit::cycle::run` boucle jusqu'aux cibles ou au budget (sorties `Ok(0)` =
but, `Ok(2)` = budget) ; `poll_stop` sert aux boucles à file (mêlée).
Le budget attend le point sûr (`kit::survival::at_safe_point`, 500 ticks
de grâce max) et ne compte jamais les pauses market (`bot.pause_ticks`).

La session mesure, le script ne déclare rien : diff pack + banque + équipé
et XP avant/après, provenances `produced` (XP concomitante), `loot`
(drop de PNJ), `ground_spawn`, `shop_bought`/`shop_sold` (couplage pièces),
`consumed` (nourriture, appâts, os enterrés), `lost`, `traded`. Sorties :
`goal_reached` | `budget_exhausted` | `blocked{missing}` | `failed` |
`stopped`, ticks, `market_pause_ticks` à part, XP et niveaux avant/après.
L'ancien `Result<i32>` reste accepté (rapport minimal). `ScriptError` et
`sdk::Error` portent `Blocked { missing }` : outil, intrant, nourriture ou
accès introuvables, jamais contournés.
