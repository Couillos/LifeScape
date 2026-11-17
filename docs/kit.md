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
Tarifs (`kit::toll::arm_fares`, 2026-11-13) : `toll` (porte d'Al Kharid,
10 po) et `boat` (bateau de Karamja, 30 po chaque sens) se paient depuis
le SAC ; le run arme `bot.allow_paid` et une réserve
`bot.fare_reserve` que `bank_trip` garde et recomplète, décidée par les
routes planifiées (2026-11-17, `Bot::route_fare`) : aller + retour au
lancement (ici → `--site` → banque du site), banque → site → banque à
chaque nouvelle banque ; 0 sans transition payante. Sans `toll`/`boat`
aucune pièce de voyage n'est gardée ni retirée (le marcheur va chercher
le `coins:N` qu'une route exige), et l'armement meurt avec le run. Pêche, minage, bûcheronnage, mêlée, distance et
magie l'arment ; la banque d'un site de Karamja se choisit depuis le
quai de Port Sarim (`access::mainland_anchor`). `dialogue_buy <objet>
--want <objet> N --site …` achète par dialogue (`gamedata::dialogue_shops`).
Décisions de vie dans `legacy/bootstrap.rs` jusqu'au brain, puis
supprimées. Chaque étape validée en live par les rendements avant/après.

## Contrat d'exécution

`cycle::run(spec, step)` : un pas = une charge (récolte) ; `poll_stop`
pour les files (mêlée) : cibles puis budget, fin différée au point sûr.
`--no-shop` coupe les achats (`bot.allow_shop`, refus au lieu de dépenser)
et les outils gratuits seuls comptent avant `blocked{missing}` (3 charges
sans hache, 2 sans pioche). La garde (`keep`) suit les voyages banque des
récolteurs ; l'équipement garde son jeu propre.

