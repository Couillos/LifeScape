# Navigation — workers sans carte, service unique

Les workers de session ne chargent aucune carte : ni paquet, ni monde
statique, ni tables de planification, ni collision peinte. `run_session`
(`rs/crates/orchestrator/src/session.rs`) ne branche que le handle du
service (`bot.plan_client` sur `run/pathfinding.sock`) et épingle la
version snapshot (`bot.plan_data_version`) ; `maps`, `nav_world` et
`nav_tables` restent à `None` pendant tout le run.

Le daemon porte l'unique snapshot (monde statique + `Tables` + version,
`rs/crates/orchestrator/src/plan_service.rs`, socket
`run/pathfinding.sock`) et sert les plans sur 2 threads (`PLAN_THREADS`),
files bornées (`QUEUE_CAP` 16, `ACCOUNT_CAP` 2, `RESULTS_CAP` 256),
cache de routes central (§ Cache de routes, 8192 plans + 2048
négatifs), garde RAM (warn/shed),
timeouts `SUBMIT_TIMEOUT` 10 s / `POLL_TIMEOUT` 5 s / garde client
`SERVICE_WAIT_MAX` 30 s (`rs/crates/nav/src/service.rs`).

Ops du service (`dispatch`) : `submit` / `poll` / `cancel` pour les plans,
lectures batch hors queue `door_defs`, `tiles_flags`, `static_locs_near`,
`in_f2p`, `toll_fence`, `wants`, `diff`, `debug`, `transitions_list`,
`probe_targets`, `doors_near`, plus `stats`. Chaque op batch est comptée
et exposée dans `nav-stats`.

CPU (2026-11-14) : le thread `plan-accept` ne fait plus qu'accepter ;
chaque connexion (lecture de la ligne + `dispatch`) part sur le pool
borné `plan-query` (2 threads, file 128, `service_accept.rs`, compteur
`pooled`) — un client muet ou un `walk_costs` ne bloque plus les autres.
`toll_fence` / `static_locs_near` lisent par région
(`StaticWorld::toll_gates_within` / `static_locs_within` : un verrou
d'écriture pour peindre, un de lecture pour lire, au lieu de deux par
tuile ; `toll_fence` r150 5,8 ms → 7 µs). Cache `walk_costs` service
indexé par l'ENSEMBLE des cibles + options complètes (danger inclus,
il manquait) ; compteur `walk_costs_hits`. Côté worker,
`sdk/src/client/static_cache.rs` répond sans aller-retour aux requêtes
statiques déjà servies sous la même version : `door_defs`, `tiles_flags`,
`in_f2p` (par clé, lot servi seulement s'il est entier en cache),
`toll_fence` / `static_locs_near` (listes couvrantes
`nav/src/region_lists.rs` : toute requête dont le losange tient dans une
liste servie est filtrée localement, même ordre ; `toll_fence` demande
`list_radius` = r + 48, champ additif `gates`, un vieux daemon ne le
renvoie pas et rien n'est caché), `walk_costs` (lot entier, même
source + ensemble + options). Invalidation : toute entrée appartient à
une version ; la première réponse portant une autre version vide tout.
Après un `reload-nav`, un worker ressert donc ses réponses statiques
jusqu'au premier aller-retour (plan, miss) qui lui apprend la version.

Planification 100 % distante (`Bot::plan_travel`,
`rs/crates/sdk/src/client/travel.rs`) : sans monde local, le plan vient
du service ou l'appel échoue en `Error::Travel("nav-down: …")` — down,
refus, timeout, version biaisée après 1 resubmit. Jamais de plan local.
Sans tables locales, les options sont celles par défaut (`f2p_only`, sans
`satisfaits`) ; sans `plan_client`, `walk_costs` rend `None` (ordre
Manhattan conservé). Le replan après échec replanifiable repasse par le
service avec l'arête bannie ; un échec après une pause market finie
pendant le plan (`Bot::pauses_done`) replanifie sans ban ni dossier
(`travel::paused_mid_trip`). `nav-down` ne déclenche ni reconnect ni
rescue : la session reste vivante, les scripts réessaient par leurs
boucles.

Exécution sans pack : le plan porte l'ouverture des portes (`open_op`,
péage, `stand`/`cross_to`) et le verdict des rides ; le worker ne garde
qu'un registre éphémère (portes du plan + vérification live). Le
diagnostic (`suspect_tile`, refus) croise le live (`world.loc_at`) et le
batch `tiles_flags`. `debug_walk` demande au service la partie statique
(flags, plan ou diagnostic `NoRoute`) et complète en live.

Dossier d'échec sous `run/failures/` : `live_locs`, messages récents,
`trail`, plan (100 % live) + `static_live_diff` calculé service-side sur
la fenêtre live envoyée par le worker (rayon 10). `orchestrator
replay-nav` rejoue le dossier et classe A (plan faux) / B (monde ≠ carte)
/ C (exécution) (`rs/crates/orchestrator/src/replaynav.rs`).

