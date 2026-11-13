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

Postes : le site est cherché autour du point `near` de la tâche (rayon
150) ; les stands sont les tuiles de côté de toute l'emprise du loc
(largeur × longueur du dump, tournée par l'angle live : `furnace1` 3×3,
cuisinière 1×2), jamais un coin ni une tuile sous le poste ; un refus
serveur (« I can't reach that! », forceapproach) passe au stand suivant.

Fonte (`activities/smelt.rs`) : poste = four F2P le plus proche du bot
(Wilderness seulement avec `wild`), `near` = ce four (pas la banque).
Une charge = des jeux complets de la recette serveur (bronze = 1 cuivre
+ 1 étain, acier = 1 fer + 2 charbon, mithril/adamant/rune 4/6/8
charbon) ; un intrant sous un jeu ⇒ `blocked{missing}` qui nomme chaque
intrant manquant (cuivre en banque sans étain ⇒ `tin_ore`), jamais un
clic de minerai seul (le serveur répond par sa boîte « need tin » et la
charge ne consomme rien). Forge (`smith.rs`) : un marteau en banque est
retiré avant tout `blocked{missing: hammer}`.

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
| R6 | crevettes, cuisinière (charge 28 crus, 290 ticks, niveau 1→5) | ~8690 (420 mesurés) | ~579 (28 plats : 14 shrimps + 14 burnt) |

Mesure R6 (2026-11-09, `admin`, stock initial par cheat `give raw_shrimp 28`
— pas de farm, réserve sur la provenance) : `run admin cook raw_shrimp 1`,
ticks 6194→6484 (290), cooking XP 0→420 (niveau 1→5, 14×30 XP, 14 brûlés sur
28 à niveau 1), 28 crus consommés, 28 plats banqués, sortie 0. A priori
(400 plats / 12000 XP) gardé jusqu'à 3 mesures (n=1).

## Banque lointaine, feux (2026-11-13)

`kit/withdraw.rs` : `bank_withdraw_items` marche jusqu'à la banque quand
elle est hors de portée (> 12 cases) et que la vue banque connue
(`inv_closed`, semée depuis la sauvegarde) contient l'objet ; rien de
connu en banque = pas de trek, 0 retiré. `ensure_tool` (pack puis banque,
sinon `blocked{outil}`) sert tinderbox, ciseau, aiguille, moule ; les
`fill_*` des postes passent par `reach_bank_for` (fini le `blocked`
d'un intrant rangé dans une banque à 20 cases).

Feux (`kit/fireline.rs`, règles de `firemaking.rs2`) : case légale =
non `WALK_BLOCKED`, aucune emprise de loc `active=1` (feu existant
compris, statique + live), F2P, hors des boîtes `bank_zones`
(`gamedata::fire_zones`, recopiées du dbrow et revérifiées par test).
Après chaque feu le serveur pousse le joueur d'une case à l'OUEST si le
pas est marchable (sinon est, sud, nord, sinon reste). Une charge cherche
une rangée de N cases légales vers l'ouest (N = bûches portées, au moins
4) dans un rayon de 14, s'y rend, allume, vérifie chaque feu en live
(« The fire catches », poussée ouest) et replanifie au premier refus
(« You can't light a fire here. », départ banni) ou rangée cassée.

## Exécutant mince : récolte (2026-11-13)

Contrat brain → récolteurs : la ressource visée en premier positionnel
(`fishing raw_trout`, `mining gold_ore`, `woodcut willow_logs`), `--site
x,z,level`, `--want <objet> N` (fin quand N de plus sont détenus, sac +
banque connue), `toll`. Le script ne pêche/mine/coupe QUE la cible (plus
de repli de palier ni de changement de méthode), la banque est celle du
site (`kit/site.rs`), et un pick vide marche jusqu'au site au lieu
d'attendre sur place (`run_load` → `reach_site` ; péage seul chemin sans
`toll` = `blocked{access:al_kharid_toll}`). Les écarts sont typés :
`blocked{level:<skill>:<N>}`, `blocked{quest:<clé>}`,
`blocked{access:<zone>}`, outils/appâts par nom exact
(`fly_fishing_rod`, `feather`, pack + banque connue comptés). Aucun
achat en magasin dans les scripts (filet, canne, plumes, appât, pioche,
hache, cisailles) : c'est la jambe d'obtention du brain.

Postes et combat : niveau manquant = `blocked{level:...}` (plus de
Usage), Rune Mysteries = `blocked{quest:runemysteries}`, le `--site` du
brain choisit le poste (`kit::produce`), `pottery <produit>` (pot, plat
à tarte, bol), `ground_pickup <objet>` au site, `bury_loot [os]` depuis la
banque. Un spot de combat nommé n'est jamais remplacé après une mort
(banque puis erreur de danger) et un spot derrière le péage sans `toll`
donne `blocked{access:al_kharid_toll}`. Les achats (`market shopbuy`,
accepte `toll` et `--site`) ne vont jamais seuls en Wilderness, sous
terre ni au-delà du péage. `--want` vaut aussi pour les boucles de
combat (`CycleState::begin`).

Lecture des ordres : `contract::check_order` lit une ligne de lancement
avec le parseur PROPRE du script (`cycle::parse_order` pour les
activités, `market::parse_order` → `parse_side`/`parse_shopbuy`/flip/
cleanup/deliver, `parse_style` pour `equip`, coordonnées pour `walkto`,
quêtes sans argument). `contract_tests.rs` traduit chaque famille de
`planner::mapping` (récolte, ferme, cuisson, `bake:` pain/repas, postes,
`cut:`, `dye:`, `ground:`, `telegrab:`, `superheat:`, `bury:`,
`fight:magic/<sort>/<spot>`, achats, 18 quêtes, 13 zones `access:`,
`equip:`) en ordre d'obtention (`--site`, `toll`, `--want`), en montée
(`--level`) et sur site de guilde (`--wear`), puis vérifie que chaque
positionnel se résout dans la table du script (repas, pièce de poterie,
os, teinture, gemme brute, barre de superheat, sort). Pièces manquantes :
`blocked{coins:N}`.

Drapeaux du contrat (2026-11-13, phase 2) : `--spell <sort>` (magie :
`bot.combat_spell`, `kit::strike` lance CE sort et bloque sur SES runes ;
un bâton élémentaire porté paie son élément, un bâton porté au sac est
manié d'abord, `kit::magic::{staff_pays, spell_shortfall,
wield_staff_for}` ; sort inconnu = Usage, niveau trop bas =
`blocked{level:magic:N}`), `--wear <uniforme>` (`kit::uniform` :
porté avant tout pas de `cycle::run` et avant la marche de `shopbuy` ;
sac, sinon banque du site, sinon `blocked{<objet>}`), `--shop <clé>`
(`market shopbuy` n'achète qu'à ce magasin, `kit::shop::buy_stock_at` ;
clé inconnue ou sans ce stock = Usage). `superheat <barre>` vise le
minerai de la barre (`ore_for` : `steel_bar` → `iron_ore`, charbon porté)
et s'arrête sur `--want`/`--level`/budget comme `alchemy` et
`enchant_teleport` (`poll_stop`). `mill_grain` prend la trémie la plus
proche du `--site`. Les ordres `market buy|sell|flip|cleanup` ne portent
plus `--site` ni `toll` (seul `shopbuy` marche vers un marchand).
