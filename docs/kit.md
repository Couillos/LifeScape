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

## Produce (transformation A sur B)

`kit/produce.rs`, premier client `activities/cook.rs` (cuire des
crevettes sur une cuisinière). Déclaration : site (locs en ordre de
préférence, ou PNJ) + adaptateur d'interface + intrant. Un pas
`cycle::run` = une charge (remplir de cru, tout cuire, banquer).

Adaptateurs (un par famille d'interface, jamais par activité) :

- `Direct` : aucun modal (cuisine : un clic = un produit). Attente de la
  consommation de l'intrant, re-clic par unité.
- `Choice { pick }` : dialogue chat1-chat4, choisit l'option contenant
  `pick`, pagine le texte courant sinon.
- `Count` : `P_COUNTDIALOG` → `RESUME_P_COUNTDIALOG` avec le reste à
  produire (0 = tout le porté) ; le serveur enchaîne, la brique draine
  sans re-cliquer.
- `Grid { button }` : ouvre l'interface (`smithing`,
  `crafting_jewelry`, `leather_crafting`, `tanner`), presse le bouton
  (nom com complet, ex. `smelting:com_19`), répond au compteur éventuel.
  La table produit → bouton reste côté appelant.

Particularités cuisine : cuisinières d'abord (brûlent moins), puis
cheminées, puis feux ; brûlés suivis comme produits (`Recipe.burnt`) ;
pas d'intrant manquant = `blocked{missing: [cru]}` ; poste introuvable
ou charge sans consommation = erreur forte, jamais de no-op silencieux.
Feux : le foyer de Lumbridge sert de repli (brûle plus, même boucle).

CLI : `run cook [cru] [charge]` (`cook` seul = crevettes, charge 0 =
infini). `cook 3` reste une charge comme les récolteurs.

A priori cuisine (remplacé dès 3 mesures) : 400 plats et 12000 XP par
6000 ticks (charge de 28 crevettes à 30 XP : banque, trajet, cuisson,
retour). Mesures R6 : voir rendements ci-dessous.

## Rendements live (référence, remesurés à chaque lot)

| Id | Tournée | XP / 6000 ticks | Objets / 6000 ticks |
|---|---|---|---|
| R6 | crevettes, cuisinière | — | — |