Portails doubles (catégories `gate_main_closed`/`gate_outer_closed`,
ids 28/29) : à l'ouverture le contenu (`gates.rs2`, `open_gate`) repose
les deux battants en murs droits `angle+3`, à 1 et 2 tuiles au-delà de
la clôture côté battant principal. Le peintre statique
(`rs/crates/nav/src/collision/gate_swing.rs`) peint ces murs en
permanence : un portail fermé coûte 1-2 tuiles de détour, un portail
ouvert ne fait plus refuser le pas suivant (avant : ~5000 refus/jour
aux portails Falador/Lumbridge, replan identique, aller-retour par le
portail). Goldens : `rs/crates/nav/tests/gate_swing.rs`.

Portes simples (catégorie `door_closed`, id 19, mur droit) : ouverte
(500 ticks, `doors.rs2`), la porte réapparaît une tuile vers son angle
(`door_open`), tournée `angle+1` : un battant sur un bord de la tuile
derrière l'embrasure (Port Sarim 1530 (3014,3237) → 1531 (3015,3237)
angle sud). Le peintre ne le peint **pas** en mur (peint, il coupait 22
pièces F2P en deux, ex. Lumbridge 1536) : il pose des marques
planificateur `DOOR_LEAF_WEST/NORTH/EAST/SOUTH` (bits 0x1000000 à
0x8000000, jamais serveur, jamais bloquants ; `tiles_flags` les renvoie,
les lecteurs masquent) et un pas qui traverse l'arête marquée (diagonale
qui l'effleure comprise) coûte `DOOR_LEAF_STEP_COST` (10) de plus, dans
`plan` comme dans `walk_costs` (`collision::leaf_step_extra`). Le plan
contourne quand un détour court existe, traverse sinon. Goldens
`porte_port_sarim_*`, `porte_lumbridge_sud` (assertion `never_steps=`).

Doubles portes (catégories `door_left_closed`/`door_right_closed`,
`doubledoors.rs2`) : op1 ouvre les deux battants, sans contrôle de côté.
Château de Lumbridge (décision utilisateur 2026-11-13 : vraie porte, le
portail des remparts est toujours ouvert, cosmétique, jamais une loc) :
1516 (3217,3218) / 1519 (3217,3219), mur droit angle OUEST. Depuis
l'extérieur, le stand est la tuile du battant voisin (reachWall1 angle
OUEST : stand nord/sud valide sans BLOCK_NORTH/SOUTH), ex. clic 1516
depuis (3217,3219) puis traversée à l'ouest. Les anciens overrides
`one_way_doors` « passable_from: west » (sans appui serveur) scellaient
le château : NoRoute exhaustif 3-9 s (men/dark_wizards, ~50/h). Un
override `one_way_doors` exige une preuve contenu (refus scripté de côté,
ex. `haunteddoorl`). Goldens `chateau_entree`, `chateau_sud_interieur`,
`chateau_draynor_men`. Battants ouverts (`open_double_doors_left`/`_right`) :
chacun recule d'une tuile vers son angle, tourné `angle+3` (gauche) ou
`angle+1` (droit) — château : 1517 (3216,3218) sud, 1520 (3216,3219) nord.
Mêmes marques `DOOR_LEAF_*` que les portes simples (avant : ~700 refus/h
sur le pas (3216,3220)->(3216,3219), replan identique). Peints en murs ils
ne coupaient aucune pièce (40 battants F2P analysés), la marque reste le
choix commun. Golden `nav/tests/door_swing.rs`.

Battant ouvert en travers du pas (2026-11-13, SDK `client/door_leaf.rs`) :
quand aucun détour n'existe, le plan traverse l'arête marquée ; si la
porte est ouverte à ce moment, le battant est un vrai mur et le serveur
refuse le pas (t01-21040 : cuisine de Falador (2988,3367), entrable
seulement par l'arête du battant de 1512 (2989,3368) ; ouvert, 1515 en
(2989,3367) angle ouest, « I can't reach that! »). `exec_walk` cherche
avant chaque jambe un mur droit live en travers d'un pas (sur la tuile
source, ou sur la cible d'un pas orthogonal) ; confirmé par la marque
`DOOR_LEAF_*` statique (un batch `tiles_flags`, seulement quand un mur
live croise un pas), il marche jusqu'à la tuile source, ferme le battant
(op1, `[oploc1,_door_opened]` / `_door_*_opened` des portes doubles),
attend sa disparition (23 ticks) puis reprend ; battant resté :
`StepRefused` typé. Un mur live sans marque n'est jamais cliqué.

Mobilier qui « s'ouvre » (catégories `chest_closed` 33, `cupboard_closed`
35, `drawer` 37 : la variante ouverte bloque toujours) n'est jamais une
porte (`loctypes.rs`) : avant, le plan « ouvrait » les tiroirs 350 de
Varrock pour marcher dessus (DoorDidNotOpen). Golden `nav/tests/furniture.rs`.

