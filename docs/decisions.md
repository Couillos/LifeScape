# Décisions d'architecture

- 2026-11-06 — Registre unique via `lifescape_scripts::names()` : une seule source scripts. Rejeté : double registre orchestrateur (chemins `.py` périmés).
- 2026-11-06 — `dump-transitions` porté en sous-commande Rust fidèle, test de comparaison sémantique. Rejeté : garder le générateur Python (deux codebases).
- 2026-11-06 — Données rapatriées dans `gamedata/data/`, chemins lus via `paths::root()`. Rejeté : laisser le Rust lire le paquet Python.
- 2026-11-06 — Python supprimé, gardés `orchestration/static/` (servi par `web.rs`) et `context/`. Rejeté : suppression totale (casse le dashboard et les notes).
- 2026-11-06 — Std only + `#![forbid(unsafe_code)]` par crate, JSON/HTTP/WS maison. Rejeté : tokio/serde (poids et GC/process inutiles ici).
- 2026-11-06 — Attentes jeu en ticks (`TICK_MS = 20`), garde-fous process en secondes. Rejeté : secondes partout (comportement différent à 20 ms vs 600 ms).
- 2026-11-06 — Parseur JSON unique (`lifescape-json`), gamedata et `devtools_check` branchés dessus. Rejeté : trois parseurs ad hoc (divergence silencieuse).
- 2026-11-06 — `buffer.rs` (Reader/Writer) déménagé dans `lifescape-nav`, le SDK le réexporte : un seul lecteur d'octets. Rejeté : dépendance nav→sdk (cycle) et duplication du lecteur.
- 2026-11-06 — Override 1530 supprimé : live prouve portes 1530 normales (reach suffit), aucune entrée requise ; §7.15 couvert par route `rempart_1530` + ce motif. Rejeté : surcharger sans donnée live.
- 2026-11-06 — Péage PaidOnly synthétique validé : Al-Kharid contournable à pied (détour 220 tuiles via `route`), branche couverte par `paid_only.rs`. Rejeté : or PaidOnly réel (aucun péage isolé en F2P).
- 2026-11-06 — Exemption budget 300 ms pour NoRoute-par-épuisement : `chateau_scell` met 3669 ms à prouver Disconnected (release) ; budget = plans réussis (max 140 ms). Rejeté : borner la preuve d'échec.
- 2026-11-06 — Coûts 10/20/50 gardés : DOOR 10, TRANSITION 20, UNVERIFIED 50 ; calibration live reportée phase 7 (prober). Rejeté : calibrer sans mesures d'arrivée.
- 2026-11-06 — Bornes d'exécution mesurées live : DOOR_SWING_WAIT 23 (swing vu à 17 ticks), RIDE_WAIT 41 (atterrissages 2-9, 26 max en live), CROSS_WAIT 7 (pas 1-4), PRE_OPEN_WAIT 17 (couvercle souvent déjà ouvert, calibration prober phase 7). Rejeté : bornes rondes sans source.
- 2026-11-06 — STALL 13 : 19 jambes live à 0,6-6 ticks/tuile (médiane 2), 13 = silence > 2x pire écart + marge pompe. Rejeté : seuil rond (10/15 sans source).
- 2026-11-06 — Budget temporel cost*4+347 ticks : 4 = 2x médiane marche, 347 = file portes + stalls ; aucun Timeout sur les runs live phase 5. Rejeté : budget fixe (ordres de grandeur différents).
- 2026-11-06 — Prober : premier stand reach-valide (ordre planificateur), mort→skip sans verdict, gate REBUILD 13 ticks anti-cache, runs filtrés fusionnés. 244 ok (pénalité levée), 98 no_move (exclus). Rejeté : multi-stand (un seul stand : garder les lignes non retestées) et réécriture complète (perdrait les lignes non retestées).
- 2026-11-09 — Workers sans carte : la session ne charge ni paquet ni monde ni tables, seul le handle `plan_client` + version épinglée ; le daemon porte l'unique snapshot nav. Rejeté : carte par worker (RSS et double parse par session).
- 2026-11-09 — Retrait du plan local : sans service, `travel_to`/`walk_to` rendent `Error::Travel("nav-down: …")`, jamais de Dijkstra worker-side ; la session reste vivante et les scripts réessaient par leurs boucles. Rejeté : fallback sync (divergence silencieuse entre plan servi et plan local).
- 2026-11-09 — Plan enrichi : le service renvoie par segment porte `open_op`, péage, `stand`/`cross_to`, et le verdict ride ; codec tolérant aux champs absents. Rejeté : lecture du pack en worker pour exécuter (recréerait la dépendance carte).
- 2026-11-09 — Lectures service en batch hors queue (`door_defs`, `tiles_flags`, `static_locs_near`, `in_f2p`, `toll_fence`, `wants`, `diff`, `debug`, `transitions_list`, `probe_targets`, `doors_near`), comptées dans `nav-stats`, 1 resubmit anti-skew puis `nav-down`. Rejeté : tickets d'admission + caps de queue pour ces lectures (pas de calcul, pas de file).
- 2026-11-09 — Dossier d'échec : le worker envoie sa fenêtre live, le service calcule `static_live_diff` (rayon 10) ; `replay-nav` classe toujours A/B/C. Rejeté : diff worker-side (exigeait la carte en worker).
- 2026-11-09 — Gamedata en statiques compilées (`gamedata/build.rs` → modules générés dans `OUT_DIR`, `&'static`, index triés + `binary_search`, `rerun-if-changed` par dump, pages `.rodata` partagées entre workers) au lieu d'un service gamedata. Rejeté : IPC gamedata (RAM résidente + latence + mode panne pour des dumps stables en prod).
- 2026-11-09 — `direct` branche `plan_client` (+ version) comme une session quand le service répond, sinon refuse le run au démarrage. Rejeté : run direct sans handle (rendait `nav-down` à chaque pas, divergent de la session).
- 2026-11-09 — Persistance SQLite confinée (D3) : nouveau crate `lifescape-store`, seul autorisé à dépendre d'une bibliothèque externe (`rusqlite =0.41.1`, fonctionnalité `bundled`, compilé avec le projet, aucun système requis hors `cc`) ; API typée sans fuite SQL, `#![forbid(unsafe_code)]` conservé, WAL, migrations versionnées, un seul ouvreur (le daemon). Rejeté : JSON par compte (pas de requêtes) et SQLite dans chaque crate (dépendance dispersée).
- 2026-11-09 — Base de temps D1 : engagements et budgets en ticks, graphes et historiques en horloge. Rejeté : tout en ticks (graphes incomparables entre dev et prod) et tout en horloge (comportement différent à 60 ms vs 600 ms).
- 2026-11-09 — Invendus au plancher D2 : haute alchimie si rentable (niveau 55/21, runes au prix market), sinon vente au magasin qui paie le plus, sinon jet (D10) pour le non échangeable invendable. Rejeté : détruire sans ordre et garder indéfiniment (banque 60 cases).
- 2026-11-09 — Périmètre F2P uniquement D4 : comptes, objets (`members = false` et `tradeable = true`), recettes, sorts, 18 quêtes du registre, 15 skills, géographie `in_f2p`. Rejeté : contenu membres (monde F2P confirmé, `map_members = false`).
- 2026-11-09 — Market prioritaire D5 : pause inline générique (généralisation du rescue), compteur d'engagement suspendu pendant la pause, retour à la position de départ, rescue d'abord. Rejeté : arrêt + relance (état à reconstruire) et décompte des pauses dans l'engagement.
- 2026-11-09 — Tous les échanges place de Varrock D6, autour de la fontaine (3213, 3429, niveau 0), anneau de cases adjacentes calculé via `tiles_flags`. Rejeté : échanges sur place (pas de liquidité visible, trajets non mesurables).
- 2026-11-09 — Tick D7 : 60 ms en dev, 600 ms en prod, réglé par `NODE_TICK_RATE` (défaut 600). Rejeté : taux unique (dev trop lent ou prod faussée).
- 2026-11-09 — Aucun plafond de pauses market D8. Rejeté : plafond (bot souvent apparié pénalisé sans raison système).
- 2026-11-09 — Équipement gardé D9 : meilleur objet F2P uniquement pour les styles entraînés par le but. Rejeté : panoplies de tous les styles (banque 60 cases).
- 2026-11-09 — Objets non échangeables et invendables devenus inutiles D10 : jetés. Rejeté : conservation (fuite de cases).
- 2026-11-09 — Aucune taxe ni frein à l'inflation D11 : masse monétaire seulement mesurée et affichée. Rejeté : taxe (équilibrage prématuré, hors socle).
- 2026-11-09 — Tout passe par le market D12 : aucun échange hors carnet ; objet sans repli D2 reste au carnet jusqu'à 1 pièce. Rejeté : dons et prêts entre bots (échanges invisibles, non mesurables).
- 2026-11-09 — Wilderness ouverte à tous D13, sans préparation ni détour banque ; pertes gérées par replanning. Rejeté : profil risque et équipement d'expédition (complexité sans gain F2P).
- 2026-11-09 — Graphe de prix en canvas maison D14, dans la lignée de `items.js`. Rejeté : bibliothèque externe (std only).
- 2026-11-09 — Choix market/farm D15 : trait `market_affinity` tiré à la création, fixe, qui pondère les coûts. Rejeté : choix pur coût (tous les bots identiques) et trait changeant (comportement illisible).
- 2026-11-09 — Rendements D16 : tout compte (produits, drops, spawns au sol, ramassés), provenance étiquetée non excluante ; seuls `shop_bought`/`traded` exclus. Rejeté : exclusion des drops (rendements faux en combat).
- 2026-11-09 — Banque pleine d'invendus D17 : jeter des piles entières, moins chère en valeur totale d'abord, ordre annulé. Rejeté : jeter partiel (ne libère aucune case).
- 2026-11-09 — Bots marchands D18 dans le socle : archétype du but « devenir riche », méthode `market:flip`, poste fixe place de Varrock. Rejeté : market sans marchands (liquidité nulle entre bots aux buts dispersés).
- 2026-11-09 — Tendances publiées D19 calculées sur les seules données publiques du market (carnet, exécutions), bonus de rentabilité lissé. Rejeté : signaux sur engagements des autres bots (mémoire partagée déguisée, contraire à D22).
- 2026-11-09 — Kit de comportements D20 : onze briques partagées, six archétypes, activités < 200 lignes, briques < 500. Rejeté : scripts monolithiques (woodcut 1846 lignes de code, décisions de vie dispersées).
- 2026-11-09 — Batterie de tâches D21 : le brain fixe objectif et ressource exacte ; un script ne choisit ni ressource ni palier, il trouve seulement où l'exploiter. Rejeté : choix de palier côté script (replanning impossible).
- 2026-11-09 — Réalisme D22 : aucune mémoire partagée entre bots ; un bot ne sait que ce qu'il voit, vit et ce qui est public (prix, rendements publiés). Rejeté : pénalité de foule et offres en route (connaissance cachée).
- 2026-11-09 — Fabrication et cuisine D23 dans le socle, premier client de la brique `produce`. Rejeté : socle sans fabrication (extensibilité non démontrée).
- 2026-11-09 — Décisions de vie de woodcut D24 isolées dans `legacy/bootstrap.rs` jusqu'au brain, puis supprimées. Rejeté : suppression immédiate (parité live perdue) et conservation (dette).
- 2026-11-09 — Arguments CLI conservés D25 via parseur commun vers `TaskSpec`. Rejeté : nouveau CLI incompatible (tests live et habitudes cassés).
- 2026-11-09 — Contrainte dure D26 : plusieurs centaines de bots dans 20 Go ; enveloppe RAM/CPU par composant, mesure PSS, admission de sessions, chantiers E1-E15. Rejeté : croissance sans enveloppe (effondrement silencieux).
- 2026-11-09 — Cible D27 : 300 bots 24 h/24 sur 8 cœurs / 20 Go / SSD avec serveur, base et dashboard ; grands runs au tick de prod. Rejeté : machine dédiée par composant (hors moyens).
- 2026-11-09 — Aucun plafond de présence à la fontaine D28. Rejeté : quotas de place (file artificielle, rendez-vous manqués).
- 2026-11-09 — Observabilité intégrale D29 : minimap, spectate et outils de debug conservés, optimisables jamais appauvris. Rejeté : suppression d'outils pour gagner du CPU (indispensables au développement).
- 2026-11-09 — Heartbeat espacé D30 en deux flux (léger fréquent, complet en diffs espacé). Rejeté : statu quo 1 snapshot/s/bot (~33 Ko/s/bot extrapolés à 300).
- 2026-11-09 — Plusieurs bots par process D31 acceptés dès gain CPU mesuré ≥ 20 %, groupes de 10 à 25. Rejeté : adoption aveugle (RAM déjà à 8-9 Mo/bot, le gain ne peut venir que du CPU).
- 2026-11-09 — Logs complets D32 : optimisations sans perte seules (buffer, déduplication, rotation). Rejeté : troncature (information perdue pour le diagnostic).
- 2026-11-09 — Pas de sessions humaines D33 : les bots jouent en continu. Rejeté : plages humaines (complexité sans besoin).
- 2026-11-09 — Logs D34 : 100 Mo par bot en rotation (~30 Go à 300) ; ≥ 60 Go libres exigés avant endurance (29 Go libres mesurés : faire de la place). Rejeté : logs non bornés (SSD rempli).
- 2026-11-09 — RAM n'est plus la contrainte D35 : 8-9 Mo PSS/bot mesurés, plafond 20 Mo/bot ; E7 se tranche sur le CPU de prod. Rejeté : multi-process pour la RAM (gain négligeable).
- 2026-11-09 — CPU du daemon contrainte n° 1 D36 : ≤ 0,33 % d'un cœur par bot au tick de prod ; E2, E3, E4, E8, E13, E14 obligatoires avant L5. Rejeté : market et brain avant sobriété (charge ajoutée sur le poste saturé).
- 2026-11-09 — `TICK_MS = 20` remplacé D37 par `waits::tick_ms()` (`NODE_TICK_RATE`, 600 par défaut) ; règle 5 d'AGENTS.md corrigée. Rejeté : constante 20 (dossiers d'échec annotés d'une fausse durée).
- 2026-11-09 — Tests live limités à 20 bots D38 ; tenue à 300 par coûts par bot × 300 + banc E15 à 300 faux workers. Rejeté : live à 300 (risque serveur et SSD, hors moyens).
- 2026-11-10 — L7 planner HTN + brain : nouveau crate `lifescape-planner` (pur, `lifescape-gamedata` en lecture seule) et `orchestrator/src/brain.rs` (vies `run/lives/`, autopilote, `create-user`) ; `market:flip` dans le script market existant ; `legacy/bootstrap.rs` supprimé (D24), woodcut autonome (blocked au lieu de chasser). Rejeté : but en JSON avec table de mapping (mapping en Rust exigé), nouveau script marchand (flip suffit), shop:buy exécutable (magasins = bornes de prix seulement).
- 2026-11-10 — R8 sans compte neuf : tutorial cassé à `chop`, runs depuis comptes post-tutorial existants ; le cerveau démarre au premier idle. Rejeté : debug du tutorial dans L7 (hors périmètre planner).
- 2026-11-10 — L7 livré : planner HTN + brain + `market:flip`/`shopbuy` + suppression `legacy/bootstrap.rs` (D24 close) ; alternatives source-aware (item_sources), couverture market/shop, auto-pick du stock marchand hors équipé, abandon en cascade, mapping method→(script,args) en Rust. Rejeté : shop:buy exécutable sans repli banque, farm des outils, méthodes en JSON.
- 2026-11-10 — Durcissement superviseur (découverte live L7) : un `job.inner` qui ne répond plus ne parque plus rien — try_lock partout sur le chemin chaud (supervise, ingest, convene, brain, describe avec repli `contested`), plus jamais de jobs verrouillé pendant les lectures d'état. Cause racine du verrou fantôme non isolée (verrou à 2 sans détenteur visible, aucun thread fautif identifié par backtraces) ; R8 passe malgré tout (autopilote vérifié sur petits buts, parité R1, shopbuy/flip live). À suivre : traçage du détenteur et test live long (pêche 40 + cuisine 30 + minage 30 + 1 quête).
- 2026-11-10 — L11 durcissement (1/4) market restart D39 : `MarketService::open` recharge les livres depuis le store (`resting_orders` open+matched non dormant, `Book::relist` à rang de création, `order_item` rebâti, matched orphelins rouverts, deals non finaux faillés `daemon restart`). Vérifié live : 2217 ordres / 70 livres rechargés après restart, 3 deals en vol faillés proprement. Rejeté : rejeu via `place` (recréerait fills/deals/executions fantômes).
- 2026-11-10 — L11 durcissement (2/4) ledger alimenté D40 : le contrôle de couverture tournait contre un ledger jamais crédité (tout `place` live refusé `holds 0`) ; chaque tour convene (10 s) reflète inv+banque+porté (ids numériques du snapshot, <30 s) via `sync_account` (SET). Fenêtre de sur-engagement connue entre deux syncs : l'échange physique double-vérifie, donc 0 perte, deals en échec + fiabilité en pénalité seulement. Rejeté : réservation à la pose (nouveau mécanisme, hors durcissement).
- 2026-11-10 — L11 durcissement (3/4) budget mêlée cumulatif : `run_spot` réinitialisait `ticks_limit` à chaque appel `fight` (un par cluster) sans garde externe → dépassement ~2,5× ; délai global `run_started`, reliquat passé à chaque `fight`, garde en tête des deux boucles, helper pur `budget_left` testé. Rejeté : toucher à `fight` (sémantique par appel conservée).
- 2026-11-10 — L11 durcissement (4/4) superviseur : compteur de contention par compte (log à 10 puis tous les 60 tours, nomme le compte coincé) ; cause racine du verrou fantôme toujours non isolée (impasse documentée, pas de contournement supplémentaire). Connu et accepté pour l'endurance : `watchdog_tick` dort 1,5 s + sonde jusqu'à 15 s dans le thread superviseur unique (≤17 s de supervision gelée par wedge), `supervise_once` bloque jusqu'à 5 s + `unload` 40 s par `send_worker` ; les wedges restent rares et le watchdog les résorbe. Rejeté : respawn asynchrone (machinerie nouvelle avant la recette).
- 2026-11-10 — L11 `web.rs` (1629 l., règle 1000 l.) : scission en `web_life.rs`/`web_economy.rs` REPORTÉE après la recette (risque de câblage du dispatch HTTP à la veille d'un run 72 h, fichier stable et endpoints déjà factorisés en fns pures). Rejeté : scission immédiate (churn sans gain d'endurance).
- 2026-11-10 — L11 disque D34 NON TENU : 24-25 Go libres mesurés (< 60 Go exigés) ; endurance 72 h sur SSD BLOQUÉE tant que la place n'est pas libérée (paliers + banc maintenus, recette en attente de disque). Rejeté : run d'endurance quand même (usure SSD + risque de panne).
- 2026-11-10 — L11 tick prod : `NODE_TICK_RATE` est IGNORÉ par cette révision (WorldConfig ne l'applique qu'en migration legacy `.env`, jamais avec `world.json` présent ; rien ne lit `process.env.NODE_TICK_RATE`). Le tick vient uniquement de `engine/data/config/world.json` → `node.tickRate` (60 mesuré, 1,67 t/s après passage à 600). CORRIGE L0/annexe-C et §5.11 (qui affirmaient l'inverse). Serveur relancé en conteneur `lostcity` avec `tickRate: 600` (fichier local non suivi, aucun code serveur touché, règle 10 intacte) ; rollback : remettre 60 + `docker restart lostcity`. Rejeté : variable d'environnement (sans effet).
- 2026-11-10 — L11 paliers prod (idle, tick 600) : P5/P10/P20 → daemon 2,73 % constant (0,13 %/bot), workers 0,07-0,13 % (PSS 3,6-4,8 Mo, 2 threads, RSS ~14 Mo), daemon PSS 62→65 Mo (+0,19 Mo/session), écritures daemon 0,8→1,2→2,3 Ko/s (0,11 Ko/s/bot ≤ 3,5), pushes 0,42-0,58/s/session, serveur thread monde 16,8→16,2→18,6 ms/tick (pente ≈ 0,1 ms/joueur → ~50 ms à 300, marge ×6 sous les 300 ms). Actifs (3 scripts) : workers 0,17-0,23 %, daemon 8,6 % en moyenne 5 min (pics à 13 % sur misses de plans + polls ~16/plan : E3/E14 restent le correctif). Tous les postes tiennent l'enveloppe §5.11 × 300 sauf le daemon actif (0,33 %/bot dépassé : ~2 %/bot actif) — l'admission (recalibrée ci-dessous) refuse avant de dégrader.
- 2026-11-10 — L11 T7 Varrock : 9 bots regroupés autour de la fontaine, aucun delta serveur mesurable (19,6 ms/tick en mouvement vs 18,6 idle) ; snapshots +38 % en volume (joueurs visibles) ; daemon PSS +85 Mo en une fois (paint nav à la demande, `painted` 1→65, nav rss 72→154 Mo) puis pente nulle sur 4 min (saturation, pas de fuite). `walkto` Rimmington→Varrock sans péage reste bloqué au départ (toll fence, comportement nominal non payé).
- 2026-11-10 — L11 E15 : `bench-l2r 300 60` → 25 718 plans (429/s), 100 % L1 (jambes chauffées), 0 refus cap/full/stale, 0 unknown, 25 718 pushes ingérés. Limites : 0 % de misses (chemin calcul non prouvé en charge), ni la pool du daemon ni le market ne sont exercés par le banc — couverts séparément par une rafale directe (1000 ops mixtes res-stats/quote en 0,2 s, p99 17 ms, 0 erreur).
- 2026-11-10 — L11 admission recalibrée D41 : le ratio L0 (10 %/bot en dev) sous-estimait d'un facteur ~20 au tick prod (gate à 47 bots pour un daemon à 2,7 %). Nouveau socle mesuré : 300 unités de trafic/s par cœur (2,7 % à ~9/s idle prod) ; gate à 300 au trafic idle, ~260 au trafic walkto, ~150 à 2/s/bot ; plafond absolu 300 inchangé. Re-tune avec trafic actif (plans+reports) en suspens. Rejeté : statu quo (contredit O9).
- 2026-11-10 — L11 E7/L2m TRANCHÉ : plusieurs bots par process REJETÉ (mesures prod : PSS 3,6 Mo/bot, 2 threads/bot ; le plancher fixe est négligeable, aucun gain CPU ≥ 20 % atteignable). T2 (MALLOC_ARENA_MAX) non refait : RSS 14-15 Mo/bot déjà dans l'enveloppe, valeur marginale. Rejeté : complexité multi-session sans gain mesuré.
- 2026-11-10 — L11 disque : `df` remonté à 49 Go libres (place libérée par un tiers, cause inconnue) — toujours < 60 Go D34 : endurance 72 h BLOQUÉE, paliers + banc maintenus. T8 (tempête open) : doseur E10 (2 s) tient, refus `open-rate` propres aux ouvertures rafale. T13 navigateur : non fait (aucun navigateur ici ; dashboard :200, `/api/lives` OK).
- 2026-11-10 — L11 recette : 20 t-bots + admin en place (17 autopilotes avec vies et buts mélangés dont 2 `rich:merchant`, 3 tutorial-idle en ballast, admin sans vie = diagnostic pur). ÉCARTS À LA SPEC §6.6 : 17 autopilotes au lieu de 20 (comptes t22-t25 et t03/t05/t09 bloqués au tutorial, `chop` cassé, aucun compte neuf viable) et 0 Iron (tout Iron serait un compte neuf). Options : téléporter 3 comptes tutorial sur le continent via `goto` (outil dev autorisé, setup opérateur, zéro code) puis `create-user --iron`, ou recette à 17. En suspens : décision utilisateur. Comportements à surveiller : `market:buy` à 0 coins échoue puis replanifie farm (wart coût L7, préexistant), trajets banque inter-villes longs au tick prod (t19 pêche→banque Varrock-est depuis Lumbridge).
- 2026-11-10 — L11 pêche : le bootstrap enterrait son propre outil (keep `coins` seul → `bank_deposit_all` banque le filet, puis `blocked{missing: net}` sans jamais tenter de reprise ; d'autres skills (cuisine) bancaient aussi le filet, irrécupérable). Correctif scripts (périmètre bugfix) : keep = coins + `FISH_SPEC` connus, puis `restore_banked_tool` avant achat/blocage (miroir du bootstrap minage) ; nouveau `restore_banked_tool` (retrait rang, sans réassemblage manche/tête — les filets sont unitaires), `restore_from_bank` y délègue pour les outils entiers. Pas de test unitaire (chemin live-only, comme `restore_from_bank`) : vérification live (reprise pêche + log `ok net restored`). Rejeté : toucher à `restore_from_bank` en place (risque hache/pioche) et au keep cuisine (le banking inter-skill est sain dès que la reprise marche).
- 2026-11-10 — L11 amorçage : 13 filets distribués via `giveother` admin (outil dev autorisé, setup opérateur, zéro code) aux bots ruinés sans outil (t01,t02,t04,t06,t07,t08,t14,t15,t16,t18 + joe,t19,olivia) ; impact économie ~65 pièces (négligeable, traçable en holdings). Cause : impasse bootstrap planificateur (broke+toolless, aucune voie de revenu sans outil — dette L7/planificateur, hors périmètre L11) + bug pêche ci-dessus. Ouvert : revente par le flip marchand de son propre outil (`hors équipé` n'exclut pas le porté — politique stock L13/planificateur), sorts 72 h à décider (re-subvention vs laisser caller).
- 2026-11-10 — L11 vérification pêche (20 min live, 12 bots actifs) : zéro `blocked{missing: net}` après correctif (contre boucle dominante avant), t16 cuisine ses prises (chaîne fish→cook attestée), marchands t07/t08 en `market flip` continu. Reste ouvert : revente par le flip de son propre outil (politique stock, planificateur), olivia_king coincée > 90 min en `rescue: tower` (rescue à examiner), écritures daemon 6,2 Ko/s à 12 scripts actifs (~0,3-0,5 Ko/s/session active).
- 2026-11-10 — P0.2 interfaces gelées (PRD 2, vague 0) : `sdk::provenance` passe à 10 valeurs (`death`, `unattributed` ajoutées, `picked` retiré du contrat D45) ; `planner::catalog` gèle `Activity` (id, skill, niveau, arch R/P/S/T/I/C/M/Q, époque P1/P2, prérequis, intrants, produits, poste, xp/action) ; `kit::chain_spec` ajoute `TaskSpec.chain: Option<ChainSpec>` (maillons, ticks totaux, xp par skill, valeur finale) sans toucher au CLI v2 ; `orchestrator::api_shapes` gèle les JSON market/rates/lives/holdings ; `sdk::client::travel_hook` gèle le rappel entre tronçons (Continue/RandomEvent/Fled/Dead) + `is_no_route` sur `Error::Travel` ; `nav::walk_costs` gèle l'op multi-cibles N1 (un Dijkstra, arrêt à toutes cibles, `Null` si inatteignable). Écarts assumés : `rates_service::is_farm_provenance` accepte encore `"picked"` (tolérance legacy, P1.4 la retire) ; bougies servies sans `n` (P1.3 l'ajoute depuis les exécutions) ; index lives et holdings actuels remplacés par les formes gelées (P1.5 + migration store). Rejeté : dupliquer la provenance dans un nouveau fichier (deux sources de vérité) et modifier `web.rs`/`brain.rs`/`daemon.rs`/`nav/service.rs` (splits P0.1 en cours).
- 2026-11-10 — P3.6 XP source unique : tous les priors XP des chaînes sont du dump brut / 10 (recettes, `extra_recipes`, `methods`, `spells`) ; un seul test (`chain_priors_match_dump_tenths`) épingle les taux par famille. Les rendements publiés (rates, mesurés dès n ≥ 3) remplacent les priors à froid, jamais l'inverse. Rejeté : XP codée par famille sans source (divergence silencieuse).
- 2026-11-10 — P3.6 mapping en Rust, jamais en JSON : `planner/src/mapping.rs` est le contrat méthode → (script, args) pour les 13 familles P3 (`smelt/smith/cook/tan/fire/spin/pottery/jewelry/runecraft/pick/mill/fill/essence`) plus inventaire (`string/alch/superheat/enchant`), styles (`fight:ranged|magic`), collecte (`gather:sheep|wheat`) et `craft:` routé (cuir → leatherwork, reste → cook). Conventions : ressource positionnelle d'abord (chaque script la lit en premier), `--node/--budget` après ; suffixes documentés (`bake:bread` → `bake_bread` sans positionnel, `spin:` résout l'intrant laine depuis le dump, `cook:` résout le cru, `pottery:/mill:/essence:` sans positionnel). Échec typé seulement si script absent : map `None` ou script inconnu au registre → nœud `Failed{no-script:…}` (plus de spin silencieux). Rejeté : table de mapping en JSON ou en lifestore (mapping en Rust exigé, D64).
- 2026-11-10 — P3.6 D64 strict : `usable_chains` EXCLUT les P2 simples (ni déblocage ni produit du but) au lieu de les déférer ; P1 gagne les égalités XP/tick (test d'égalité dédié). Filtre comparé au vrai produit de la chaîne. Rejeté : P2 toujours joignable (dépenses hors-but hors-ère).
- 2026-11-10 — P3.6 couverture 1→99 : chaque skill F2P a une méthode dump par tranche de 10 (`best_method` épinglé 1→99), chaque palier du brain émet une méthode mappée, chaque étape de chaîne est mappée, et les chaînes tiennent au volume (1000 unités) au-delà du dernier palier §8. Chaînes ajoutées : `enchant` (amulette → sort), `bury` (vaches → os, enterrement en boucle de combat, pas de script autonome), `lesser_alch` (démons mineurs → rune_med_helm → alchimie ; drop non garanti : makes 1.0 = borne haute, rates mesurés priment). Rejeté : chaînes P2.5 retouchées au-delà du renommage natif (`craft:gold_ring` → `jewelry:`, `craft:pot` → `pottery:`, `alch:high` → `alch:<objet>`, `string:` aux vrais noms non-enfilés).
- 2026-11-10 — P3.6 5e four STATUÉ (revue P2.1) : la tuile (3081, 9496) est le four de l'île du tutoriel (transitions `newbieladder`, zone tutoriel uniquement, inatteignable après) ; les 4 sites du catalogue §5.7 tiennent (Al-Kharid, Falador, Lumbridge, Wilderness), test `four_mainland_furnace_sites_plus_tutorial`. Renvoyé P2.1/nav : exclure les stations du plan tutoriel des requêtes F2P ou barrer les transitions retour (un bot continental ne doit pas reroutrer vers le tutoriel). Rejeté : compter un 5e site continental (aucun) et supprimer la tuile du dump (traçabilité).
- 2026-11-10 — P3.6 revues absorbées : `wilderness_legs` durci en `==` exact (2830 ticks) ; amulettes câblées aux sorts (`string:unstrung_*` réels, `enchant:` résolu par `enchant_for`, chaîne `enchant`). Renvoyés (hors périmètre P3.6, fichiers gelés) : durcir les `>=` des tests gamedata en `==` (P2.1, stations/spawns), dettes P2.4 restantes ci-dessous, affichage dashboard N8 ci-dessous. Rejeté : toucher gamedata/scripts/web depuis P3.6 (périmètres exclusifs des vagues).
- 2026-11-10 — P3.6 dette P2.4 SOLDÉE côté planner, RESTE côté scripts/SDK : les méthodes inventaire (`fire/string/alch/superheat/enchant`) sont mappées (petit : fait). Restent, avec échec typé `Usage` au run (constaté, pas contourné) : op `spell-cast` SDK manquant (`alchemy`, `superheat`, `enchant_teleport` résolvent puis échouent loud), grille du tanneur non vérifiée (`leatherwork` tan → `Usage`, jamais pressée à l'aveugle). Dossier motivé : l'exécution de ces jambes attend l'op SDK + la grille, le planificateur les propose déjà correctement. Rejeté : fallback silencieux ou mapping contourné (règle 7).
- 2026-11-10 — P3.6 dette N8 dashboard AJOURNÉE (dossier motivé) : le relais CLI existe (`nav-stats` expose `detour(sum/max/plans)`, alerte > 1,2 en place côté service), mais l'affichage dashboard exige `web_*`/`orchestration/static` (hors périmètre : seuls `brain*.rs` ouverts côté orchestrateur). Proposition au front : colonne Détour dans lives ou panneau nav-stats (somme/max/plans + alerte), alimentée par l'endpoint existant. Rejeté : toucher au front depuis P3.6 (churn hors périmètre) et dupliquer la métrique côté brain (une seule source : le service nav).
- 2026-11-10 — P3.6 écarts connus : `equip:*` reste sans exécuteur (le nœud échoue désormais typé au lieu de tourner silencieux ; l'équipement via quêtes/scripts reste à câbler, vague ultérieure) ; `tan:*` échoue typé tant que la grille P2.4 n'est pas mappée ; les chaînes `lesser_alch`/`bury` portent des priors de drop/dégâts à remplacer par les rates mesurés ; `brain` ne choisit pas encore ses jambes via `CHAINS` (choix XP/argent/objet au niveau méthode : `rank_money_method` + jitter, `best_for_*` testés en pur) — sélection par chaînes entières en vague ultérieure. Rejeté : câbler la sélection par chaînes dans `resolve_ready` (jambes multi-maillons ≠ nœuds unitaires, conception due).
- 2026-11-11 — Convention exit-code TRANCHÉE + grilles cuir vérifiées : tout script supervisé sort `Ok(0)` (but), `Ok(2)` (budget) ou `Err` (session `failed` sinon) — `equip`/`alchemy`/`superheat`/`enchant_teleport` rendaient `Ok(compte)` et marquaient `failed` à chaque succès ; les comptes restent dans le log. Grille tanneur vérifiée (`tanner.rs2` : soft 96/95/93, hard 101/100/98) et grille `leather_crafting` vérifiée (`leather.rs2` : gants 116/115/114, bottes 119/118/117, vambraces 122/121/120, chaps 125/124/123, cowl 131/130/129, armure 113/112/111) : `leatherwork` presse le x1 vérifié quand les options restent sourdes, dossier admin (modale 2311 → `if_button` 8638 → gants +1, cuir 5→4). Spawns boot imprenables exclus du F2P (`UNREACHABLE_OBJ_SPAWNS` : hache 2970,3376,1 + pioche 3009,3342,0 ; pioche confirmée `Disconnected` hors ligne, hache à confirmer live). Garde pêche : `structurally_dry` échoue loud et nommé (hint `toll` si la clôture bloque) au lieu d'attendre 16 repicks dans un pit sans eau.
- 2026-11-12 — PRD3 D-A (utilisateur) : « productible en F2P » = recette, récolte de skill, mélange, sort ou script d'objet-sur-objet dont tous les intrants sont eux-mêmes obtenables en F2P (point fixe, `gamedata::obtain`) ; drops et spawns au sol ne comptent pas. Achat chez un marchand PNJ autorisé seulement pour un objet F2P non productible (`merchant_buy_allowed`), appliqué au planner (jambes `shop:buy`, porte d'outils) et au kit (`buy_stock` → `Blocked`). Inventaire figé : `prd/PRD_3_inventaire_boutiques_f2p.tsv` (217 objets en boutique F2P, 82 achetables). Rejeté : recettes seules (le minerai resterait achetable au marchand).
- 2026-11-12 — PRD3 D-B (utilisateur) : objet productible sans vendeur au carnet → offre d'achat permanente posée par le cerveau ET graphe de production lancé ; le premier abouti ferme le nœud et annule l'autre (`brain_market::cancel_open_buys`). Ordre de résolution (`planner/resolve_obtain.rs`) : ask couvert → production faisable → marchand si D-A → offre au repos → blocage typé `missing:level:<skill>:<n>` | `missing:<outil>` | `missing:coins:<n>` (nœud `have_coins`, gagné par la méthode d'argent la plus rentable).
- 2026-11-14 — Prix courant (décision utilisateur) : la VWAP 24 h ne décide plus rien ; prix = médiane des 5 derniers fills (tout âge) bornée par le carnet, sinon carnet, sinon 1 (règles d'ouverture). Anneau mémoire par objet + lecture indexée `LIMIT 5`, jamais de scan (CPU). Le % 24 h de la page reste un affichage.
- 2026-11-12 — PRD3 prix : un objet jamais échangé vaut 1 pièce (référence : VWAP 24 h → dernier print → milieu du carnet → 1) ; le prix magasin n'ancre plus rien. La bande ×5/×0,2 ne s'applique qu'à une référence issue de vraies transactions (sinon personne ne pourrait enchérir au-dessus de 5). Cotation ZIP proportionnelle (`market::step_toward`, 5 %/pas, ≥ 1 pièce, patience 200 ticks, `market_reprice`) bornée par plafond (acheteur) / plancher (vendeur). Validé hors ligne (`market::sim_prd3`) : objet rare et demandé 5 → ~75, objet abondant reste à 1, jamais au-dessus du plafond acheteur.
- 2026-11-12 — PRD3 jamais de vente aux PNJ : `sell_for_target`/`general_store` supprimés, repli D2 = alchimie si rentable sinon jet ; le cerveau liste lui-même le surplus échangeable des bots inactifs toutes les 600 s (`brain_sales`), hors pièces, nourriture ≤ 100, keep-list, intrants du skill entraîné, outils, porté, envies. Achats `market:buy` posés côté daemon (plus de script worker : l'ordre est de la comptabilité, la livraison reste la convocation).
- 2026-11-12 — PRD3 générateurs : `../lostcity/engine/tools/lifescape-dump.mts` modifié avec accord explicite (D-C) — prérequis d'équipement par table de labels `levelrequire` (distance/magie, ordre des arguments, verrous de quête), params `levelrequire`/`rangebonus`. `gamedata/tools/dump_plan.mts` + `drop_rates.mts` : taux de drop F2P évalués depuis les sources RuneScript (bones `death_drop` inclus), `shop_access.json` (boutiques ouvertes en F2P par handler, varps de quête), ventes par dialogue, dons scriptés, recettes de config. Rejeté : tables codées à la main par objet.
- 2026-11-12 — PRD3 limites connues : `file_quests` est un indice grossier (fichier entier) réservé au choix d'équipement (Silverlight) ; 16 objets F2P sans aucune source en rev 274 figés par nom (`obtain_tests::NO_F2P_SOURCE`) ; arcs saule/érable members en 274 (meilleur arc F2P : chêne) ; pioches non forgeables en 274 (achat marchand autorisé).
- 2026-11-12 — Rates : le tutoriel ne nourrit plus les rendements (règle utilisateur) — `rates_service::is_unrated_method`, filtré à l'ingestion et au rechargement du snapshot (purge des anciens compartiments au prochain rapport).
- 2026-11-12 — Marché PRD3 sans aucun échange (constat live 16:00 UTC : 125 ventes ouvertes, 13 achats, 0 exécution depuis le 11/11, `ctx coins=0` pour tous les bots alors que les sauvegardes montrent 25 à 427 pièces, toutes en banque). Cause racine : le serveur cesse de transmettre la banque à chaque fermeture (`inv_stoptransmit(bank_main:inv)`), le SDK la retirait de `world.inv`, le snapshot publiait une banque vide → cerveau à 0 pièce (ni `market:buy`, ni offre D-B, intrants bloqués : `missing logs` alors que 22 bûches étaient à 1 pièce) et registre du market à 0 (toute offre refusée). Correctif : banque connue conservée (`World::inv_closed`, relogin, amorçage sauvegarde), heartbeat et registre la lisent. Secondaires : intrants manquants obtenus par charge d'inventaire (`need_count`, avant : 1 unité, abandon au 4ᵉ blocage), blocages remis à zéro par la progression, `have_coins` passe d'abord par les drops de pièces (hommes, gobelins). Rejeté : réservation de pièces à la pose, vente aux PNJ (interdite PRD3), achat marchand des haches (D-A : la hache de bronze reste productible, la demande attend un forgeron).
- 2026-11-12 — Redémarrage daemon sans pénalité : un worker restauré lit `idle` avant que son script ne rapporte ; pendant 120 s après la première supervision d'un compte par ce daemon, un nœud actif sans rapport repasse `ready` sans échec compté (`brain::daemon_warming`, par compte : la restauration de 14 workers dure ~90 s). Constaté live : chaque redémarrage coûtait un échec par nœud actif (trois redémarrages = nœuds abandonnés). Compteurs gonflés corrigés à la main (pénalités loggées 15:51-15:55 retirées, dépendants rétablis).
- 2026-11-12 — Graphes bloqués à vie (bh99tw2f, bbkjz2vk : « graph stuck … operator decides » puis « no goal left »). Causes : (1) jambes `farm:ball_of_wool` / `farm:bronze_pickaxe` figées dans le fichier de vie par le planner pré-PRD3 (`farm:` dès qu'une source drop existe : diablotins, nains) que `map_method` refuse (aucun spot ne chasse ces PNJ) ⇒ `no-script` ; le planner actuel dérive filage / marché+marchand D-A, mais les vies persistées gardaient l'ancienne liste ; (2) trois « no report » causés par redéploiements opérateur (16:48 : `open` qui remplace le job, le rapport `stopped` poussé juste avant part avec l'ancien job ; file du cerveau pleine qui jetait le rapport drainé) ont abandonné `have_coins` n20, et la cascade a emporté n19/n18/n14… en gardant leurs anciens `Blocked: missing …` (cause masquée) ; relance manuelle de n20 seul ⇒ dépendants restés abandonnés ; (3) un but atteint à côté d'un vieux nœud abandonné était classé `stuck` (pêche 15 faite puis jamais retirée) ⇒ toutes les compétences « stuck » ⇒ plus aucun but ; (4) la ligne « stuck » + deux entrées d'historique se répétaient à chaque passe. Correctifs : `normalize` re-dérive les jambes mortes d'un `obtain` ouvert ; la cascade écrit `dependency nX abandoned` ; `open` excuse l'arrêt (comme `unload`/`close`, effacé au lancement suivant) ; rapport remis en attente si la file est pleine ; but `done` si les niveaux live l'atteignent ; période de blocage notée une fois (`brain_stuck.rs`). Politique (règle de cycle de vie, pas un repli) : après 6000 ticks de blocage sans but à tirer, `lifecycle::revive_stuck` relance chaque nœud abandonné (budgets neufs, jambes re-dérivées, cascade comprise), au plus 3 fois par nœud, puis `graph final` et l'opérateur décide. Rejeté : relance immédiate ou illimitée, relance silencieuse sans trace (`history: graph revived`), compter les arrêts opérateur comme des crashs.
- 2026-11-12 — Budget de plan typé `BudgetExhausted` (handovers marché « square unreachable ») : le daemon debug (4 threads pool) coupait Falador/Draynor → fontaine de Varrock au `PLAN_BUDGET` 3000 ms (même jambes : 3,7-6,2 s de boucle sous charge, 2 s à vide) et la coupe répondait `Disconnected closest=` près du départ, mise en cache L1. Désormais : coupe (visites ou mur) = `BudgetExhausted` jamais caché, C/D sautés après une coupe de A (seul le péage B reste), budgets A 20 s + diagnostic 8 s (< `SERVICE_WAIT_MAX` 30 s), pire jambe mesurée 11,5 s (Varrock est → ferme Falador, 183k visites). Rejeté : garder Disconnected sur coupe (faux verdict caché), budget visites seul (25 s+ de CPU par NoRoute exhaustif).

## 2026-11-12 — Nettoyage de l'économie empoisonnée (règle 13)

Constat : `cheat giveother` depuis `admin` avait injecté 10 000 pièces à t03
(11/11 00:20Z) et 1 000 à t99 (11/11 11:11Z) ; 100 pièces étaient passées à
t01 (deal 26), et `admin` avait vendu au marché des objets trichés (deals
22, 24, 27). Décision utilisateur : plan complet.
- Sauvegarde `backup/poison-2026-11-12/` (base, `.sav`, `run/lives`, users).
- Ordres 5932/5934 de t03 annulés ; comptes t03 et t99 supprimés (`.sav`,
  `run/lives`, `run/users`) ; lignes de base gardées comme trace d'audit.
- `.sav` hors ligne édités (inventaires ré-encodés, CRC recalculé, connexion
  vérifiée) : t01 −100 pièces, t02 −1 bûche de saule, t06 −5 plumes ; t18
  avait déjà consommé sa crevette.
- Historique `holdings_*` des pièces : −10 000 du 11/11 01:00 au 11:00,
  −11 000 du 11/11 12:00 au 12/11 20:00, −100 au 12/11 21:00 (courbe
  continue : 757 avant et après l'injection).
- Code : `economy_guard.rs` refuse `giveother` (call et setup `verify`) ;
  la liste d'exclusion des totaux devient `run/economy-excluded.txt`
  (admin et bite toujours exclus).
Second passage (validé par l'utilisateur) : tous les autres `giveother`
historiques retirés des `.sav` hors ligne (sauvegarde `sav2/`), plafonnés à
ce que le compte détient encore : filets (t01, t02, t04, t06–t08, t14–t16,
t18, t19, joe_cooper, olivia_king), outils, runes, appâts, seaux, grain…
de t01/t05, clé de b9htjtfz ; t40 : 278 crevettes cuites et 74 brûlées
(dérivées des 588 crevettes crues trichées) et 2 cisailles. Ordre 5926 de
t08 (filet triché en vente) annulé. Connexions de contrôle t01/t40 OK.
Reste inexpliquée : la bosse +375 pièces du 11/11 08:00.

## 2026-11-13 — Échanges fiables : protocole de remise, verrous, audit des carnets

Constat (nuit du 12 au 13) : 620 deals, 63 conclus. Causes mesurées dans les
journaux des deux parties : sac plein au retrait (refus serveur ignoré),
stock fantôme vendu sur plusieurs deals, paires à 6–7 deals traités chacun
dans son ordre (`mismatch` puis `timeout`), convocation tardive (point sûr)
avec une échéance déjà consommée, retraits en billets cassés (la banque était
cherchée avec l'id du billet).
Décisions (règles utilisateur) :
- Remise : tout déposer à la banque la plus proche (outils gardés), retirer
  les lots exacts à Varrock ouest (billets dès qu'ils existent), puis la
  fontaine. Un seul échange par partenaire et objet (`market_lots.rs`),
  deals pris dans l'ordre croissant des deux côtés.
- Un côté qui ne peut produire ses lots rapporte `short` : ses ordres sur
  l'objet sont annulés, le côté honnête garde sa priorité.
- Passé son échéance, un bot attend tant que le daemon garde le deal ouvert
  (plafond 3000 ticks) ; le balayage couvre cette attente.
- Verrous : ce que les ordres ouverts et deals promettent (objets du vendeur,
  pièces de l'acheteur) est poussé au worker toutes les 10 s ; tout retrait
  bancaire est plafonné hors de la procédure d'échange (`client/locks.rs`).
- Audit : toutes les ~60 s, chaque ordre est pesé contre sac + banque du
  compte (sauvegarde hors ligne) moins ses deals en cours ; les non couverts
  sont annulés (`market_audit.rs`). Premier passage : 222 ordres annulés.
- Un ordre partiellement rempli (`matched`) reste annulable.
- Intégrité des carnets (revue du 13) : le livre en mémoire fait foi. Annuler
  ou amender retire d'abord l'ordre du livre (verrou moteur seul), puis écrit
  le carnet (verrou store seul) ; un ordre absent du livre (rempli, en vol)
  n'est jamais « annulé », et une ligne finale (`settled`/`cancelled`) ne
  bouge plus. La fusion des fragments se fait dans livre + carnet sans repasser
  par les gardes de placement (bande de prix, couverture). Au redémarrage, les
  deals en vol échouent et leurs lots reviennent aux deux parties (comme un
  `nav_down`). Verrous et audit comptent un ordre sur billet sur l'objet de base.

## 2026-11-13 — Cœur du planner : montée depuis le niveau live, bannissements temporaires

Constat (audit lecture seule de la flotte) : 40 % des ticks de montée
mêlée (2,74 M sur 6,90 M) rejouaient des tranches déjà atteintes
(`expand_goal` partait toujours du niveau 1 ; b8jxrdd2 « attack 10 » à
attaque 45 : 18 015 ticks sur des poulets) ; 12 bots sur 25 avaient toutes
leurs compétences bannies à vie (`stuck:<compétences>` lu sur tout
l'historique) et restaient inactifs (`graph: final`) ; un graphe fini sans
but à tirer ré-écrivait `done:` à chaque passe.
Décisions (règles utilisateur) :
- Les buts se déplient depuis le niveau live (`expand_goal_from`,
  `append_goal`, `record_block_from`) ; un nœud de montée que le niveau
  live atteint déjà est clos sans lancement ; toute tranche part avec
  `--level` (récolte et combat).
- Un but bloqué bannit ses compétences 6000 ticks seulement, cause typée
  écrite et loggée (`stuck:<s>@6000:<classe> (node <id>, <erreur>)`),
  expiration loggée une fois (`unban:<s>`). Anciennes entrées sans tick =
  expirées : les bots inactifs retrouvent un but au premier passage du
  nouveau binaire. Le tirage est retenté à chaque passe, `final` compris.
- Discipline de cycle de vie : plus aucun statut écrit hors
  `lifecycle.rs` côté daemon (`complete_node`, `retry_uncounted`,
  `end_engagement`, `insert_ready`, `reached_live`). `brain.rs` découpé
  (`brain_launch.rs`). Dette connue : le planner écrit encore `Blocked`
  au résolveur (`expand`, `resolve_obtain`) et `replan_node` change des
  statuts (appelé via `lifecycle`).
Rejeté : bannissement à vie (bots inactifs sans recours), relance
immédiate sans délai (même échec en boucle), bannir sur un manque
d'outil/intrant/pièces (sous-tâches existantes), repartir du niveau 1 en
comptant sur le script pour s'arrêter (budget de 18 000 ticks brûlé).

## 2026-11-13 — Page Erreurs : journal centralisé persistant (table `error_events`)

Besoin : toutes les erreurs au même endroit, identifiables (compte, tâche,
nœud, coordonnées, zone), agrégées par type et par signature, avec KPIs.
Décision : migration 3 de `lifescape-store`, table `error_events` alimentée
par un seul thread du daemon (`errors_ingest.rs`) :
- poussées (canal borné, jamais bloquant, pertes comptées et loguées) :
  événements `script_error` (`ingest_push`), rapports de tâche `failed` /
  `blocked` (repliés sur la ligne de l'événement de la même panne : une
  panne = une ligne), lignes `brain …` du journal du daemon (abandon de
  nœud, graphe/but bloqué, ban de skill, plafond de relance) captées dans
  `logln!` même, sans relire le fichier ;
- tirées toutes les 30 s : dossiers `run/failures/*.json` (par mtime) et
  deals `failed` (table `deals`), clé unique `source_ref` ⇒ rescans et
  redémarrages sans doublon, rattrapage des 72 h au premier passage.
Signature = message sans nombres/coordonnées/listes `[…]`/nom du compte/
chat `; last game:`. Rétention 72 h (purge horaire) : les dossiers nav
seuls font 500–1400 lignes/h (~650 o). Mesuré sur les données réelles :
36 700 dossiers + 790 deals rattrapés en ~15 s, résumé 24 h en ~0,25 s.
Taux d'échec par tâche lus de `task_reports` (échecs / runs), taux d'échec
des deals de `deals`. Rejeté : agrégation à la requête sur les sources
brutes (52 000 dossiers, 445 Mo, journal sans date) ; anneau mémoire +
instantané JSON (filtres et fenêtres à recoder, perdu entre deux
instantanés) ; analyse du fichier `orchestrator.log` (heures sans date,
rotation).

## 2026-11-13 — Équipement par paliers, prix par l'offre et la demande

Constat (audit 2026-11-13) : envies d'équipement sur la meilleure pièce
absolue (plastron noir/mithril, amulette de puissance), plafonds 7–105
pièces ouverts à 1 et jamais servis (l'offre était annulée et reposée
toutes les 600 s : sa marche repartait de la référence), jamais
transformées en nœuds ; personne ne forge ; 564 asks presque tous au
plancher 1 (17k bûches de saule, 11k plumes) ; masse ≈ 2 340 pièces,
entrée ≈ 1 660/24 h par les seuls drops mêlée.
Décisions (règles utilisateur) :
- Équipement = sous-but réel, palier suivant seulement (bronze → fer →
  acier → mithril → adamant → rune, F2P, niveaux du bot : attaque pour
  l'arme, défense pour l'armure), arme d'abord puis corps, jambes,
  casque, bouclier (`planner::gear_ladder`, `brain_gear`). Achat au
  marché seulement (D-A, aucune exception pour l'armure), plafond 50 % du
  portefeuille (choisi : un palier à la fois porte tout le budget
  d'équipement, l'autre moitié reste aux intrants et outils ; il monte
  avec les pièces) ; vendeur présent → nœuds obtain + equip_best lancés
  en priorité, ask trop cher → `have_coins` (2× l'ask) ; pas de vendeur →
  offre D-B permanente qui attend un producteur sans bloquer le bot. Un
  palier sans vendeur à la résolution ou abandonné est retiré du graphe
  (`lifecycle::withdraw_gear_step`) ; les ids retirés restent réservés
  (l'historique compte dans `next_id`).
- Prix : 1 pièce = amorçage seulement. Pas de 5 % par fenêtre de 200
  ticks dirigé par les exécutions (vente servie ↑, vente au repos ↓,
  achat au repos ↑ jusqu'au plafond, achat servi ↓) ; ouverture des
  ventes à la référence (fin de la marge +10 %) ; aucun plancher lié au
  coût des intrants (réserve = moitié d'une référence échangée, 1 sinon).
  Les ordres partiellement servis continuent de marcher ; une annonce
  sans bornes est réadoptée par la passe de ventes (hors marchands) ;
  une offre permanente garde sa marche, seul son plafond suit le
  portefeuille (`keep_bid`).
- Producteurs : leurs produits (barres, forgé, artisanat, cuisine) sont
  vendus, sauf ceux qu'une autre méthode ouverte consomme.
- Crochet de production pour les ambitions : `planner::profit::chain_profit`
  (recettes gamedata, prix marché, rendement mesuré), erreurs typées.
- Un `obtain_items` déjà couvert par les avoirs (sac, banque, porté) est
  clos sans lancement (`already_held`).
Rejeté : plafond lié au coût des intrants (contraire à la règle), viser
la meilleure pièce d'un coup (jamais servie), palier en attente active
d'un vendeur (bloquait le worker jusqu'à 6000 ticks par essai), achat
magasin pour l'armure, plafond = portefeuille entier (plus rien pour les
intrants).

## 2026-11-13 — Ambitions par bot, couverture de flotte, demande du marché

Constat : la succession tirait toujours la compétence la plus basse ; tous
les bots finissaient sur les mêmes activités (poulets, crevettes, bûches),
le marché n'offrait qu'une poignée d'objets et aucune offre d'achat ne
déclenchait de production.
Décisions (règles utilisateur) :
- Chaque bot a une ambition long terme (catalogue de 12 groupes F2P, la
  mêlée comptant pour UN groupe, + marchand) qui tire ses buts par jalons
  (niveaux, quotas de production listés, équipement, pièces).
- Couverture : au moins un bot par groupe et un marchand (groupe
  primaire), affectés aux bots dont les niveaux s'y prêtent le mieux ;
  au-delà, tirage au hasard pondéré par la demande et √adéquation (la
  règle `levels:<groupe>` qui figeait un spécialiste a été retirée le jour
  même : 9 des 12 bots hors couverture devenaient guerriers). La
  couverture ne compte que les comptes ayant un job dans le daemon (hors
  `admin`/`bite`), jamais les vieilles vies de `run/lives`. Les comptes
  existants reçoivent leur ambition au premier passage, loggée.
- Changement facultatif au plus une fois toutes les 72 000 ticks ; un
  jalon bloqué 3 fois ⇒ nouveau tirage immédiat (politique approuvée,
  loggée), comme une échelle épuisée : ces deux-là ne sont pas des
  changements facultatifs et ne sont pas bridés.
- Bannissements (6000 ticks) respectés : les jalons d'une compétence
  bannie sont sautés, jamais forcés.
- Offres d'achat ⇒ production : une offre ouverte qu'une ambition sait
  servir devient un jalon `produce` (entraînement et achat d'intrants
  compris) ; le poids des tirages suit la demande (offres × âge) et les
  profits (chaînes D58 aux prix marché, profits mesurés).
- `GoalKind::Produce` : production seulement, jamais d'achat du produit
  (sinon le quota serait « rempli » au marché).
- Plus-bas-skill réservé à l'ambition explicite `generalist` ; sans
  ambition, aucun but (erreur typée), jamais de repli silencieux.
- `farm:blankrune` → `essence_run` (l'essence n'avait aucune jambe de
  production : un runier ne pouvait que l'acheter).
Rejeté : une ambition par stat de mêlée (12 groupes, pas 15) ; changement
libre à chaque jalon (bots instables, marché erratique) ; garder le
plus-bas-skill en repli (masque une ambition manquante) ; remplir un quota
par achat.

## 2026-11-13 — Aucun bot inactif : but d'appoint, attente de fill parquée (D-SIDE)

Contexte : 5 min après le déploiement des ambitions, 12 bots sur 25
inactifs. Deux causes : (A) tous les jalons ouverts tombaient sur des
compétences bannies (`every open milestone trains a banned skill
(cooking,woodcutting)`) et le bot attendait l'expiration du ban (6000
ticks) ; (B) un nœud `market:buy` gardait le verrou pendant l'attente du
fill (jusqu'à 6000 ticks), worker inactif (offres à 1 pièce, plafonds
6–13 sur amulet_of_power, iron_platebody/legs, amulet_of_magic, raw_pike).

Décision (application de la règle utilisateur « aucun bot inactif ») :
- (A) But d'appoint temporaire (`planner::side_goal`) quand tous les
  jalons ouverts sont bannis, sans toucher l'ambition ni ses jalons :
  1. la plus lourde demande du marché à marge > 0
     (`market_demand::demand_items_with`, `chain_profit`) que le bot
     produit avec une compétence NON bannie, niveau atteint, quantité pas
     déjà détenue → `GoalKind::Produce`, origine `side:demand:<objet>` ;
  2. sinon une méthode d'argent (`GoalKind::MakeMoney`, origine
     `side:money`) ; toute résolution argent/pièces est détournée des
     compétences bannies (`steer_off_bans`, mêlée = un groupe).
  Log `brain X: side goal <but> while <skills> banned`. L'origine ne
  nomme aucun jalon : la clôture ne touche rien, le tirage suivant revient
  à l'ambition dès qu'un jalon redevient tirable. Rien de possible (Iron
  aux compétences d'argent toutes bannies) : log typé comme avant, jamais
  de repli silencieux.
- (B) Un `market:buy` posé est PARQUÉ (`lifecycle::park_for_fill`) : prêt,
  méthode gardée, sans verrou, historique `awaiting_fill`. L'offre reste
  au carnet (la livraison se fait aux points sûrs du worker, boucle de
  convocation). Chaque passe règle les attentes (`brain_filler::
  settle_parked` : rempli, partiel, échéance — mêmes règles qu'avant) ;
  le worker lance le nœud prêt suivant hors attentes
  (`next_ready_except`), sinon un REMPLISSAGE : une fenêtre d'argent de
  1500 ticks (spots à pièces d'abord, hors compétences bannies), nœud
  `fill-<id>` hors graphe (rien compté sur le graphe). Son rapport est lu
  (`brain_filler::fold_filler_report`, 2026-11-14) : un remplissage en
  échec ou fini sans travailler (0 tick, erreur) écarte sa compétence des
  remplissages suivants pendant `FILLER_RETRY_TICKS` (500) — avant, « report
  for unknown node, ignored » relançait le même `melee men` à 0 tick toutes
  les 10 s pendant une heure. Une attente
  verrouillée par un ancien daemon est parquée au premier passage.
- Équipement mêlée : un but `EquipBest{melee}` (jalon `equip` des
  ambitions guerrier/prêtre) ne développe plus le set niveau 1 en
  obtentions marché (source des amulet_of_power / iron_platebody à 1
  pièce) : l'échelle (`brain_gear`) achète la mêlée hors graphe, le but ne
  fait que porter le meilleur possédé. Les vieux fichiers perdent leurs
  obtentions ouvertes de ce set (`gear_ladder::withdraw_melee_set`, offres
  annulées). Distance/magie gardent leur set (amulet_of_magic : jalon
  `equip magic` du mage).
Rejeté : laisser le bot attendre l'expiration du ban (contraire à la
règle) ; changer l'ambition pour un ban temporaire (l'ambition ne change
qu'aux règles 72 000 ticks / 3 blocages) ; annuler l'offre pour libérer
le worker (perte de priorité, demande jamais servie).

## 2026-11-13 — Les outils se vendent

Règle utilisateur : les outils sont des marchandises comme les autres.
L'ancienne porte D52 (toute hache, pioche, filet, canne, tinderbox… jamais
vendue, par aucun bot) et son extension du jour (marteau, ciseau, aiguille,
cisailles, couteau, appât + annulation de toutes les annonces d'outils)
sont retirées. Un bot garde UNE unité du meilleur outil qu'il détient dans
chaque famille dont son travail a besoin (méthodes des nœuds ouverts,
récolte et combat compris, et compétences des jalons ouverts de son
ambition), plus les piles entières des consommables de ces méthodes
(appât, plumes, flèches, runes) ; tout le reste se vend (copies en trop,
paliers inférieurs, outils d'autres métiers) — `brain_tools.rs`. Une
annonce n'est retirée que si elle verrouille la dernière copie libre d'un
outil de travail (cas b3gybcev : son seul marteau en vente).
Rejeté : interdire la vente de tout outil (prive le marché d'une classe
entière d'objets) ; garder toute la pile d'un outil de travail (24 seaux
pour une traite).

## 2026-11-13 — Château de Lumbridge : ouvert par sa porte

Décision utilisateur : le château a une vraie porte (doubles portes
1516/1519 en (3217,3218)/(3217,3219), mur droit angle OUEST) ; le portail
des remparts est toujours ouvert, cosmétique, pas une loc. Les overrides
`one_way_doors` 1516/1519 (`passable_from: west`, posés le 2026-11-11 sans
appui serveur) sont retirés : `doubledoors.rs2` n'a aucun contrôle de côté
et reachWall1 angle OUEST accepte le stand nord sans BLOCK_NORTH, soit la
tuile du battant voisin, marchable depuis l'est. La golden `chateau_scell`
devient `chateau_entree` (+ `chateau_sud_interieur`, `chateau_draynor_men`).
L'exemption de budget NoRoute (2026-11-06) reste, sa mesure de référence
`chateau_scell` est historique. Rejeté : garder le château scellé (NoRoute
exhaustif 3-9 s, ~50/h pour men/dark_wizards).

## 2026-11-13 — Carnet jamais croisé (bronze axe 1351)

Constat live : `Book::relist` réinsérait sans apparier ; après le deal
1068 échoué (lara_lane × bmzvbp5u à 20), le bid de lara_lane revenait à
20 au-dessus d'asks tiers à 16 et le carnet restait croisé (buys
[20,12,4], sells [15,16,16,18,19]) ; un vendeur entrant servait ensuite
le bid à 20. Second défaut : `place` s'arrêtait sur son propre ordre au
lieu de le sauter, laissant le reste croisé contre des tiers.
Règle (utilisateur) : tout ordre qui (ré)entre — placement, amend,
relist après échec, fusion, réinsertion au boot — est TAKER et s'exécute
au prix de l'ordre opposé déjà au repos (un acheteur paie le meilleur
ask), quel que soit le `seq` conservé pour le rang. Guérison d'un carnet
déjà croisé (`Book::uncross`) : le taker est la dernière (ré)entrée
(estampille d'arrivée du livre ; au boot, ordre de `updated_clock`, les
relists de deals morts au redémarrage datés du boot). Exceptions : un
compte ne s'apparie jamais avec lui-même ; les deux comptes d'un deal
échoué (toute raison) ne s'apparient plus ENTRE EUX pendant
`PAIR_COOLDOWN_TICKS` = 3000 ticks (`live_tick_ms`, restauré au boot
depuis `failed_deals_since`), libres avec tout autre. `uncross_books`
tourne au boot et à chaque tour de convocation. Chaque appariement passe
par le chemin normal (`merge_fills`, `record_fill`, `refresh_order_rows`) ;
moteur puis store, jamais imbriqués.
Rejeté : relist sans appariement (carnet croisé à vie) ; prix du `seq`
le plus ancien (lara aurait payé 20 avec une hache à 16) ; interdire tout
rappariement des comptes d'un deal échoué (bloquerait des tiers).

## 2026-11-13 — Volume d'achat / volume de vente (graphique /market)

Demande utilisateur : deux volumes par bougie, « quand un mec achète il
consomme des ordres de vente, et inversement pour les vendeurs ». Volume
d'ACHAT = quantité dont le taker (ordre qui entre ou ré-entre et consomme
le carnet, règle du carnet jamais croisé ci-dessus) est un acheteur ;
volume de VENTE = taker vendeur. `Fill::taker` porte ce côté ; il suit le
deal (`deals.taker_side`), puis l'exécution écrite au règlement
(`executions.taker_side`, seuls les deals réglés impriment), les bougies
(`candles.buy_volume`/`sell_volume`, reconstruites depuis les exécutions)
et les prints live (`s`). Migration 4 du store : colonnes ajoutées
seulement (une transaction), anciennes lignes = côté inconnu NULL, compté
dans le volume total `v` seulement. API `history` : bougies `vb`/`vs`,
trades `s` ("buy"|"sell"|null). Front : vert (achat) au-dessus d'une ligne
de base au milieu du panneau volume, rouge (vente) en dessous, même
échelle ; l'inconnu s'empile en gris au-dessus du vert. Rejeté : deviner
le côté des anciennes exécutions (prix vs bougie précédente) ;
recalculer `vb`/`vs` à chaque requête au lieu du cache de bougies.

## 2026-11-14 — Prix courant borné par les prix d'impact

Constat : le prix courant (médiane des 5 derniers fills bornée par le
meilleur bid / meilleur ask) se laissait fixer par une poussière : une
unité listée à 1 pièce ramenait à 1 le prix d'un objet échangé par lots
de 100, pour tous les bots (cliquet à 1 pièce). Pratique des vraies
places : le dernier prix seul n'est qu'un AFFICHAGE ; le « nominal
price » de HKEX = dernier prix borné dans [meilleur bid, meilleur ask] ;
le « mark price » des dérivés (BitMEX, Binance, OKX) lit des prix
d'IMPACT (le prix pour remplir une taille significative) et des
médianes, précisément pour qu'un ordre minuscule ne bouge pas la
référence. Le nôtre : médiane des 5 derniers fills bornée par les prix
d'impact, taille d'impact N = médiane des quantités de ces 5 fills
(1 ≤ N ≤ 1000, jamais échangé → 1) ; un côté de profondeur < N ne borne
pas ; jamais échangé, règles inchangées (milieu, sinon un côté, sinon
1). Bande ×5/×0,2 et drapeau `traded` inchangés. CPU : anneau mémoire
(prix, quantité) par objet, une passe sur les quelques meilleurs ordres
du carnet mémoire ; côté store une lecture indexée des meilleurs ordres
(`LIMIT 64`, arrêt dès N couvert). Rejeté : meilleur bid/ask (poussière
souveraine) ; VWAP de profondeur sur tout le carnet (scan, et une
profondeur lointaine n'est pas le prix du moment) ; ignorer le carnet
(un vrai ask profond plus bas EST le prix du moment) ; filtrer les
ordres sous un seuil de quantité fixe (un objet échangé à l'unité serait
aveugle à son carnet).

## 2026-11-14 — Rentabilité de tout le marché, demande dérivée, tableau des commandes

Contexte : demande utilisateur « évaluer la rentabilité en fonction de
toutes les dépendances et des rates, pour tout le marché » ; seuls les
bids `pot_flour` étaient servis, aucune barre ni minerai n'était demandé
au prix que justifient les armes.

Décision :
- Le moteur de réponse aux offres évalue chaque objet de valeur (bids,
  prix courant avec volume, plancher d'alchimie du lanceur), coût
  récursif avec tous les intrants, sites réels (site du brain au niveau
  de combat, marche entre étapes, péage, bateau, outils achetés une fois,
  remise à la fontaine) ; le forfait de 800 ticks par réponse est retiré.
- Revenu = ce que le marché absorbe (bids au-dessus du coût, 50 % du
  volume 24 h au prix courant derrière les asks déjà là, moins offre en
  route, stock propre et réclamations) ; moitié par producteur.
- Entraînement amorti : coût net des produits vendus pendant la montée,
  nul si l'ambition monte déjà la compétence, un lot sur 4 sinon.
- Demande dérivée : l'offre permanente d'un intrant (D-B) est plafonnée et
  ouverte par la réserve dérivée du produit (80 % de sa valeur moins
  temps et autres intrants).
- Réclamations publiées sur un tableau public (unités par objet, sans
  nom) : l'équivalent des commandes d'artisanat d'un vrai jeu. D22 (aucune
  mémoire partagée) tient car l'information est publique et anonyme ; le
  rejet D22 visait des « offres en route » lues dans l'état caché des
  autres bots.
- Poids d'ambition : un type sert toute demande que sa compétence
  primaire fabrique à n'importe quel palier, ses intrants à moitié par
  étape ; l'échelle du forgeron va jusqu'à l'adamant.

Rejeté : garder le forfait de trajet (pénalise les petits lots proches) ;
imputer toute la montée au lot (personne ne monte jamais) ; offres
d'intrants au prix courant seulement (une barre à 2 ne fait bouger
personne) ; lire les buts des autres bots (connaissance cachée, D22).

## 2026-11-14 — Cache de routes central, rapports de taux différés

Constat (VPS 13:00) : daemon ≈ 19 % d'un cœur, dont `plan-query` 6 %
(`walk_costs`) et `plan-pool` 5,8 % (Dijkstra des ratés) ; 17 Mio/min
écrits sur disque, dont ~15 par `run/rates-snapshot.json` (790 Ko
réécrits à chaque rapport). L'ancien L1 (1024 plans sous la clé complète,
préfixe/suffixe par balayage JSON de tout le LRU, danger ignoré) servait
~55 % des requêtes ; les options fragmentaient les clés (tags de pièces,
péage) alors que le plan trouvé n'en dépend pas.

Décision :
- Un cache de routes unique dans le service, lié à la version du
  snapshot (vidé au `reload-nav`), réutilisation PROUVÉE : dominance
  d'options (tags, péage, bans, danger : règles de sous-graphe), sous-
  chemins entre points de coupe re-tarifés pas à pas (exacts), épissures
  par marches locales servies seulement si le surcoût est prouvé ≤
  `max(4, 6 %)` par une borne inférieure (inégalité triangulaire sur le
  chemin optimal en cache). Négatifs sous la clé complète, TTL 60 s.
- Mêmes gains hors cache : mémo des poches scellées par version, lecture
  du monde sous verrou partagé, arrêt de Dijkstra au plancher de rang
  pour les destinations bloquées (réponse identique), horizon de
  `walk_costs` (64 + 4 × distance), réveil des attentes `plan` par
  condvar, ligne `cpu-profile nav` par minute.
- Snapshot des taux : marqué sale à chaque rapport, écrit au plus une
  fois par minute (`rates-flush`) et à l'arrêt propre. Perte possible au
  crash / SIGTERM : la dernière minute d'agrégat (pas de gestionnaire de
  signal sans `unsafe`) ; les lignes brutes du store restent.

Rejeté : clés normalisées à l'aveugle (servir un plan à des options qu'il
ne valide pas) ; épissure acceptée sur un simple « jambes courtes » (2 ×
jambes est une borne trop lâche : elle refusait presque tout, ou alors
il fallait accepter un surcoût non prouvé) ; A* (heuristique admissible
cassée par les rides/bateaux, risque sur les goldens) ; TTL sur les plans
positifs (le monde statique ne change qu'au reload) ; écrire le snapshot
à chaque rapport en asynchrone (même volume disque).

## 2026-11-14 — Chemin court prouvé, coût fixe des plans

Constat : après le cache de routes, nav ≈ 7-8,5 s CPU/min ; 52 % des plans
≤ 3 cases, 77 % ≤ 8, chacun payant le planificateur complet ; profil par
étape : le `DangerMap` (≈ 180 carrés live, HashSet de ~50 k cases, ~7 ms)
reconstruit par Dijkstra, par `analyze` et par `walk_costs` dominait tout.

Décision :
- `DangerMap` en bitmap dense par niveau (même réponse par case).
- Sauts courts (même niveau, Chebyshev ≤ 8) répondus sur `plan-query` par
  un Dijkstra de marche fenêtré aux règles du planificateur, servi
  SEULEMENT si l'égalité de coût est prouvée (bornes des arêtes sautées :
  portes, bord, rides) ; sinon le planificateur. Distance nulle évitée
  dans le SDK.
- Coût fixe restant : danger trié, comparaisons de listes sans tri,
  cache F2P par zone, `walk_costs` en hachage identité + borne de but
  admissible, poches sans relecture des portes, codec sorti de `service.rs`.
- Compteurs `fast_hits` / `fast_zero` / `fast_declined`.

Rejeté : heuristique « court donc ligne droite » (non prouvée, portes et
clôtures) ; mettre les sauts courts en cache (8 k entrées pour des plans de
1-3 cases, et la clé contient la liste de danger) ; trame binaire pour les
options (JSON ~60 µs, gain marginal, compatibilité des workers) ; abaisser
le plafond des poches ou l'horizon `walk_costs` (changerait les réponses).
Rejeu : 73,3 s → 10,0 s CPU (−86 %, danger live), 9,9 → 7,0 s (ancien
profil) ; prévision live ≈ 1,1 s/min.
## 2026-11-14 — Géants des collines sans clé, Wilderness à la ligne moteur

Constat live : aucun bot `melee ladder` n'allait aux géants (george_97,
combat 75, chassait les gardes ; log `gate BrassKey closed`). Quatre causes :
(1) le ladder exigeait la clé en laiton, jamais obtenue (spawn (3131,9862)
souterrain, donc aucune jambe `ground:`) ; (2) la nav répondait
`NeedsRequirement` dès que le chemin levé passait la porte à clé 1804,
sans chercher le détour sans clé (trappe d'Edgeville, portails 1558/1557
du donjon : 260 contre 171) ; (3) `danger::in_wilderness` lisait tout
z ≥ 3520 comme Wilderness, souterrains compris : chaque chasseur de
donjon était agressif à vie, aucun camp souterrain voisin n'était sûr ;
(4) aucune échelle de cave ne remontait (`dump-transitions` sautait
`ladder_from_cellar`, en-tête commenté) : un bot descendu ne pouvait plus
banquer. Décisions : la nav relance une recherche stricte bornée par le coût
(`2 × levé + 64`) avant `NeedsRequirement` ; Wilderness = test moteur
`Player.isInWilderness` (z 3520..6400 et 9920..12800, x 2944..3391) ; les
géants sont `Gate::Open`, la clé possédée ne sert que de raccourci
(`Rung::shortcut_key`, gardée à chaque voyage) ; un camp souterrain est
refusé si un chasseur de l'approche (boîte trappe→camp ± 50) qui
attaquerait ce niveau de combat brise la ligne de repas en deux coups
(`approach_threat`, même règle que `burst`). Rejeté : faire obtenir la
clé (inutile, le donjon y mène) ; exiger la passivité de tous les
chasseurs du trajet (hobgoblins armés : combat 85).
Restent hors ladder : géants moussus (salle derrière la toile à trancher
`bigweb_slashable` (3146,9898), aucune arête nav), donjon des glaces
(pas de banque F2P à `BANK_TREK_LIMIT` de Mudskipper Point).

## 2026-11-14 — Aucun bot inactif (2) : achat plutôt que combat, ban traversé, passes enchaînées, détecteur

Constat live (60 bots) : ryan_mason (mage épinglé, 0 pièce) inactif 30+ min,
un nœud toutes les ~1,5 min, `obtain airrune` bloqué `missing:combat:43`
(sites du sol/drop sûrs seulement à 43) puis `reach_combat 43` (style magie)
bloqué faute de runes, en boucle ; aucune méthode d'argent lancée. Ailleurs :
1,5–2 min sans décision après chaque nœud clos sans lancement (owen_wood,
lily_brook, jim_ellis : pièce d'équipement déjà détenue, enchère parquée) ;
remplissage refusé tant que toutes les méthodes d'argent sont bannies
(george_97 « no filler … (strength) »).

Décisions :
- Une obtention dont les jambes de terrain exigent un combat que le bot n'a
  pas, alors qu'un achat la vend (marché, boutique, vendeur de dialogue,
  jambe chiffrée), bloque sur les pièces de l'achat (`have_coins`), jamais
  sur le niveau de combat (`resolve_obtain::prefer_purchase_over_combat`).
  Sans jambe d'achat (Iron, non échangeable), le combat reste le blocage.
- Filet : un blocage `combat:N` dont un `reach_combat ≥ N` attend déjà le
  nœud bloqué est un cycle : abandon typé immédiat (`cycle: …`), jamais un
  jumeau neuf (`lifecycle::cycle_twin`). Objets et niveaux exclus (une
  montée bloquée sur les barres du palier suivant reste normale).
- Fenêtre d'argent obligatoire (remplissage, nœud pièces, but `side:money`) :
  hors bans si possible, sinon la mieux classée travaille À TRAVERS son ban
  (`side_goal::steer_for_work`, log `through the <skill> ban`), jamais à
  travers un remplissage qui vient d'échouer. Un ban ne laisse jamais un bot
  sans rien faire (règle utilisateur).
- Tick simple du cerveau sur l'horloge (`PLAIN_TICK_SECS` = 3 s) et non plus
  « une passe superviseur sur 6 » (balayage de ~8 s à 60 bots : un tick par
  50–90 s). Une passe qui a changé la vie sans rien lancer est suivie d'une
  autre aussitôt, jusqu'à `brain_chain::CHAINED_PASSES` (6).
- Détecteur (`brain_idle`) : un worker piloté par le cerveau (fichier de
  vie), connecté, ni en pause ni tenu par l'opérateur, sans script depuis
  5 min écrit `brain <compte>: idle <N>s without a script: <3 dernières
  décisions>` → ligne `idle_bot` de la page Erreurs ; répété toutes les
  15 min tant que ça dure.
Rejeté : entraîner 39 niveaux de combat pour une rune à 1 pièce ; attendre
la fin du ban ; un remplissage générique sans attente parquée (non demandé :
le détecteur rend visible tout autre cas).

## 2026-11-14 — Trousse permanente et équipement complet (utilisateur)

Décision utilisateur, pour TOUS les bots (non combattants compris ; Iron :
sans marché) :
- Un bot garde ses outils quand il change de but. La garde des ventes
  (`brain_tools::kept_tools`) ajoute la trousse (`planner::tool_kit`) au
  travail en cours : par famille la meilleure hache / pioche détenue (même
  au-dessus du niveau) et la meilleure qui travaille à son niveau (pioche :
  niveau de minage, `pickaxe_checker.rs2` ; hache : toute hache détenue,
  `woodcutting_axe_checker` sans niveau en 2004), 1 marteau, 1 tinderbox.
  Paliers inférieurs et copies en trop : surplus vendu. Même garde pour le
  concierge de banque (`space:cleanup --keep`, avec l'équipement gardé) et
  l'alchimie.
- Trousse permanente : hache, pioche, marteau, tinderbox. Achat
  (`tool_kit::kit_purchase`, passe `brain_tool_shop` de 60 s, banque
  diffusée, aucun pas d'équipement ouvert, avant le plan d'équipement) :
  d'abord la trousse de base (marteau, tinderbox, pioche, hache manquants),
  le meilleur palier ≤ 25 % (`TOOL_SHARE_PCT`) du portefeuille libre, sinon
  le palier le moins cher ≤ 50 % (`KIT_BASE_SHARE_PCT`) ; puis les
  améliorations : meilleur palier au-dessus du palier qui travaille, ≤ 25 %.
  Palier hache : niveau de bûcheronnage (règle utilisateur) ; le niveau
  d'attaque ne compte que pour manier (le serveur coupe avec la hache dans
  le sac). Prix : coût de l'objet (Bob, Nurmof ; magasins généraux 1 pièce).
- Exception D-A (haches de la trousse, `obtain::kit_shop_route`) : Bob's
  Brilliant Axes vend bronze, fer, acier. Le pas d'équipement d'une hache
  porte `market:buy` puis `shop:buy` : un ask couvert d'abord, sinon la
  boutique ; jamais une offre au repos que personne ne remplit. Les
  obtentions du graphe gardent D-A (leur hache attend un forgeron). Mithril
  et plus : marché seul (liste d'envies).
- Équipement complet : le plan d'équipement (`gear_plan`) garde un
  emplacement VIDE tant qu'une amélioration d'un emplacement déjà équipé peut
  céder (budget court) ; jamais vendu (`kept_gear`). Iron : plan restreint
  aux pièces de boutique (`plan_gear_from`, `merchant_route`), au prix
  boutique, déposé en pas d'équipement.
- Pas d'obtention d'équipement close par un autre membre de la famille :
  un pas qui veut `bronze_pickaxe` ne se ferme pas sur une pioche de rune
  au-dessus du niveau (`brain_launch`, `is_gear_obtain` → compte exact).
Rejeté : offre marché à 1-2 pièces sur une hache de base ; achat du palier
maximal sans budget ; garder tous les paliers (le surplus finance le reste).
Non fait : ramassage des spawns au sol pour un bot à 0 pièce (la trousse
attend qu'il gagne ses 2 à 32 premières pièces).
- 2026-11-15 — Admission de sessions retirée (E1 enveloppe, E10 pas d'ouverture 2 s) sur décision utilisateur : tout `open` passe immédiatement ; `admission.rs` → `disk_guard.rs` (avertissement D34 seul). Rejeté : garder le pas d'ouverture (création de comptes refusée à la suite).

## 2026-11-16 — Rendement = bilan de flux signé, seule mesure (plan rendements, R0-R1)

Décision utilisateur (plan `docs/plan-rendements.txt`, D1-D20 et §8) :
on ne mesure qu'une chose, le RENDEMENT d'une activité, bilan de flux
SIGNÉ par 6000 ticks de travail : ce qui entre (+) et ce qui sort (−),
mesurés de la même façon. Tout le reste en est dérivé, jamais mesuré à
part :
- XP/6000 = Σ rendement(unité porteuse d'XP) × XP de l'unité (dump / 10,
  `method_way::action_xp`) ; unités : P produit réussi, C intrant consommé
  (bûche brûlée, os enterré, objet alché ou enchanté, essence), S lancer
  de sort payé, K kills / dégâts par type de PNJ (combat.rs2
  `give_combat_experience` : dégâts × 10 dixièmes × 4 selon le style
  (contrôlé 1,33 × 3, longue portée 2 + 2, magie 2) × `combat_xp_multiplier`
  du PNJ, PV × 1,33). L'XP observée n'est qu'un contrôle (D4, alerte > 5 %).
- Profit/6000 = Σ flux signés × prix courant (médiane des 5 derniers fills
  bornée par le carnet, D12) + flux de pièces (péages, alchimie).
- Guides money making et entraînement GÉNÉRÉS (une ligne par méthode fine
  × site × palier d'outil × tranche de 5), lus par les bots et affichés au
  dashboard (D1, D2), jamais écrits à la main.

Résumé D1-D20 : D1 guides bots = dashboard ; D2 clé méthode fine + site +
outil + tranche ; D3 combat = kills par PNJ ; D4 XP observée = contrôle ;
D5 recette sur stock au prix de vente ; D6 classement profit/6000, trajets
et marché comptés ; D7 déclin par ticks de travail (demi-vie ~60 000) ;
D8 mesures v1 purgées (confirmation à l'exécution) ; D9 prérequis +
confiance, ni capacité ni risque ; D10 tirage pondéré ≤ 15 % du top ;
D11 guide public seul ; D12 prix courant ; D13 traits marché ↔ autarcique
et rapide ↔ économe ; D14 exploration ~5 % sous 3 mesures ; D15 100 %
market au prix courant ; D16 chaînes = lignes à part ; D17 pièces → temps
au profit de la meilleure ligne accessible ; D18 tous les sorts F2P
méthodes du planificateur ; D19 unités comptées par le registre par action
du SDK + compteurs d'événements, diff net = contrôle ; D20 quêtes = lignes
raccourci du guide d'entraînement.

§8 (consommation de tous les skills) : chaque sortie d'objet d'un run a une
cause ; `lost` et `unattributed` sont des alertes à zéro visé.

R1 (fait) :
- Contrat de rapport 2 (`report_contract.rs`) : méthode fine du
  planificateur transmise par la jambe (`--method`, drapeau de session
  retiré avant le script, `report_meta.rs` ; sans méthode transmise :
  `method_source = "none"`, rien d'inventé), site, cible, outil/arme,
  style de combat, niveaux de tous les skills au départ, `work_ticks` et
  `pause_ticks`, bilan `flows` [{name, cause, qty}], `units`
  {product, input, cast, kill, damage}, `alerts`. Les champs v1 (`items`
  aux provenances D45, `activity`, `ticks`) restent dérivés des causes :
  l'agrégation actuelle lit le même rapport jusqu'à R2.
- Causes (`flow_causes.rs`) : product, byproduct, input, ammo, rune, bait,
  supply, food, discarded, loot, ground_spawn, fee, death, shop_bought,
  shop_sold, traded, unattributed. Registre SDK (`item_ledger.rs`) : sac +
  équipement porté observés ensemble (porter ne sort rien, une flèche tirée
  sort du carquois) ; actions ciblées par objet et par signe (manger,
  enterrer, jeter, ramasser, lancer un sort) ; banque ouverte ou fermée
  depuis < 2 ticks = transfert ; pertes < 30 ticks après une mort = mort ;
  XP par stat dans la fenêtre. Compteurs `run_events.rs` : lancers payés
  (XP de magie dans les 12 ticks), dégâts et kills sur la cible (masques
  NPC_INFO 0x1/0x10 décodés : dégâts, type, PV, PV max ; le serveur
  n'envoie pas l'attaquant).
- Diff net (sac + porté + banque) = contrôle de cohérence seul : un écart
  avec la somme des flux d'un objet est une alerte `residual`, jamais un
  rendement.
- Store schéma 5 : colonnes de contexte de `task_reports`
  (`measure_version` 1 = ancien rapport, 2 = contrat 2), tables
  `task_levels`, `task_flows`, `task_units`, `task_alerts`. Purge des
  mesures 1 préparée, NON exécutée : `orchestrator purge-measures-v1`
  (simulation) puis `--confirm`, daemon arrêté (un daemon schéma 4 refuse
  un fichier schéma 5).
Rejeté : garder le diff net comme source des flux (il efface ce qui est
produit puis consommé dans le même run : os ramassés puis enterrés, coupe +
feu, fonte + forge, poterie façonnage + cuisson) ; mettre la méthode fine
dans `method` (changerait la clé de l'agrégation actuelle avant R2).

## 2026-11-15 — Achats aux marchands (remplace D-A pour l'achat)

Règle utilisateur : tout objet F2P qu'un marchand F2P stocke peut lui être
ACHETÉ, fabricable ou non (`gamedata::obtain::merchant_buy_allowed`,
planner.md § Achats aux marchands) ; `resolve_obtain` passe par la
boutique quand elle ne coûte pas plus que le marché et que le
portefeuille paie. Le prix du marchand plafonne le marché et retire des
pièces du monde. D-A (2026-11-12, achat réservé aux objets non
productibles) ne vaut plus pour l'ACHAT ; la vente aux PNJ reste
interdite jusqu'à la vague 2 du plan marché (Q3 ci-dessous). Rejeté :
garder D-A (offres que personne ne sert sur des objets en boutique, C12).

## 2026-11-16 — Modèle de marché : lignée, motif, besoins persistants (plan marché, vague 1 A)

Décisions utilisateur (`docs/plan-marche.txt` §9), elles priment sur les
recommandations du plan :
- Q0 : tick réel 60 ms (`world.json`, environnement accéléré, 600 ms un
  jour) ; tout horizon s'exprime en ticks, 2000 ticks = 2 min aujourd'hui.
- Q1 : un amend garde sa lignée par une colonne `replaces`, statut
  inchangé (la ligne remplacée reste `cancelled`, `cancel_reason =
  'amended'`).
- Q2 (vague 2, M3) : concession en temps vers la limite PROPRE de
  l'ordre, `p(t) = ouverture + (limite − ouverture) × (t/H)^(1/β)`,
  horizon H par motif, vitesse β réglée par `price_patience` × urgence ;
  retour ZIP sur les fills publics conservé. Le pas de 5 % reste en
  vague 1.
- Q3 (vague 2) : vente au magasin PNJ des invendables quand le marché
  paie moins que le marchand (exception à D-A, comme un joueur ; la
  monnaie créée est suivie par `orchestrator economy`).
- Q4 (vague 2) : ordre arrivé à sa limite sans contrepartie : le vendeur
  est retiré à H, l'acheteur reste au carnet tant que son besoin existe.
- Q5 (différé, après plan-rendements R2/R4) : stock-cible d'intrants sur
  la tranche en cours et la suivante, plafonné par `market_affinity` et
  le budget.
- Q6 (vague 3) : teneurs de marché DÉDIÉS (nouveaux comptes `create-user`,
  phase de jeu jusqu'au capital cible, puis cotation à profit selon le
  stock autour de la fontaine de Varrock, informations publiques
  seulement, règle 13) ; `market:flip` SUPPRIMÉ en vague 3.
- Q7 : achats boutique rares (Champions' Guild, Lundail) permis à tous les
  bots, routes vérifiées d'abord (vague 1 B, M9).
- Q8 : les bids sur objets introuvables sont laissés tels quels (une
  demande comme une autre, ni filtrés des signaux ni retirés).

Fait en vague 1 A (M0 + M1 + M2) :
- Store schéma 6 (`lineage.rs`) : colonnes `replaces`, `root_id`,
  `root_clock` (t0), `motive`, `cancel_reason` ; la migration reclasse
  l'historique : une ligne `cancelled` remplacée par la ligne suivante du
  même compte, objet et côté créée à ≤ 1 s de son annulation est un
  amend (live : 306 949 amends sur 353 553 annulations). Une transaction,
  échec = schéma 5 intact.
- Motifs (`market_lineage.rs`) : `need` (nœud bloquant, offre D-B),
  `gear`, `tool`, `stock` (alchimie), `surplus`, `loot` (réserve `Found`),
  `flip` ; vide = non dit (ordres de worker). Horizons §3.5 : need 6 000,
  gear/tool/surplus/flip 36 000, stock/loot 72 000 ticks. `QuoteBounds`
  porte lignée, t0, motif, horizon, ouverture ; l'adoption après
  redémarrage les relit en base (`QuoteBounds::for_row`).
- Âge de la demande (`market_demand`) = âge de la lignée (C1).
- Besoins persistants (C3) : `place_obtain_bid` reprend l'offre au repos
  du compte sur l'objet (lignée `need` d'abord, `keep_bid_from`) au lieu
  de l'annuler et de la reposer au prix de mémoire ; l'échéance de 6000
  ticks termine la JAMBE marché du nœud (échec compté, replanification),
  jamais l'offre ; l'offre se ferme quand le besoin est servi
  (`need_met`) ou disparu (`need_gone`, balayage
  `brain_market::release_orphan_needs` : aucun nœud ouvert ne veut plus
  l'objet).
- Plan d'équipement tenu (C2, `brain_gear_hold.rs`) : budget lissé
  (25 % de chaque lecture), un emplacement garde sa pièce tant que son
  plafond reste ≥ 70 % du plafond initial, replanification complète au
  plus une fois par 36 000 ticks ou sur gain de palier (palier de niveau
  de combat 1/5/10/20/30/40/50/60, pièce acquise ou perdue).
- Séquestre (C4) : les pièces engagées étaient déjà soustraites du
  portefeuille des obtentions (`ctx.coins` libre) et verrouillées en
  banque (`set_locks`) ; l'audit de couverture BAISSE désormais une offre
  découverte au prix que la bourse couvre (amend, la lignée continue) au
  lieu de l'annuler, et classe les lignées par t0.
- Crédibilité (C5, §3.6) : aucune offre d'outil, d'équipement ou D-B dont
  le plafond est sous le plancher public de l'objet (valeur de basse
  alchimie, ou prix courant échangé / 4, au moins 1) ; une offre de
  souhait devenue non crédible est retirée (`credibility`). Les achats
  bloquants (`market:buy`) n'y sont pas soumis (le planner choisit déjà
  la jambe sur le portefeuille).
- Annulations motivées (`cancel {reason}`) : `amended` (réservé),
  `gear_plan`, `audit`, `need_met`, `need_gone`, `credibility`.
- Mesure : `orchestrator economy` imprime la section marché = critères
  §6 (`Store::market_criteria`).
- Un carnet par objet, billet compris (constat M9 2026-11-16 : Gold bar
  ask 51 sur 2357, bid 59 sur son billet 2358, jamais croisés, ~40
  objets coupés en deux) : `op_place` range un ordre posé sur un billet
  dans le carnet de l'objet de base (`market_audit::book_item`) ; au
  démarrage, les ordres au repos sur un billet passent au carnet de base
  (`rebase_note_orders`, idempotent : la correspondance billet → objet
  vient de gamedata, que le store ne lit pas, d'où ce pas au boot plutôt
  que dans la migration SQL). Le deal nomme l'objet de base ; la remise
  porte des billets (`market_client::trade_id`) et le vendeur retire la
  forme que sa banque tient : avant, un deal sur l'id du billet faisait
  retirer le billet lui-même de la banque, qui ne tient que des objets
  de base.
Rejeté : statut `amended` (Q1 b retenu, l'existant garde son statut) ;
annuler les offres non crédibles des besoins bloquants (vague 2, avec la
courbe) ; garder l'échéance sur l'offre (remise à zéro de la marche une
fois sur deux, §1.2).

## 2026-11-16 — Concession, ordres au marché, vendeurs, vente aux PNJ (plan marché, vague 2)

M3 (Q2) : le pas de 5 % par fenêtre est remplacé par une courbe de
concession par lignée vers la limite propre de l'ordre (plafond acheteur,
plancher vendeur) sur l'horizon du motif (`lifescape_market::concession`,
`market_reprice`) : `p(t) = ouverture + (limite − ouverture) ×
(t/H)^(1/β)`, β = 2^(1 − 2 × `price_patience`) × urgence (`need` ×2,
`stock`/`loot` ×0,5), bornée [0,25 ; 4] ; `price_patience` lu dans la vie
du bot (le 0,5 du store n'est plus écrit). Retour ZIP : fill propre →
recul de 5 % et courbe relancée ; prix courant public (hors fill propre)
du côté concédé → tiré de moitié. Amend seulement ≥ 1 % (≥ 1 pièce) ou
pour poser la limite ; fenêtre H/12 dans [500 ; 2000] ticks, phase par
hash compte/objet. M4 : un ordre pressé (`need`, banque pleine `urgent`)
prend le meilleur prix opposé dans sa limite ; tout ordre prend celui que
sa courbe atteint à la fenêtre suivante. Q4 : vendeur à son plancher
après H sans fill retiré (`horizon`), stock gardé un horizon ; acheteur
gardé à sa limite. M5 : `found_open` rejoint le meilleur ask (fin de la
cascade à 1), sous-cotation relative (1 %) et frappe du bid réservées au
vendeur pressé ; plancher = valeur de garde `max(coût propre, prix
boutique (Q3), net d'alchimie, 1)` ; filtre de ramassage par valeur
(`brain_loot`, `--skip-loot` : pile < 20 ticks du taux du bot).

Simulateur `sim_lifecycle` (20 acheteurs / 20 vendeurs, limites 60-140 /
50-130, 2 unités, patiences 0,05-0,95, arrivées sur 18 000 ticks, H 36 000,
5 graines ; même carnet, mêmes arrivées) :

| critère (5 graines)            | concession        | ancien pas 5 %    |
|--------------------------------|-------------------|-------------------|
| fills / paires compatibles     | 1,08 – 1,33       | 1,00 – 1,30       |
| prix final vs équilibre        | ≤ 7,0 %           | ≤ 17,0 % (g. 33)  |
| inversions du prix courant     | 4,5 % (≤ 15,4 %)  | 0 – 15,4 %        |
| prix d'ask/fill le plus bas    | 81 – 96 (> 1)     | 74 – 97           |
| attente moyenne d'un acheteur  | 2 269 – 17 578 t. | 2 468 – 10 700 t. |
| vendeurs retirés à H (Q4)      | 3 – 8             | 0                 |
| butin 50 vendeurs / 3 acheteurs: plus bas ask | 5 (= garde min.) | 1 (cascade) |

(fills > paires : l'enchère continue fait aussi des échanges hors
équilibre ; la concession attend davantage : les patients concèdent tard,
c'est voulu.)

Q3 (décision utilisateur, EXCEPTION ASSUMÉE À D-A) : un objet que le
marché paie MOINS qu'un marchand F2P (meilleur bid et prix courant
échangé ; rien pour un objet jamais échangé sans bid,
`outlets::npc_sale_beats_market`) est vendu à la boutique, comme un joueur
qui vide son butin au magasin général. Valeur serveur `calc_shop_value`
(`shop.rs2`, `shop_sale.rs` : coût × multiplicateur d'achat / 1000 au stock
de base, puis − delta par unité tenue ; le dump n'a pas `shop_delta`, le
script minore avec 30, le plus fort du contenu). Jamais une unité vendue
≤ ce que paie le marché (MIN = marché + 1, vérifié par unité). Le voyage
part avec le nœud janitor (`SpaceCleanup`) quand le gain dépasse 1500
ticks au taux du bot ; la boutique qui paie le plus, gardien le plus
proche. La monnaie créée est suivie : provenance `shop_sold` du registre
de pièces, ligne « npc shop sales » de `orchestrator economy`. Le
plancher de toute vente ne descend jamais sous ce prix boutique (option
extérieure). Rejeté : alchimie seule (magie 21/55, runes de nature non
productibles F2P) ; puits sans pièces seul (aucune valorisation).

Petits ajouts : plan d'équipement et Iron achètent chez Scavvo/Valaine
dès 32 points de quête (`gear_plan::shop_route_open`) ; migrations du
store dans UNE transaction `BEGIN IMMEDIATE` avec relecture de
`user_version` sous verrou et `busy_timeout` 30 s (course live
`migration 6: duplicate column name: replaces`) ; `resolve_obtain`
bloque sur `missing:coins` (`have_coins`) quand le plafond d'un
`market:buy` est sous le plancher public de crédibilité (§3.6).

## 2026-11-16 — Teneurs de marché dédiés, suppression de la revente marchande (plan marché, vague 3, Q6)

Décision utilisateur Q6 (`docs/plan-marche.txt` §9) : des comptes DÉDIÉS
cotent tout l'univers F2P ; la revente des marchands tirés (~10 %, méthode
de revente, op de cotation, archétype `merchant`, ambition et rôle de
couverture `merchant`, `rich:merchant`) est SUPPRIMÉE : plus aucune
ressource ne lui est allouée (aucune référence restante, test
`no_merchant_flip_reference_remains`). Migration au chargement des vies
(`planner::market_maker::migrate_merchant`, testée) : archétype →
producteur, but `get_rich` producteur, jambe de revente retirée des
nœuds (le nœud se résout à nouveau), ambition `merchant` retirée (le
cerveau en assigne une de flotte).

- Univers = `gamedata::f2p::market_universe` : 1128 lignes F2P
  échangeables − 552 billets − pièces = 575 objets (un carnet chacun).
- Capacité = (banque 60 + sac 28 − 20 cases gardées) × 80 % = 54 ;
  N = ⌈575/54⌉ = 11 comptes ; au plus 49 types de tranche détenus à la
  fois (strictement sous la ligne du concierge : 55 − pièces − trousse 4 − 1).
- Tranches : hachage rendez-vous plafonné (⌈575/11⌉ = 53), objets par id
  croissant : couverture totale, sans recouvrement, déterministe, stable
  à l'ajout d'un objet.
- Capital cible = point fixe Σ min(lot_i, 10 % × T) (lot = 1000 pièces à
  la valeur boutique, ≤ 500 unités), arrondi au millier : 60 000 à
  290 000 selon la tranche.
- Cycle de vie : `create-user --role market_maker` → tutoriel → phase de
  jeu (ambition opérateur `market_maker`, échelle de pièces, méthodes
  productrices) → bascule définitive au capital cible (achats et annonces
  de tranche annulés `role`, nœuds abandonnés `role: market maker`, but
  `MakeMarket`, poste `market:make` sur l'anneau de remise de la fontaine
  de Varrock).
- Cotation (`lifescape_market::maker`) : v = prix courant, sinon milieu
  des autres, sinon (découverte) min(valeur boutique, meilleur ask) /
  (1 + h) ; r = clamp((stock − lot)/lot, −1, 1) ; p = v(1 − h r) ;
  bid = ⌊p(1 − h)⌋ ≤ v − 1, ask = ⌈p(1 + h)⌉ ≥ v + 1 et > prix boutique
  de reprise ; h = 4 / 8 / 10 % (liquide / mince / découverte, quart de
  lot sans relèvement) ; sans demande (aucun achat d'un autre ni bid
  depuis 72 000 ticks) : bid −5 % par passe vers 1, stock au-delà d'un
  lot vendu au PNJ (Q3). Plafonds : 10 % des fonds propres par objet
  (stock à max(valeur, coût) + bid), 500 unités, 25 % d'un côté. Ordres à
  prix fixe (motif `market_make`), jamais sur la courbe de concession ;
  bids de teneur exclus des signaux de demande.

Simulateur (`sim_maker`, 40 objets façon constat §1.1 : 60 % carnets de
butin, 20 % deux côtés clairsemés, 20 % acheteurs seuls ; même graine,
même carnet, avec / sans teneur ; capital 100 000 ; débouché PNJ Q3 de
la vague 2 pour TOUS les vendeurs dans les deux runs,
`sim_lifecycle::run_with_outlet`) :

| graine | deux côtés vague 2 | vague 3 | écart médian vague 2 | vague 3 |
|--------|--------------------|---------|----------------------|---------|
| 11     | 6,0 %              | 81,4 %  | ×1,475               | ×1,369  |
| 22     | 4,7 %              | 66,5 %  | ×1,472               | ×1,301  |
| 33     | 5,6 %              | 79,5 %  | ×1,443               | ×1,170  |
| 44     | 4,7 %              | 81,7 %  | ×1,373               | ×1,136  |
| 55     | 2,1 %              | 85,8 %  | ×1,333               | ×1,160  |

Critère (inchangé) atteint sur les 5 graines. Lecture honnête : sans Q3
dans le simulateur, la graine 22 échouait (×1,301 contre ×1,274). Avec
Q3, les chiffres de la vague 3 ne bougent pas : le prix échangé que le
teneur a imprimé reste au-dessus du prix boutique, la boutique ne prend
rien. C'est la référence vague 2 qui change : ses carnets jamais
échangés et sans bid partent au magasin, la part à deux côtés tombe et
son écart médian s'élargit. L'hypothèse « les asks quittent les carnets
sans demande du teneur » est donc infirmée ; le critère passe parce que
la vague 2 du simulateur porte enfin sa règle Q3. P&L du teneur sur ces
40 objets : −65 à −3 837 pièces (pertes sur le butin sans demande, bornées par les
plafonds). Objet en chute 100 → 10 : perte 5 932 à 6 090 pièces sur
100 000 (≤ 10 %), stock max 137 (≤ 500).

Rejeté : liste d'objets écrite à la main ; tranches contiguës par id
(un objet ajouté décale tout) ; capital cible fixe sans lien avec la
tranche ; ordres de teneur sur la courbe de concession (le teneur
recote lui-même selon son stock) ; garder la revente marchande en
parallèle (décision Q6).
