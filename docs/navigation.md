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
cache L1 (1024 entrées, 32 Ko max/entrée), garde RAM (warn/shed),
timeouts `SUBMIT_TIMEOUT` 10 s / `POLL_TIMEOUT` 5 s / garde client
`SERVICE_WAIT_MAX` 30 s (`rs/crates/nav/src/service.rs`).

Ops du service (`dispatch`) : `submit` / `poll` / `cancel` pour les plans,
lectures batch hors queue `door_defs`, `tiles_flags`, `static_locs_near`,
`in_f2p`, `toll_fence`, `wants`, `diff`, `debug`, `transitions_list`,
`probe_targets`, `doors_near`, plus `stats`. Chaque op batch est comptée
et exposée dans `nav-stats`.

Planification 100 % distante (`Bot::plan_travel`,
`rs/crates/sdk/src/client/travel.rs`) : sans monde local, le plan vient
du service ou l'appel échoue en `Error::Travel("nav-down: …")` — down,
refus, timeout, version biaisée après 1 resubmit. Jamais de plan local.
Sans tables locales, les options sont celles par défaut (`f2p_only`, sans
`satisfaits`) ; sans `plan_client`, `walk_costs` rend `None` (ordre
Manhattan conservé). Le replan après échec replanifiable repasse par le
service avec l'arête bannie. `nav-down` ne déclenche ni reconnect ni
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

Mobilier qui « s'ouvre » (catégories `chest_closed` 33, `cupboard_closed`
35, `drawer` 37 : la variante ouverte bloque toujours) n'est jamais une
porte (`loctypes.rs`) : avant, le plan « ouvrait » les tiroirs 350 de
Varrock pour marcher dessus (DoorDidNotOpen). Golden `nav/tests/furniture.rs`.

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