Pause marché pendant un trajet (2026-11-13, `client/travel_pause.rs`) :
la porte de pause de session (`session_rescue::gate_market`) joue la
livraison inline à la fin d'un pump, souvent celui d'une jambe en vol,
puis ramène le bot à la tuile de départ de la pause et la clôt
(`Bot::note_pause`, compteur `pause_ticks`). Le trajet interrompu
reprenait sur un état périmé (échéance consommée par 400-4900 ticks de
pause, file de marche serveur remplacée, registre de portes vidé par le
trajet imbriqué) : en 40 min, 28 `walk timed out`/`door loc timed out`,
~30 `step refused` et 5 `door did not open` (dont les portes du château
1516/1519 et les `Timeout` de b5xntyz6 vers (3208,3237)) étaient des
pauses. Désormais : `await_leg` s'arrête dès qu'une pause s'est close
(marqueur `paused-mid-trip`), et un échec de segment avec une pause close
pendant le plan replanifie depuis la position, sans arête bannie, sans
dossier, échéance repoussée de la durée de la pause
(`run_plan_through_pauses`, au plus `PAUSE_REPLANS` = 2 par trajet,
hors du replan normal). Un segment porte dont le bot est déjà sur
`cross_to` tient (plan demandé pendant le dernier pas d'une traversée :
3 « off stand »). `suspect_tile` ne lit plus un « pas » nul ou long
(jambe renvoyée après pause) comme un mur fermé.

Péage sans pièces : `allow_paid` d'un script n'ouvre les chemins payants
que si le sac porte le péage (`travel_custom::paid_allowed`, 10 pièces,
`border_gate.rs2` refuse sans message sous 10) ; sinon le plan contourne
la clôture (golden `peage_detour`) ou répond `PaidOnly coins:10`.
Avant : bct5rxpg (ground_pickup, 0 pièce) cliquait la porte 2882 puis
2883, « Yes, ok. », rien, deux `DoorDidNotOpen`.
Après le choix « Yes, ok. », le drain attend la réponse serveur
(`toll_pick_answered` : dialogue fermé ou autre chose que le choix
dépensé) avant de lire la page : le choix encore affiché au tick du clic
passait pour « un nouveau choix », le `continue` du `chatplayer` ne
partait jamais et la porte restait close (2026-11-14, admin, 10 pièces
au sac : quatre `DoorDidNotOpen` ; après : deux `continue`, porte ok).

Poches scellées (`nav/src/pocket.rs`) : une destination marchable mais
emmurée (coin entre table, tabouret et murs, bande derrière une
clôture) faisait balayer tout le monde F2P à la recherche avant
`Disconnected` (4-6 s par plan live, 21 dossiers melee/40 min, visées par
`approach_npc`). Un flood arrière depuis la destination, sur la vue la
plus permissive (toutes portes à op d'ouverture ouvertes, péages,
exigences et F2P ignorés, règle de pas serveur), plafonné à 512 tuiles :
s'il se ferme sans toucher le départ et qu'aucune montée/arête custom n'y
atterrit, le plan répond `Disconnected` tout de suite (closest = départ)
et `walk_costs` laisse la cible à `None` sans épuiser la recherche
(lève en partie la « limite connue » ci-dessous pour ces cibles).
Goldens `poche_*` (4-5 ms), test `pocket::tests`.

