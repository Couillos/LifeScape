# Kit de comportements (état cible)

But : scripts d'activité < 200 lignes = déclaration d'archétype +
particularités, exécutés par des briques partagées. Le brain décide *quoi*,
le kit sait *comment*, le script dit *ce qui est propre à l'activité*.

## Archétypes (couvrent tout le F2P)

Récolte sur décor / sur PNJ / au sol, transformation à un poste / en
inventaire, combat (mêlée, distance, magie). Hors archétypes : quêtes
(vocabulaire existant), banque, magasin, market, trajets.

## Briques (`scripts/src/kit/`)

`cycle` (boucle préparation → trajet → travail → retour, seul lecteur de
la `TaskSpec`), `tools` (rangs, remontage, `blocked{missing: outil}`),
`spots` (source branchable, filtre F2P, péage, occupation perçue),
`harvest` (clic, épuisement, repousse, case de travail), `produce`
(utiliser A sur B, adaptateurs compteur / choix / grilles), `survival`
(seuils, fuite, mort, prédicat de point sûr), `banking` (liste à garder,
billets), `gear`, `loot` (politique au prix de référence, provenance),
`fight` + style, `observe` (mémoire propre au bot, tâche courante seule).
Brique < 500 lignes, un contrat et des tests propres ; aucune ne décide de
la suite. Reste rangé sans changement de comportement : `pack_*`,
`walk_checked`, `BanSet`, `nearest_bank`, magasins.

## Règles

`kit/` (briques), `activities/` (une par fichier), `quests/`, `legacy/`
(transition). Plafonds vérifiés par test (< 200 activité, < 500 brique).
`run woodcut 3 toll wild` reste valable via parseur commun vers `TaskSpec`.
Décisions de vie dans `legacy/bootstrap.rs` jusqu'au brain, puis
supprimées. Chaque étape validée en live par les rendements avant/après.

## Contrat d'exécution

`cycle::run(spec, step)` : un pas = une charge (récolte) ; `poll_stop`
pour les files (mêlée) : cibles puis budget, fin différée au point sûr.
`--no-shop` coupe les achats (`bot.allow_shop`, refus au lieu de dépenser)
et les outils gratuits seuls comptent avant `blocked{missing}` (3 charges
sans hache, 2 sans pioche). La garde (`keep`) suit les voyages banque des
récolteurs ; l'équipement garde son jeu propre.

Pause inline générique : `pose_market` à côté du rescue, même porte à un
seul niveau, rescue prioritaire. Le market attend `at_safe_point`, rejoue
son script puis retourne marcher au point de départ ; l'horloge
d'engagement est suspendue (retour compris, `market_pause_ticks`).