Garde de progrès (2026-11-14, `kit/progress.rs`, boucles muettes de
`pottery` : ~1400 banques en 30 min, « ok load » sans un plat) : après
chaque pas de `cycle::run`, l'état est marqué (XP totale, détenu par objet
= sac + porté + dernière banque connue, ouvertures de banque
`Bot::bank_opens`, position). Progrès = XP totale en hausse ou un objet
au-dessus de son plus haut vu depuis le dernier progrès (un dépôt, un
retrait, un aller-retour qui rend ce qu'il a pris ne comptent jamais).
Sans progrès, `Failed("no progress: <activité> <motif> (0 product, 0 XP,
N bank opens, T ticks, <positions>) at …")` : 3 charges `UnitDone`
d'affilée, 6 ouvertures de banque sur au moins 2 pas (barattage), 12 pas
et 1500 ticks (reprises), ou 3000 ticks quel que soit le rythme ; les
positions de fin de pas disent l'aller-retour (`back and forth A <-> B`).
« ok load » : chaque activité finit sa charge par
`state.load_done(bot, …)`, qui logge `ok load N …` seulement si la charge
a produit, sinon `empty load N …` (la garde échoue à la 3e).

Eau (`kit/water.rs`) : un récipient d'eau est un outil. `mix_all` utilise
l'intrant sur les pleins portés, remplit les vides d'UNE sorte (celle dont
la source est la plus proche de la banque) à sa source la plus
proche de la banque (fontaine, évier, puits pour les seaux ; jamais
autour du site assigné) et recommence tant qu'il reste de l'intrant ;
aucun récipient en sac ni en banque connue ⇒ `blocked{bucket_water}`
(un seau se remplit aussi aux puits : celui d'Edgeville jouxte la banque ;
les fontaines de cruche les plus proches, au monastère, ne se rejoignent
que par le niveau 1 de la Wilderness). Un mélange envoyé pendant un
`p_delay` est jeté sans un mot par `OpHeldUHandler` : renvoyé (5 envois
de 6 ticks).
Poterie (`activities/pottery.rs`) : banque du `--site` (Barbarian
Village → Edgeville, jamais la banque proche du bot), 7 récipients (pleins
d'abord) + argile pour le reste du sac en une visite, `blocked{clay,
bucket_water}` en une fois, mélange, tour, four ; un ordre `piedish_unfired`
s'arrête à la tour (le cuire mangeait l'objet compté par `--want`) ; une
charge sans pièce = `Failed`. Les pièces déjà banquées au-delà du
mélange (crues d'un ordre cuit, argile molle) sortent d'abord, sans eau.
Live `admin` (2026-11-14, `direct`, Barbarian Village) : 14 argiles,
3 seaux remplis 3 fois au puits d'Edgeville, 14 argiles molles, tour
(côté ouest après refus est), four, 14 plats banqués en 740 ticks ;
banque vide ⇒ `blocked{clay}`.

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
(largeur × longueur du dump, tournée par l'angle du loc : `furnace1` 3×3,
cuisinière 1×2), jamais un coin ni une tuile sous le poste, jamais un
côté que le `forceapproach` serveur refuse (`stations::block_access`,
relu des `.loc` par test : cuisinière, tour et four de potier, fours
`furnace1/2` est, rouet sud, enclume de Doric nord ; tourné par l'angle).
Angle = flux live s'il a changé le loc, sinon placement statique
(`tiles_flags`) : le flux ne porte que les locs modifiés (2026-11-14, le
`furnace1` tourné de Falador lu à 0 refusait tous ses stands ; Al Kharid
essayait ouest, sud (torche : dossier NoRoute) et nord ~140 ticks par
charge avant l'est). Un refus restant (« I can't reach that! ») passe au
stand suivant, y compris avant une grille (l'attente de la grille
s'arrête au refus). La carte servie ne garde qu'un loc par case : un
mur sur la case du poste le cache (2026-11-14, tours de Barbarian
Village (3087,3409)/(3087,3410) : « no produce station potterywheel »).
Les postes viennent donc aussi du dump `stations` ; leur angle inconnu
⇒ tous les côtés offerts. Un clic refusé (grille jamais ouverte) est
loggé avec les messages du jeu, plus jamais un `break` muet.

Fonte (`activities/smelt.rs`) : poste = four du `--site`, sinon four F2P
le plus proche du bot (Wilderness seulement avec `wild`, jamais l'île du
tutoriel), `near` = ce four (pas la banque).
Une charge = des jeux complets de la recette serveur (bronze = 1 cuivre
+ 1 étain, acier = 1 fer + 2 charbon, mithril/adamant/rune 4/6/8
charbon) ; un intrant sous un jeu ⇒ `blocked{missing}` qui nomme chaque
intrant manquant (cuivre en banque sans étain ⇒ `tin_ore`), jamais un
clic de minerai seul (le serveur répond par sa boîte « need tin » et la
charge ne consomme rien). Jeux d'une charge (`load_sets`, 2026-11-14) =
min(place du sac, intrant le plus rare sac + banque connue, jeux restant
dus au `--want`) : 20 cuivres et 5 étains chargent 5 jeux, jamais 14
cuivres dont 9 inutiles ; le reste dû se divise par la chance serveur
(`recipes::success_odds` : fer 50 %, 2 minerais par barre due), jamais
au-delà de l'ordre. Forge (`smith.rs`, 2026-11-14) : enclume = celle du
`--site` (celle de Doric aussi : le cerveau a vu la quête), sinon
l'enclume ordinaire la plus proche hors Wilderness (`anvil_post`) ; barre
sur l'enclume depuis le côté le plus proche (« can't reach » ⇒ côté
suivant), puis UN appui Make couvrant la charge (`press::make_press`,
2026-11-16 : `inv_button1/2/3` = 1/5/10, le serveur 274 n'a pas de
make-X ; quand les barres limitent, le plus petit appui qui les couvre
— 8 écus de fer sur 24 barres = un Make 10, plus jamais 5+1+1+1 à ~16
ticks la visite de Make 1 ; quand l'ordre limite, le plus grand appui
dans le reste du `--want`, clous = 2 par appui) ; le serveur enchaîne
(5 ticks par objet) et s'arrête seul sur les barres (« not enough bars »,
fermée) ; la chaîne est suivie par `press::follow_chain` : finie dès les
objets faits ou les barres sous un jeu, plus 4 ticks de `p_delay`, une
coupure (niveau) après 12 ticks muets se ré-appuie pour le reste.
Mesure `admin` `smith:iron_kiteshield 8` : 84 → 43 ticks à l'enclume
(10,5 → 5,4 ticks par objet), rapport 374 → 333 ticks, sans alerte. Marteau et reliquat de barres gardés à chaque
banque (fini le marteau déposé puis re-retiré) ; un marteau en banque
est retiré avant tout `blocked{missing: hammer}` ; « complete Doric's
Quest » ⇒ `blocked{quest:doric}`. Sites planner (`obtain::station_open`) :
jamais l'île du tutoriel (four 3081,9496, enclumes 3075/3083) ni les
enclumes de Doric (aucune porte de quête ne porte un site).

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

Sorts (`kit/spell_load.rs`, 2026-11-13) : `enchant_teleport`,
`alchemy`, `superheat` et `telegrab_wine` comptent runes et cibles dans
le sac PUIS la banque connue (`banked_usable`), jamais le sac seul
(bd23dqpq bloqué 4 fois « missing: cosmicrune, waterrune » avec 2
cosmiques et 67 eaux en banque). `SpellLoad` = runes par sort + intrants
par sort (le premier est la cible ; superheat acier = minerai de fer + 2
charbons ; bananes = un sort pour tout le sac). Un bâton élémentaire
paie sa rune comme le compte le planner (`tools::family_held`) : porté,
au sac (manié) ou en banque (retiré puis manié, `ensure_staff`). Quand
le sac ne finance plus un sort : `bank_trip` banque les produits (runes,
cibles et bâtons gardés), puis `bank_withdraw_counts` retire en une
visite les quantités du plan (`SpellLoad::plan`), dimensionné par
`count`/`--want` restants, la banque et les cases libres (une pile = une
case, un non-empilable = une case par unité). `blocked{noms exacts}`
seulement si sac + banque ne financent pas un sort ; un run sans but
(ni count, ni `--want`, ni `--level`) qui a déjà lancé finit `ok` à sec.
Cible notée (`SpellLoad::noted_target`, alchimie, 2026-11-14) : la cible
sort de la banque en billets (`bank_withdraw_counts` reconnaît un id
`cert_` et passe en mode note), une case pour toute la charge ; billets
comptés comme sorts et gardés aux dépôts ; le sort vise la forme de base
tant que le sac en a, puis le billet (le serveur alche un billet à son
`oc_cost` de base). Cadence (`magic::CastPacer`) : `OpHeldTHandler`
jette sans un mot un `OPHELDT` reçu pendant `p_delay` ; le sort suivant
attend `cast_cooldown` = n + 1 trames après l'effet pour un `p_delay(n)`
(le `World` décode les paquets AVANT de lever le délai du tick) : cycle
de 5 ticks en haute alchimie (`p_delay(3)`), 3 en basse, superheat et
enchantements (`p_delay(1)`) ; un envoi sans effet en 8 ticks est
renvoyé (3 envois, puis échec avec la dernière ligne serveur) — avant,
chaque deuxième alchimie partait trop tôt et le run échouait « never
landed » 40 ticks plus tard. `alchemy` lit `--spell lowlvl_alchemy|
highlvl_alchemy` (sinon haute dès 55, basse dès 21 ; niveau trop bas ⇒
`blocked{level:magic:N}`).
Combat magique : le passage en banque (`gear::magic_ammo`) retire les
runes du sort ORDONNÉ (`--spell`), plus seulement air + esprit ; à court
dans le sac avec les runes en banque, `kit::strike` demande un voyage
banque (`Ok(false)`, comme un carquois vide) au lieu de `blocked`
(`magic::bank_shortfall`).

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

`--want` d'un récolteur (2026-11-14, paul_rhodes n60 : `obtain oak_logs
308` lancé `woodcut --site …` sans ses bûches par un binaire d'avant le
contrat, 3 649 bûches de saule en 25 runs finis au budget) : l'objet
compté EST la cible. `woodcut --want oak_logs N` sans positionnel coupe
du chêne ; un positionnel qui nomme un autre objet (`woodcut willow_logs
--want oak_logs N`) est un `Usage` (ordre croisé, le but ne bougerait
jamais). Le but s'arrête aussi en cours de charge : `CycleState::want_goal`
→ `LoadCfg::until`, `run_load` rend la charge finie dès que sac + banque
connue atteignent base + N (comme un sac plein), le script la banque et
le cycle s'arrête, plus un sac entier au-delà de N.

`--want` lu DANS la charge (`kit/want.rs`, 2026-11-17, live admin
`fishing --want raw_lobster 2` : 11 homards, le stand enchaînait jusqu'au
saut du spot). Même convention (sac + banque connue, base au lancement),
une primitive pour toutes les boucles : `want::load_done` (sac plein OU
but atteint), `met`, `left` (unités dues, `u32::MAX` sans `--want`),
`cap` (borne un make-X, un retrait, un appui), `stage_bounds` (étape de
transformation), `inputs_for` (intrants pour N sorties). Sans `--want`,
rien ne change.
- Stands (`harvest::watch_stand(…, until)`) : le gain qui atteint le but
  finit le stand comme un sac plein (`WorkOutcome::Full`) ; `run_load`
  copie `LoadCfg::until` dans `LoopState::until` pour pêche, minage,
  coupe ; essence, traite, cueillette, tonte passent le but de leur
  `CycleState`. Fin : la charge est banquée (récolteurs à charge) puis
  `goal_reached` ; traite/cueillette/tonte s'arrêtent au `poll_stop` du
  pas suivant (sac compté, comme avant).
- Transformations (`ProduceJob::until`) : un but sur un produit de
  l'étape est relu à chaque clic (cuisson, pain, repas, filage, teinture,
  remplissage, bijoux, fonte : un brûlé ou un fer raté ne compte pas, la
  boucle continue) et chaque réponse make-X demande au plus le reste ; un
  but sur une étape suivante (argile façonnée pour un plat cuit) borne
  l'étape une fois. Pain : farine/eau retirées ≤ reste ; poterie : pièces
  prêtes + argile ≤ reste ; runes : essence ≤ ⌈reste / runes par
  essence⌉ (`action_odds::runes_per_essence`).
- Boucles par objet : taille de gemmes, enfilage, couture (appui
  `make_press(cuir, reste)`), moulin (passes ≤ reste), ramassage au sol,
  moules d'or s'arrêtent dès le but.
- Combat (`farm:<drop>`) : `HuntPlan::want(goal)` → `HuntStop` relu entre
  deux adversaires (après le butin), comme `--level`.
Dépassement inévitable : une action en cours (une prise multiple, un
autel qui lie toute l'essence portée, une pression déjà lancée, un butin
de plusieurs unités). Déjà bornés avant : forge (`press::make_press`),
fonte (`load_sets`), sorts (`spell_load::casts_left`), tannage, achat en
dialogue.

Étages (2026-11-14, 4-5 bots bloqués au sommet du moulin de Lumbridge
(3165,3306,2)) : aucune banque F2P n'est à l'étage en rév. 274, donc
`nearest_bank`/`nearest_bank_at` (`gamedata::banks::nearest_free_bank`)
prennent une banque du même niveau si elle existe, sinon celle du sol ;
`bank_trip` descend par les échelles du routeur. `reach_site` marche au
NIVEAU DU SITE (`skill_walk::walk_checked_at`), jamais au niveau du bot
(avant : (3085,3230,2) demandé depuis l'étage, `Disconnected`). Les
combats (`melee`, `ranged`, `magic`) lisent `--site` : grappes au niveau
du site, la plus proche du site d'abord (`kit::foes::ranked_clusters`),
marche au niveau du site ; aucun spawn = `Failed` typé, et un run sans
kill, sans banque ni mort échoue (`nothing_hunted_error`) au lieu d'un
`goal_reached` à 0 tick. Le moulin (`mill_grain`) trouve trémie, levier et
meule dans la carte servie (`static_locs_near`) : le worker ne peint aucune
carte, `world.loc_at` seul ne voyait jamais la trémie.

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

Site, stands et trajets (2026-11-13, erreurs du lot de 21h19) :

- Pêche : un `--site` assigné lève la limite de trek banque des pickers
  (`spots::bank_trek_ok`) ; seuls ses spots sont candidats. Un site de
  Karamja (Musa Point, `0_45_49_saltfish` filet/appât et
  `0_45_49_rarefish` casier/harpon) banque à Draynor via le bateau
  (ancre du quai), 230 cases plus loin : l'ordre fait foi, plus de
  « no fishing water in range » une fois débarqué. Outil et appât sortent
  de la banque AVANT le trajet (`fish_gear::gear_before_departure`, une
  visite au guichet ; pièces de voyage déjà portées par `arm_fares`) :
  live admin 2026-11-17, un run homard partait à Karamja sans casier,
  le découvrait sur le quai et refaisait le bateau (2 traversées, 60
  pièces). Minage, coupe, essence, cisailles, seaux, sorts et combat
  équipent déjà avant de partir.
- Minage : client routefinder, le serveur ne marche jamais un joueur vers
  un loc et un rocher (centrepiece) ne s'atteint que par un côté. Le stand
  est un côté du rocher (`mine::rock_stands`, plus proche d'abord), jamais
  le coin diagonal où la marche s'arrête (« I can't reach that! » sur
  chaque clic, rochers bannis puis « mine failed »).
  Roche vidée (2026-11-16, `spots::choose_rock`) : le pool lit le flux
  (`rock_depleted_seen` : étage vide ou case vidée) ; la roche vidée n'est
  plus bannie 250 ticks (repousse fer 20, cuivre/étain 10, charbon 100 :
  Rimmington perdait ~200 ticks de « no rock spot » par tour de six
  roches) : le bot passe à la roche debout la plus proche du site, sauf
  si la repousse (`action_odds::rock_respawn`, borne haute) bat la marche
  vers elle ; tout vidé ⇒ il regarde le flux jusqu'à la première repousse
  (`mine::wait_rock_respawn`, ≤ repousse + 10, plafond 300), jamais un
  clic sur une roche vide. Un « no ore » que le flux ne confirme pas
  garde le ban (jamais de boucle de clics). Mesure `admin`
  `farm:iron_ore 20` (Rimmington) : 43,7 → 11,5 ticks par minerai au site,
  rapport 1029 → 491 ticks, sans alerte.
- Trajets : `walk_checked` ne coupe plus le voyage sur une égratignure ;
  un coup reçu ne l'arrête (Ok(false), l'appelant fuit) que si les PV
  tombent sous la ligne de repas (`skill_walk::hurt_aborts`,
  `EAT_FRACTION`). Les « random event between legs » du lot étaient tous
  des coups des gardes de la prison de Draynor sur la route de Lumbridge
  (aucun vrai événement aléatoire).

Lecture des ordres : `contract::check_order` lit une ligne de lancement
avec le parseur PROPRE du script (`cycle::parse_order` pour les
activités, `market::parse_order` → `parse_side`/`parse_shopbuy`/`parse_make`/
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
proche du `--site`. Les ordres `market buy|sell|make|cleanup` ne portent
plus `--site` ni `toll` (seul `shopbuy` marche vers un marchand).

## Banque pleine (2026-11-14)

Serveur (`bank.rs2`, `bank_check_allowed`) : une banque F2P qui occupe
60 cases refuse tout type NOUVEAU (« You need to subscribe for a members
account… ») ; un type déjà en banque s'empile toujours. Live bst29bh9 :
chaque `smelt` et `mill_grain` échouait `deposit did not clear` sur le
seul marteau acheté pour la forge. `kit/bank_full.rs` :
`bank_trip` dépose ce qui rentre (`deposit_what_fits`) et garde dans le
sac les types refusés (outils compris, loggé `bank full: kept …`) ;
il n'échoue que quand un reste n'a aucune raison de rester (aucun refus
du serveur, type déjà en banque) ou quand le sac n'a plus une case pour
la charge (`deposit_verdict`). `bank_trip_resilient` ne cherche pas une
autre banque quand c'est le produit de la charge qui est refusé (même
compte, même banque pleine) : `bank full: … load stuck`.

Concierge (`market cleanup`) : les comptes lisent le SAC
(`Bot::pack_count`, `janitor::packed`) et la DERNIÈRE banque connue
(`janitor::known_bank`, ouverte ou gardée à sa fermeture), jamais
`inventory()` (banque ouverte = banque + miroir comptés comme portés) ni
`bank_items()` (vide dès qu'un retrait ferme la boîte : D17 lisait
« vidé, sous la limite » après la première pile). Le surplus se met en
vente là où il est (sac + banque, le carnet réserve le stock en banque) ;
une pile que le vendeur garde (aucun coût propre) ou que le carnet
refuse est un candidat D17, lâchée pile entière jusqu'à repasser sous
55 types.

## Voyage banque de combat (2026-11-14)

`kit/combat_bank.rs` (`prepare_at_bank[_for]`, réexporté par `gear`).
Live frank_lane, pia_brown, leo_king, bbkjz2vk, bd23dqpq : la garde du
dépôt valait « tout objet portable du sac », lue sur `inventory()`
(banque ouverte = banque + miroir) ; le butin (20 casques médiums,
dagues de fer, 140 flèches de bronze) restait au sac, chaque kill
finissait `left: pack full` et repartait à la banque, des centaines de
trajets sans erreur. Le même `inventory()` lisait « déjà porté » un arc
ou une épée rangés : jamais retirés (ryan_park à la dague en distance,
pia_brown à la hache).

Garde (`combat_keep_ids`) = munitions du style (toutes les flèches que
l'arc prévu accepte ; runes du sort ordonné) + objets de la chasse
(`HuntPlan.keep` : clé de laiton, pièces du péage) + nourriture portée
seulement quand la banque n'en a pas. Tout le reste est déposé
(`deposit_what_fits`) : butin, doublons, outils ; la passe de vente
normale s'en charge. Pièces prévues absentes du corps
(`kit_to_withdraw`, premier candidat par emplacement, pas de bouclier
sous un arc) : UNE unité chacune ; nourriture au plan sans descendre
sous 2 cases libres (`food_load`) ; équipement ; une pièce remplacée
retombée au sac repart en banque dans la même visite (`pack_surplus`).
Un voyage qui laisse moins de 2 cases libres (`fight` repart à ≤ 1)
échoue `Failed` en nommant le contenu du sac (`trip_verdict`), jamais
une boucle. Équiper ne logge que les emplacements qui changent
(`gear::equip_logged`, fini le « equipped bronze_med_helm » par voyage).

Plan d'arme : en mêlée une hache de bûcheron ou une pioche
(`gamedata::gear::is_tool`) passe derrière toute vraie arme possédée
(seule, elle reste mieux que les poings) ; masses (`weapon_spiked`)
pesées comme les armes contondantes, boutons `combat_spiked`
attaque/force/défense. Distance : jamais d'arme de mêlée en main
(`off_style_weapon` la retire sans arc). `equip <style>` lit le kit
possédé (sac + porté + banque connue, `gear::owned_gear`) et va
chercher en banque les pièces prévues qui n'y sont que là (plus la pile
de flèches quand le carquois est vide) ; le cerveau
(`gear_plan::unworn_upgrade`) ne compte plus un outil porté comme
l'arme du bot.

## Échoué sur Karamja, munitions, bans, tonte (2026-11-15)

- `kit/stranded.rs` : avant chaque script (sauf rescues, tutoriel et
  outils de dev, `lib::guards_the_fare`), un bot sur Karamja avec moins de
  30 pièces en sac gagne le retour chez Luthas (`luthas.rs2` : 30 pièces
  par caisse de 10 bananes, `banana_crate.rs2`, `banana_tree.rs2`) ; sac
  plein : une unité de l'objet le moins cher est jetée. La caisse pleine se
  lit au message serveur (« The crate is already full. ») ou à dix
  bananes posées : la varp `crate_bananas` n'arrive jamais au client.
  Ensuite, un run sans ordre `boat` qui démarre sur l'île rentre d'abord à
  Port Sarim (traversée payée ouverte pour cette marche seulement) : ses
  chemins non payants ne mènent à aucune banque depuis l'île.
- `quests::common::find_loc` : l'état live d'une case remplace son loc
  statique (arbre à bananes cueilli → `bananatreefour` ; cliquer l'id
  statique était ignoré sans message par le serveur).
- Munitions (`gamedata::equipment::ammo_fits`, règle de
  `player_ranged.rs2`) : un arc tire des `arrows` jusqu'à son tier, une
  arbalète des `bolts`. Plan d'équipement, recharge du carquois et
  remèdes l'appliquent (frank_shaw : 64 carreaux derrière un arc long en
  chêne, un run de 18 000 ticks sans un tir).
- Bûcheron : un arbre dont aucune case voisine n'a de route (preuve
  `Disconnected` du planificateur, `skill_walk::step_adjacent_proved`)
  est banni longtemps ; `BanSet::ban_at_least` empêche le ban court qui
  suit d'écraser ce ban long.
- Tonte : un pré épuisé envoie le bot jusqu'à 2 autres prés (au-delà de
  30 cases) avant l'échec « no sheep ».


## Bijoux, enfilage, cuir, ateliers PNJ (2026-11-15)

- `smelt_jewelry` voie or : le four passe par la brique `produce`
  (`furnace1`/`furnace2` exacts, `--site` du brain, côtés que
  `forceapproach` accepte) avec l'adaptateur `ProduceUi::InvGrid` (objet du
  slot pressé Make 10/5/1, slot vide = erreur nette). Avant : `find_loc`
  par sous-chaîne cliquait le `furnace2` de Lumbridge du mauvais côté
  (« I can't reach that! »), 0 sur 38. Testé live aux fours de Lumbridge,
  Al Kharid et Falador.
- `stringing` : un chargement apparie amulettes et pelotes (avant : une
  seule pelote, une amulette par aller-retour).
- `leatherwork` : la matière est celle de la recette (cuir dur pour le
  corps en cuir dur) ; un fil par 5 pièces (`leather.rs2`), le fil s'empile.
  Couture (2026-11-16) : un appui Make 10/5 (`press::make_press`, triplet
  sous le bouton x1 : gants 116/115/114) au lieu d'un x1 par pièce ; la
  file serveur coud une pièce tous les 2-3 ticks jusqu'au cuir ou au fil
  épuisé (`press::follow_chain`), une coupure (niveau) se ré-appuie.
  Mesure `admin` `craft:leather_vambraces 5` : 4,6 → 3,2 ticks par pièce.
  Bijoux or (`InvGrid`) : même `make_press` (avant : 10/5/1 sur le seul
  porté, sans le reste du `--want`).
- `produce` atelier PNJ hors de vue : marche jusqu'à son apparition la plus
  proche (dump) puis le cherche à nouveau (teinture chez Aggie).

## Transferts banque et attente d'action (2026-11-16)

Banque : une seule primitive SDK, `client/bank.rs`. `bank_button(veut,
pile)` choisit LE bouton d'un joueur (`bank.rs2`/`bank_deposit.rs2`, ops
1/5/10/All/X des deux conteneurs) : All quand toute la pile part, 1/5/10
quand ça tombe juste, sinon X (`P_COUNTDIALOG` → `RESUME_P_COUNTDIALOG`).
Le serveur déplace `min(demandé, inv_total)` sur toutes les cases d'un
coup : un Deposit-All par TYPE vide un objet non empilable entier
(`deposit_presses`). `bank_deposit`, `bank_deposit_all`, `bank_withdraw`,
`bank_withdraw_noted` et `kit::withdraw::bank_withdraw_items` passent
tous par elle ; un refus serveur (sac/banque pleins, 60 types) arrête
l'attente. Avant : un Deposit-All par case (12,3 pressions par dépôt sur
la flotte), retraits en 10/5/1 (5,5 pressions par retrait ; pièces
verrouillées par le marché = une pression par 10 pièces).

Attente d'action (règle de la coupe de bois, `kit::harvest`) :
`reclick_after_silence` ne re-clique jamais tant qu'un coup est acquitté
sur une cible vivante, une seule sonde au premier tour muet sinon ;
`after_gain` termine le poste à la déplétion (souche, rocher vidé :
`HarvestSource::depleted`), re-clique tout de suite seulement quand le
serveur a fini l'action avec le produit (`gain_ends_action` : minerai,
gemme). Minage, essence, pêche (banc déplacé = `spot_hopped` sur le slot
cliqué), cueillette, traite, tonte : même noyau. Combat mêlée/distance :
relance seulement après une fenêtre sans XP ni pas
(`strike::reattack_due`) ; la magie reste un sort par clic. Fonte : menu
`Smelt` du four, bouton X, chaîne serveur (`smelt::SMELT_X`) ; une
chaîne coupée (niveau gagné) rouvre le menu (`produce::chain_cut`).
Cuisson, feu, enterrement : un clic par unité côté serveur (274), déjà
attendu.

## Meilleur outil banqué (2026-11-16)

`kit/tool_upgrade.rs` (live : jim_ellis, bûcheronnage 83, coupait des ifs
à la hache de bronze, une hache de fer en banque ; `reclaim_free`
s'arrêtait dès qu'une hache était tenue). Haches et pioches : à chaque
visite de banque (`bank_trip`, pour toute famille dont la garde contient
un outil) et au départ du run (`reclaim_free` avec un outil tenu, marche
jusqu'à la banque du run si la vue banque connue en montre un meilleur),
le meilleur outil banqué de rang strictement supérieur au meilleur tenu
UTILISABLE (`gamedata::gear::usable_tools` au niveau du skill, comme la
trousse du planner) sort ; les autres exemplaires de la famille du sac
rentrent en banque (jamais deux dans le sac). Un outil porté reste porté
(le plan d'équipement possède la main droite), le meilleur va au sac où
le serveur s'en sert. Verrous du marché respectés (`banked_usable`).
Pêche exclue : un outil par palier (filet, canne, harpon, casier), pas un
rang (`fish_gear::ensure_gear` retire l'outil exact du palier).

## Repas et nourriture de combat (2026-11-17)

Demande utilisateur : la bière n'est jamais une nourriture ; manger tôt,
sans gâcher, se soigner à la banque, emporter la nourriture adaptée.
- Boissons alcoolisées (`gamedata::food::ALCOHOLIC_DRINKS`, catégorie
  serveur `alcoholic_drinks` : bière +1 PV, Attaque −1 et −6 % ; vin +11,
  Attaque −2 ; bières, stout, mind bomb, grog, alcools) : `heal` = 0,
  jamais mangées, jamais comptées comme nourriture (avant : 1 478 bières
  « mangées » en combat d'échelle, la règle sans-nourriture ne tombait pas).
- Ligne de repas (`kit::meals`, combat) : PV ≤ max − meilleur soin tenu
  (cette bouchée ne gâche rien), jamais sous la ligne d'urgence = 2 max hits
  du PNJ (`melee_math::npc_max_hit` : un coup peut tomber pendant les
  3 ticks du repas, le suivant ne doit pas tuer), au moins 35 % des PV max.
  Max = PV de base (avant : le plus haut PV courant vu, d'où « 13/26 » sur
  un bot à 64 PV). Choix : sous l'urgence le plus gros soin ; sinon le plus
  gros soin ≤ PV manquants, rien quand aucun ne tient (50/55 : ni homard ni
  truite, une crevette). Sans nourriture sous l'urgence sur un spot qui
  mord : retour banque.
- Banque de combat (`prepare_at_bank_for`) : les bouchées qui remplissent
  les PV manquants sans gâchis (`meals::heal_plan`) sortent en plus de la
  charge et sont mangées avant de repartir (`eat_to_full`).
- Charge : la nourriture qui tient dans la moitié des PV max d'abord
  (`melee_ladder::food_room`, meilleur soin d'abord), les gros soins
  ensuite (plus petit d'abord) : un bot à 20 PV n'emporte pas d'espadon.
- Spots ordonnés (`melee <spot>`, fermes de drop, ranged, magie) : même
  règle que l'échelle (`kit::ladder::ordered_plan`) : nourriture requise
  partout sauf vaches, poulets et spots inoffensifs (max hit du PNJ sous
  `HARMLESS_HIT_SHARE` = 5 % des PV de base du bot : hommes, max hit 1,
  pour un bot à 30 PV ; décision utilisateur 2026-11-17, fillers compris ;
  règle unique `gamedata::melee_ladder::spot_food_required`, lue aussi par
  le planner pour préférer une méthode faisable maintenant),
  charge dimensionnée par le barreau ;
  sans nourriture en banque ⇒ `Blocked: <food_to_ask>` (avant : plan
  « plain », combat sans nourriture jusqu'à la mort : 57 runs `men` morts).

## Contention, blessure, préconditions (2026-11-17)

- Charge sans ressource avec d'autres joueurs sur le site (mining, ramassage,
  tonte) : `Blocked contended:<site>` (`site::contended_error`), jamais un
  échec (D4).
- Charge ratée sous la moitié des PV : banque, soin, retour
  (`survival::hurt_return`, D3), deux fois au plus d'affilée.
- Nourriture gardée au dépôt de combat bornée par la charge et
  `HUNT_MIN_FREE` (`combat_bank::kept_food_surplus`).
- Intrants vérifiés au départ quand la banque est connue
  (`withdraw::preflight_blocked`, smelt).
- Un script de combat sans objectif (ou dont l'objectif n'est pas atteint,
  tour 3 point 6 : `cycle::goal_met`) finit `budget_exhausted`
  (`cycle::ended_code`), jamais `goal_reached`.

## Banque par coût de route (tour 2, N3, 2026-11-17)

`kit::bank_route` : la banque servante d'un site (`site::site_bank`) et
celle du bot (`bank::nearest_bank`) sont choisies par COÛT DE ROUTE : les
4 banques les plus proches à vol d'oiseau (un guichet par banque) sont
chiffrées en un seul `walk_costs` (`Bot::walk_costs_from`, depuis le site
ou le bot), la moins chère gagne, une banque sans route jamais ; sans
service (aucun coût) l'ordre Manhattan reste. Mine du marais de Lumbridge
(3244,3165) : Draynor (route 170), plus Al Kharid (467 à pied). Les deux
choix écartent un guichet couvert par un chasseur qui tuerait le bot tant
qu'un sûr existe.

Tour 3 (point 2) : la banque d'un SITE se lit dans la table hors ligne
`gamedata::site_banks` (voir `gamedata.md`) : `site::site_bank` (site du
cerveau), `spots::nearest_bank_at` (pickers de spots, roue, péage),
`foes` (camp de combat) et `danger::site_bank` (exigence de combat) lisent
la même banque, au coût de route réel ; la garde de trajet d'un spot de
pêche compare ce coût à `danger::BANK_TREK_LIMIT` (250, coût de route).
Seule la banque du BOT (`bank::nearest_bank`, position live quelconque)
reste chiffrée en direct par `bank_route`.

## Besoin verrouillé par ses propres annonces (tour 3, point 3, P5)

`kit::locked::tag_locked`, appliqué par `lifescape_scripts::run` à tout
`Blocked` : un besoin dont le bot ne porte aucun exemplaire libre (sac, ou
banque connue hors verrou de marché) mais dont la banque garde des unités
verrouillées par ses ordres devient `locked:<objet>` (famille d'outils :
le meilleur membre verrouillé ; intrant : l'objet). Le cerveau
(`brain_fold`, `brain_tools::locked_needs`) retire les annonces de la
famille (raison `need_held`) et relance le nœud sans bloc compté
(`locked_released`) ; aucune annonce retirée (verrou d'un échange en
cours) : le besoin nu suit la voie usuelle.