`approach_npc` (SDK, 2026-11-13) ne vise plus que des stands
atteignables (`Bot::npc_stands`) : tuiles de côté de l'emprise, un batch
`tiles_flags` écarte les tuiles bloquées et celles murées côté PNJ (règle
`reachRectangle1` : pas de mur sur l'arête face au PNJ), un batch
`walk_costs` écarte les poches et le reste inatteignable (les tuiles
bloquées n'y entrent jamais : elles épuiseraient la recherche), ordre
par coût de marche. Aucun stand : `Unreachable` « NPC <nom> (slot N) at
(x,z,niv): no side tile reachable », sans un pas. Avant : marche vers la
poche, dossier NoRoute (Varrock (3247,3398)…, Lumbridge (3257,3242)).

Escaliers en colimaçon du château de Lumbridge (2026-11-13) : le prober
et l'exécuteur de montées cliquaient la tuile d'emprise la plus proche
du stand ; le serveur ne trouve un loc que sur son origine
(`OpLocHandler` → `Zone.getLoc(x, z, id)`), le clic tombait sans message.
Les 6 verdicts du château (1738/1739/1740) étaient des `no_move` faux :
l'étage 1 (le duc, Rune Mysteries) était injoignable. Les deux cliquent
désormais `from` (l'origine, vrai pour les 367 lignes du dump). Verdicts
du château reroulés en live (admin, clic origine) : 8 trajets ok. Le haut
de la tour sud (niveau 2) est du ciel pour la carte (stand ajouté à
`sky_floors` depuis, voir plus bas). Goldens `chateau_escalier_*`, `chateau_duc_retour`,
`chateau_toit_*`.

Côté d'approche des montées (2026-11-13) : les 91 `no_move` du
2026-11-06 portaient tous « I can't reach that! » (le serveur trouvait
le loc ; seuls les 8 `spiralstairs`/`spiralstairsmiddle` 2×2 hors
château venaient du clic d'emprise). Deux causes dans `plan::stands` :
`forceapproach` ignoré (`stairs` sud, `stairstop` nord, `spiralstairstop`
est, échelles `*_directional`, échelles de navire, `woodenstairs*`,
`spookystairs*`… : `LocConfig.ts` bloque les quatre côtés sauf le nommé,
tourné par l'angle, `reach::rotate_flags`) et largeur/longueur passées
à l'envers à `reach` (un escalier 2×3 à angle impair lu 2 de large : une
tuile voisine « chevauchait » le loc). `dump-transitions` écrit
désormais `force_approach` (drapeaux moteur, 0 = tout côté) par ligne
de `transitions.json`, lu par `Tables` ; `stands` le passe à `reach` et
suit l'ordre moteur (`reachedLoc(.., type.width, type.length, ..)`).
Vérifié en live (admin, `call` cheat tele + `op_loc` depuis le premier
stand calculé, 2026-11-13) : 87 `no_move` → ok ; 14 tuiles d'arrivée ou
de stand non peintes ajoutées à `sky_floors` (hauts de tours niveau 3,
haut de la tour sud du château, ponts du navire de Karamja, étage
(3175..3176,3401..3419)). Restent `no_move` : `miningguildladder` (Mining
60). Non verdictés (barrières de contenu) : `monasteryladder` (ordre de
la Guilde de prière), `newbieladder*` (tutoriel), `shantay_pass` (monde
membre), `laddertop_directional_188` (admin tué par les chevaliers noirs
à l'arrivée). Test `nav/tests/force_approach.rs`, goldens 55-61
(`chateau_tour_sud_toit_descente`, `tour_sorciers_*`, `manoir_draynor_etage`,
`palais_varrock_etage`, `falador_chateau_etage`, `varrock_ouest_escalier`).

Portes diagonales (forme 9, `WALL_DIAGONAL`, 2026-11-14) : la règle de
portée moteur (`ReachStrategy.reachWall1`, bras `WALL_DIAGONAL` : les
quatre voisins orthogonaux, sauf mur du stand tourné vers la porte)
était codée sous la forme 1 dans `nav/src/reach.rs` ; aucune porte
diagonale n'était cliquable. Tour des Sorciers : la pièce de l'échelle de
la cave (Sedridor, Rune Mysteries) est fermée par 1536 (3107,3162) angle
sud ⇒ Disconnected (dossiers bst29bh9-218/-290/-423). Ouverte
(`doors.rs2`, bras diagonal de `door_open` : OUEST +z, NORD +x, EST −z,
SUD −x), la porte devient une loc pleine pour 500 ticks, (3106,3162)
ici : `gate_swing::open_diagonal_door_leaf` marque les quatre bords de
cette tuile (`DOOR_LEAF_*`, jamais un mur) et le plan passe par
(3107,3161). Goldens `tour_sorciers_echelle`, `tour_sorciers_sortie`
(`avoids=3106,3162`), `sedridor_depuis_duc` (château niveau 1 → cave).

Porte verrouillée pour toujours : `lockeddoor1` (1591, op1 Open,
`locked_doors.rs2` ne l'ouvre jamais) est dans `blocked_locs` ; golden
`porte_verrouillee_varrock`.

Arrêts (`Interrupted`) : fin de budget (`budget_exhausted`), unload ou
stop du cerveau tombant en pleine marche écrivent encore leur dossier
(rejouable) mais `errors_model::from_dossier` ne les ingère plus comme
erreurs (26 lignes « run stopped » melee en 40 min, toutes légitimes).

Veille de blocage d'une jambe (`await_leg`, SDK) : une tuile plus loin
sur la jambe compte comme progrès, pas seulement un rapprochement de sa
dernière tuile (une jambe qui contourne une clôture s'éloigne de sa fin
plus de 13 ticks en avançant : ~80 faux « stalled »/h, champs au sud de
Varrock).

Destination bloquée (guichet, comptoir, arbre, eau) : le plan finit sur
la tuile de repli de meilleur rang (`Tile::fallback_rank`) : niveau,
puis Chebyshev, puis `fallback::walled_off` (voisin séparé de la cible
par un mur : bord partagé pour un côté, les quatre bords du coin pour
une diagonale), puis coût. Seul un voisin muré est déclassé : une
diagonale ouverte reste valide et la tuile où le bot se trouve (coût 0)
garde l'avantage (rive (3087,3228) pour l'eau (3086,3227) : plan vide).
Avant : la diagonale extérieure (2944,3366) du guichet ouest de Falador,
derrière le mur, gagnait au coût ; `open_bank` y tentait d'abord le stand
guichetier (poche fermée, NoRoute exhaustif). Goldens `guichet_falador_*`
(assertion `adjusted=`), `rive_deja_la`.

Guichets de banque (2026-11-13) : chaque guichet F2P a des stands des
deux côtés du comptoir ; le côté banquier est une poche fermée (planifié
vers elle : NoRoute épuisant 3-11 s sous charge, ~1000 dossiers/jour à
Varrock ouest (3187,3436), Varrock est (3252,3418), Falador ouest
(2945,3366)). `open_bank` ne marche que vers les stands `client` de
`gamedata/data/bank_stands.json` (via `Tables::bank_client_stands`,
intersectés avec les stands live de la règle de portée), au plus proche
d'abord ; guichet absent de la table ou sans stand client live : erreur
typée `Unreachable`, jamais l'ordre aveugle. Table recalculée par
`nav/tests/bank_stands.rs` (stand banquier : plan sortant `Disconnected`
en quelques ms ; stand client : plan entrant exact), goldens
`banque_*` dans `routes.txt`.

Versions et reload : chaque réponse porte la version snapshot ;
`reload-nav` rebâtit le snapshot à côté et l'échange atomiquement
(l'in-flight finit sur l'ancien `Arc`, le bump de version rend les
anciennes réponses irrecevables, 1 resubmit max).

Outils offline (one-shots dev, jamais les workers) gardent leur
chargement local : `route` (+ `--paid`, `--ascii`), `map`,
`replay-nav`, `dump-transitions`, `mapbuild`. `direct` branche lui aussi
`plan_client` (+ version) quand le service répond, sinon il refuse le
run au démarrage au lieu de diverger. Référence moteur : goldens
`rs/crates/nav/tests/` (`routes.txt`, `golden.rs`, `paid_only`,
`one_way`, `abort`, `probe`) — le planificateur `plan` n'a pas changé.

## Cache de routes (2026-11-14)

Un seul cache pour tous les bots, dans le service (`nav/src/route_cache.rs`,
`route_path.rs`, `route_splice.rs`, `route_patch.rs`), lié à UNE version
de snapshot : `reload-nav` le vide (`RouteCache::reset`), un calcul de
l'ancienne version qui finit après la bascule n'y entre jamais. Les plans
ne dépendent que du monde statique et des options : l'état live des
portes n'est pas une entrée du planificateur (une porte est toujours
planifiée « ouvrable »), donc rien d'autre ne périme une entrée ; un
échec live revient en arête bannie dans les options de la requête et
n'est servi par aucune entrée qui la traverse.

Recherche (ordre, `op_submit`) :
1. Négatif vivant sous la clé complète (`PlanKey`, tags inclus) :
   NoRoute servi `NEGATIVE_TTL` = 60 s puis recalculé (stoppe les
   tempêtes de replan sur une destination morte).
2. Même départ, même destination : un plan servi à toute requête dont
   les options le valident EXACTEMENT (coût identique au plan frais) —
   tags : la première recherche lève les prérequis, un plan trouvé ne
   dépend que des tags de ses propres arêtes ; `allow_paid` : un plan
   payant qui ne traverse ni péage ni bateau sert un bot sans péage
   (sous-graphe) ; arêtes bannies : bans de l'entrée ⊆ bans de la
   requête et aucune arête du plan bannie ; danger : carrés de l'entrée ⊆
   carrés de la requête et aucune tuile marchée dans les carrés en plus.
   Compteurs `exact` (mêmes options) / `dominated`.
3. Sous-chemin (`prefix`, `suffix`, `infix`) : départ et destination sur
   un chemin en cache, à des points de coupe (tuile atteinte par un pas de
   marche, sans porte : même état de recherche qu'un plan frais), options
   compatibles sur la tranche. Coût re-tarifé pas à pas avec la formule
   du planificateur (marche 1 + danger + battant, porte 10 par arête,
   ride 20 (+50 non vérifié), custom 70) ; si la somme ne retombe pas sur
   le coût du plan, l'entrée ne sert qu'entière. Exact (sous-structure).
4. Épissure (`splice`) : départ et/ou destination à ≤ 8 tuiles
   (`SPLICE_RADIUS`) d'un chemin en cache dont la tranche coûte ≥ 32
   (`SPLICE_MIN_COST` ; plus court, un plan frais coûte autant). Deux
   jambes de marche locales (pas du planificateur, sans porte ni péage,
   hors carrés de danger, borne F2P) rejoignent puis quittent le chemin.
   **Règle epsilon** : servi seulement si `coût − borne_inf ≤
   max(4, 6 % du coût)` (`splice_epsilon`), où `borne_inf` est une borne
   inférieure RIGOUREUSE du plan frais : d(O,T) ≥ d(X,Y) − d(X,O) −
   d(T,Y) maximisée sur les points de coupe X près du départ et Y près
   de la destination (d(X,Y) lu sur les coûts cumulés du chemin optimal,
   d(X,O) et d(T,Y) majorés par deux marches locales de plus). Le surcoût
   est donc prouvé ≤ epsilon ; avec les diagonales la borne est souvent
   serrée (départ le long d'un chemin droit : surcoût prouvé 0).
   Destination bloquée (arbre, guichet) : épissure vers la tuile qu'un
   plan ajusté en cache pour la même cible a choisie, si elle est sur
   l'anneau praticable le plus proche (vue permissive) et ouverte vers la
   cible ; la borne retire l'écart de l'anneau (un plan frais peut finir
   sur une autre tuile de l'anneau). Plan servi ajusté comme le frais.
5. Sinon calcul, avec vol en cours partagé (même clé complète déjà en
   file ou en calcul : on rejoint le ticket, jamais deux Dijkstra).

Candidats : index par paire, par départ, par destination et par
cellules 16×16 traversées par le CHEMIN (une requête teste les entrées
qui passent près de ses deux bouts, 48 au plus). LRU (`ROUTE_CAP` 8192
plans, 64 Mio estimés), moitié la plus ancienne évincée sur alerte RAM.
Aucune recherche sous le verrou du cache : les épissures tournent sur des
candidats sortis du cache (`Arc`).

Hors cache, même chantier CPU : verdicts de poches scellées mémorisés par
version (`pocket::PocketMemo`, une inondation par tuile cible au lieu
d'une par plan et par cible de `walk_costs`) ; `StaticWorld::get` lit
sous verrou PARTAGÉ (le verrou exclusif ne sert plus qu'à peindre un
carré : avant, chaque lecture de tuile sérialisait tous les threads) ;
destination bloquée : Dijkstra s'arrête dès la première tuile au
plancher de rang (`fallback::rank_floor` : anneau praticable le plus
proche, ouvert vers la cible) — même réponse, sans balayer 64 tuiles
au-delà (≈ 8-16 k visites par plan ajusté avant) ; `walk_costs` borne sa
recherche à `64 + 4 × distance de la cible la plus lointaine` (une cible
qui ne s'atteint que par un détour plus long lit `None` ; avant : jusqu'à
500 k visites, 1,8 s) ; les attentes `plan` se réveillent sur une
condvar à la fin du calcul au lieu de tranches de 10 ms.

Preuve (rejeu du trafic réel, `nav/src/service_replay_tests.rs`, ignoré :
`NAV_REPLAY=<extrait des lignes « nav plan … from (…) to (…) »>`,
15 000 requêtes des logs du 14/11, 5 000 de chauffe, profil d'options
par compte, un `walk_costs` par tiers de plans courts ; séquentiel) :
| (10 000 requêtes mesurées) | avant | après |
|---|---|---|
| CPU `plan-pool` | 49,9 s | 12,6 s (−75 %) |
| CPU `plan-query` (`walk_costs`) | 13,1 s | 3,5 s (−74 %) |
| CPU `plan-wait` + `plan-accept` | 4,5 s | 4,5 s |
| CPU total service | 67,6 s | 20,5 s (−70 %) |
| plans calculés (ratés) | 7 682 | 4 898 |
| temps mur (séquentiel) | 132,5 s | 21,5 s |
| `walk_costs` p50 / max | 2,2 ms / 1 613 ms | 0,4 ms / 49 ms |

Qualité : 500 requêtes re-planifiées à froid, surcoût max 4 (somme 58),
aucun au-delà d'epsilon, aucun verdict différent ; goldens `routes.txt`
inchangées. Le rejeu part d'un cache plus froid que le daemon (≈ 77 %
de ratés avant contre ≈ 33 % en live) : en live la part servie est
plus haute, le gain relatif par raté (arrêt au plancher, poches, verrou
partagé) est le même.

## Chemin court et coût fixe par requête (2026-11-14)

Constat après le cache (live 13:45-13:48) : 700-900 plans/min, nav ≈ 7
à 8,5 s CPU/min ; 52 % des plans font ≤ 3 cases sur un niveau (réalignement
sur un poulet, un arbre, un rocher), 77 % ≤ 8, 1 % à distance nulle.
Profil par étape (`service_replay_breakdown`, rejeu avec la vraie liste
de danger, `nav/src/service_replay_live.rs` : `kit::danger::arm` envoie
TOUS les spawns agressifs à moins de 250 cases, ~180 carrés) : le coût
fixe était le `DangerMap` (HashSet de 40-67 k cases, ~7 ms) construit à
chaque Dijkstra, à chaque `route_path::analyze` et à chaque `walk_costs`
— un saut d'une case coûtait ~14 ms. Le reste : JSON ~60 µs, clé ~12 µs,
poche ~20-180 µs (nouvelle cible), Dijkstra court ~90 µs.

- `nav/src/danger.rs` : bitmap dense par niveau sur la boîte des carrés
  (≈ 25 µs pour 180 carrés, lecture = un test de bit), même réponse case
  par case (test contre l'ancien ensemble) ; `DangerMap::clipped` pour
  une fenêtre.
- Chemin court (`nav/src/fast_hop.rs`, `service_fast.rs`) : départ et
  destination sur un niveau, Chebyshev ≤ 8 (`FAST_RADIUS`), destination
  praticable en statique → Dijkstra de marche sur une fenêtre 33 × 33,
  MÊMES pas, prix (1 + danger + battant), borne F2P et ordre (coût, clé)
  que le planificateur, répondu sur le thread `plan-query` qui a lu la
  trame (ni hand-off, ni ticket, ni cache, ni poche, ni `analyze`).
  Servi seulement si PROUVÉ égal : toute arête que la fenêtre saute
  (case portant une porte, arête vers une telle case, bord de fenêtre :
  ≥ 1 ; ride/custom partant d'une case : ≥ `TRANSITION_COST`) donne une
  borne coût(case) + minimum ; servi si coût ≤ la plus petite borne,
  sinon le planificateur. Distance nulle praticable : plan vide coût 0.
  Trame `plan` terminale identique (`outcome: ready`), `submit` en
  ticket instantané. Le premier saut court après démarrage / `reload-nav`
  construit le L2 sur la voie `plan-query` (une fois par version).
- SDK : `walk_to` / `travel_to` vers la case du bot ne contacte plus le
  service (`sdk/src/client/travel_here.rs`, plan vide, options par défaut
  + danger, jamais lues sans replan).
- Coût fixe restant : `decode_options` sans copie, danger trié par
  requête ; cache de routes : listes de danger identiques comparées sans
  tri ni balayage carré × carré (`zones_covered`, `zones_missing`),
  `Patch::read` via un `DangerMap` fenêtré ; cache F2P par zone 8 × 8
  dans les recherches (le verrou monde à chaque voisin) ; `walk_costs`
  en tables à hachage identité + borne de but (avec seulement des cibles
  praticables, une case ne s'étend pas si coût + borne inférieure vers la
  cible la plus proche dépasse l'horizon : pas ≥ 1, ride/custom ≥ 20
  depuis l'atterrissage le plus proche ; mêmes prix, `None` au-delà) ;
  poches : portes lues une fois par case, ensembles à hachage identité.
- Codec JSON du service sorti dans `nav/src/service_codec.rs`
  (`service.rs` 3847 → ~3420 lignes, format inchangé).

Compteurs : `fast_hits` / `fast_zero` / `fast_declined` (op `stats`,
`nav-stats` `fast=a/b/c`, ligne `cpu-profile nav` `plans=… fast=…`,
`hit` reste le taux du cache sur les plans non courts).

Équivalence : goldens (`fast_hop_tests.rs` : sauts du poulailler aux coûts
des logs, distance nulle, porte du château 1516 refusée, carré de danger
contourné), 2 400 requêtes aléatoires (1 123 servies, coût ET cases
identiques au planificateur), rejeu des 8 497 requêtes réelles du 14/11
(5 996 servies, toutes identiques en coût et en cases), `walk_costs`
borné = prix des plans unitaires sur 515 cibles aléatoires.

Mesure (rejeu `service_replay`, 6 000 requêtes réelles mesurées après
2 500 de chauffe, 1 343 `walk_costs`, séquentiel, CPU des threads
`plan-*`) :
| profil d'options | avant (ms CPU) | après (ms CPU) |
|---|---|---|
| danger live (~180 carrés) | 73 334 (pool 55 112, query 14 246, wait 3 707) | 9 972 (pool 6 483, query 2 361, wait 951) −86 % |
| ancien profil (1 carré pour 10 %) | 9 900 (pool 5 830, query 2 052, wait 1 841) | 7 047 (pool 4 597, query 1 457, wait 828) −29 % |

Ratés 2 835 → 707 (70 % des requêtes servies par le chemin court, 412
refusées), `walk_costs` p50 0,6 ms / max 8 ms (avant 9 ms / 43 ms),
qualité inchangée (300 re-planifiés à froid, surcoût max 2, aucun verdict
différent). Prévision live (ratios par groupe du profil danger appliqués
à la minute 13:45-13:48) : pool ≈ 0,35 s, query ≈ 0,55 s, wait ≈ 0,12 s,
accept 0,05 s → ≈ 1,1 s CPU/min au lieu de 6-7 s ; borne prudente (ratios
de l'ancien profil) ≈ 4,5-5 s/min.

## Arêtes ajoutées à la main : bateau de Karamja (2026-11-13)

`gamedata/data/nav_overrides.json` `custom_edges` porte des arêtes que
les dumps ne voient pas (`tables::CustomEdge`, deux genres) :
- `npc_fare` : parler à l'un des `npcs`, répondre `choices` (dans
  l'ordre des pages), payer `fare` pièces prises dans le SAC ; le
  serveur affiche l'interface de traversée puis téléporte sur `to`.
  Marins de Port Sarim (`sailors.rs2`, « Yes please. », pont
  (2956,3143,1)) et officier des douanes de Karamja
  (`customs_officer.rs2`, voyage, fouille, « Ok. », pont (3032,3217,1)) ;
- `loc` : cliquer `loc` op `op` en `at` (passerelles `_off` de
  `gangplank.rs2` : téléport puis telejump sur le quai (2956,3146,0) ou
  (3029,3217,0)).

Une arête à `fare` est un chemin payant comme le péage : seulement avec
`allow_paid`, et elle exige `coins:<fare>` en sac (ajouté au chargement).
Diagnostic : sans `allow_paid`, `PaidOnly` bloqué par
`Custom(<nom>) missing coins:30` ; payant sans pièces,
`NeedsRequirement`. Le plan porte tout ce que l'exécuteur doit savoir
(`custom_edge::CustomEdgeRef` : genre, loc, tuile, PNJ, réponses,
tarif ; sérialisé dans le segment `custom`). Exécuteur SDK
`client/travel_custom.rs` : `npc_fare` vérifie le tarif en sac, parle au
PNJ visible le plus proche, choisit les réponses (`fare_pick`), continue
les pages, attend l'interface puis l'arrivée (rhum confisqué : un second
dialogue) ; `loc` clique et attend le saut. Hors ligne :
`orchestrator route 3029 3217 0 2915 3150 0 --paid` (`--paid` compte
aussi les tarifs comme portés). Goldens `bateau_*` (`uses_custom=`,
`blocked_custom=`). Vérifié en live le 2026-11-13 (admin, pas à pas via
`call`) : Captain Tobias → (2956,3143,1), passerelle 2082 → (2956,3146,0),
officier des douanes (voyage, fouille, « Ok. ») → (3032,3217,1),
passerelle 2084 → (3029,3217,0), 30 pièces prises à chaque traversée ;
l'exécuteur SDK lui-même attend un déploiement pour son essai live.

## Péage d'Al Kharid : côtés par connexité (2026-11-14)

`nav/src/toll_side.rs` (pur) : `flood` remplit à pied un niveau dans une
fenêtre (pas serveur, portes ordinaires ouvertes, portes à péage et
portes bloquées à la main fermées, exigences ignorées : un remplissage
optimiste resté séparé prouve la séparation). `derive_region` part des
deux cases de la porte (3268,3227) / (3267,3227) dans la fenêtre
x 2880-3400, z 3100-…, et monte le plafond nord par dichotomie tant que
les deux côtés restent disjoints : la clôture s'arrête à z 3329 (au-delà,
(3267-3270, 3330) relie les deux côtés à pied : le détour gratuit par
le nord, ~300 cases de Lumbridge à la banque d'Al Kharid). Les cases non
marchables (eau d'un banc, rochers, arbres) prennent le côté de leurs
voisines marchables les plus proches (anneau Chebyshev ≤ 3, majorité,
égalité = aucun côté). Le côté est sérialisé dans
`gamedata/data/toll_region.json` (test d'or `tests/toll_region.rs`,
réécriture `LIFESCAPE_BLESS_TOLL_REGION=1`), lu par `gamedata::toll` :
cerveau et scripts testent la même région, jamais x = 3268 (la clôture
passe à x 3266 vers z 3240, à x 3270 en haut ; la rive sud d'Al Kharid
descend à x 3259).

## Danger et destinations substituées (2026-11-12)

- `NavOptions::danger` : carrés `(x, z, niveau, rayon)` où un PNJ agressif
  attaquerait le bot. Chaque pas de marche dans un carré coûte
  `DANGER_STEP_COST` (8) de plus : la route contourne quand un chemin
  existe, traverse sinon (destination dangereuse comprise, jamais
  d'interdiction ; un surcoût plus fort faisait exploser la recherche
  vers une destination en zone). Calculés côté scripts
  (`kit::danger::arm`, appelé par `check_events`) : `hunts_players`,
  agressif selon la règle serveur (`Npc.ts` : passif seulement si
  `combat > vislevel × 2`, jamais dans le Wilderness) ET létal
  (`vislevel ≥ 5` ou `vislevel ≥ combat` : les gobelins niveau 2 ne
  comptent pas pour un combat 3), rayon `huntrange + wanderrange + 1`, l'errance
  venant des configs serveur (`gamedata/data/npc_ranges.json`, généré par
  `orchestrator dump-npc-ranges` ; gardes de la prison 11, mages 3, défaut
  moteur 5), spawns à moins de 250 cases. Mêmes carrés pour le choix de
  spot. Golden : `nav/tests/danger.rs` (Lumbridge → banque de Draynor :
  43 cases sous les gardes → 0, pour 12 pas de plus).
- Destination substituée lointaine : un plan dont `adjusted_dest` est à
  plus de 4 cases (Chebyshev) de la cible, ou sur un autre niveau, est un
  `NoRoute` côté SDK (`plan_travel`), rien n'est marché (avant : un spot
  derrière le péage devenait un coin du marais où le bot restait).
- Limite connue : `walk_costs` renvoie `None` pour une case bloquée
  (arbre, eau) et un lot contenant une cible inatteignable épuise la
  recherche ; les pickers ne peuvent donc pas encore vérifier
  l'atteignabilité d'un spot avant de le choisir (à faire côté nav :
  coût vers la case d'appui la plus proche, recherche bornée).
