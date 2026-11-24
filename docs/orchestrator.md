# Orchestrateur

Un seul daemon supervise tous les bots, un worker de session par compte
(`rs/crates/orchestrator/src/daemon.rs`, `supervise.rs`, `session*.rs`).
Cycle de vie : `open` (connecte, `idle`), `run` (charge, `running`),
`unload` (décharge, session gardée), `close` (logout propre). Un `run` qui
lève rend `idle` + `last_error`, jamais de mort de session. Événement non
résolu (`@@EVENT`) → unload + rescue en session (`session_rescue.rs`) puis
reprise du script interrompu.

Le daemon héberge aussi le service de navigation : un snapshot
(monde + tables + version) chargé au démarrage (`plan_service.rs`),
servi sur `run/pathfinding.sock`. Les workers ne portent que le handle
(`plan_client`) et la version épinglée — aucune carte en worker.
Les routes GET du tableau de bord sont une table (`orch-web/src/web_routes.rs` : pages statiques, routes d'API, préfixes ; `web.rs` ne garde que la boucle HTTP).
Les opérations du service sont typées (`nav::service_ops::NavOp`) et celles du
marché aussi (`scripts::market_ops::MarketOp`) ; le démon lui-même (`market_eta`)
planifie par `plan_service::service()` en direct, sans passer par la socket.

CLI (`src/cli.rs`, `src/main.rs`, le daemon démarre seul) :
`open|run|call|shell|status|show|see|logs|unload|pause|resume|hold|close|redeploy|shutdown|daemon|direct|map|route|replay-nav|dump-transitions|reload-nav|nav-stats`.

`ambition <compte> alcher` (2026-11-18, `src/brain_operator.rs`) : donne au
compte l'ambition opérateur alchimiste (`planner/src/alcher.rs`, tenue pour
de bon, jamais tirée par la flotte). Appliquée au prochain passage du
cerveau sur ce compte au repos : nœuds ouverts abandonnés (`role:alcher`).
L'alchimiste ne répond à aucune enchère : son but d'alchimie d'abord (temps
non décompté, `brain_alch::time_rate`), achats de natures et de lots sous le
net d'alch par profit décroissant (`market_pass`), sinon de l'argent qui
finance ces achats.

`ambition <compte> natural` (2026-11-21, `src/brain_operator.rs`) : le compte
opérateur (épingle de compétence ou type alchimiste) rejoint l'équilibre
naturel. Même mécanique : appliquée au prochain passage du cerveau sur ce
compte au repos. Retire l'épingle (`pin = null`) ou le type opérateur, abandonne
les nœuds ouverts de l'ancien rôle (`role: operator ambition`), lève les bans
causés par le changement de rôle (cas de l'alchimiste), puis donne l'ambition
par la valeur A4 : même calcul que `assign_by_value` / la passe de
rééquilibrage (`brain_value::draw` -> `pick_by_value`), valeur par 6000 ticks
partagée par les tenants actuels de chaque métier ; révision `operator:natural`,
chiffres de la valeur dans le journal du daemon. Ensuite le bot est ordinaire
(A2 : aucun changement avant atteinte). Refusée (erreur typée) pour Zezima,
N0valyfe et Yogosun (insensible à la casse), qui gardent l'ambition overall
pour de bon, et pour un teneur de marché.

`verify` (W2) : runner de scénarios déclaratifs (`scenarios/*.json`, lus avec
`lifescape-json`). Setup série via `admin` (cheats + `bank_all`, effets
vérifiés), runs parallèles sur comptes distincts (N clampé à 20 D38),
verdicts `show` vs baseline (position, inventaire pack+banque,
delta, XP, message par tick, PNJ). Dossier <40 lignes en échec uniquement.
Modules : `verify_cmd.rs` (runner), `verify_scenario.rs` (format),
`verify_assert.rs` (asserts).
`direct` rejoue un script sans daemon : il branche le handle du service
de navigation comme une session et refuse le run au démarrage si le
service ne répond pas (jamais de divergence silencieuse). `route`
planifie offline (`--paid`, `--ascii`), `replay-nav` classe un dossier
d'échec (A/B/C), `dump-transitions` régénère
`gamedata/data/transitions.json` depuis le serveur.
`reload-nav` échange le snapshot du service sans redémarrer ;
`nav-stats` imprime la jauge (chemin court `fast=réponses/dont
distance nulle/rendus au planificateur`, cache L1, queue, refus cap/full/stale,
RAM warn/shed, RSS daemon, carrés peints) et le JSON complet expose en
plus un compteur par op batch (`door_defs`, `tiles_flags`,
`static_locs_near`, `in_f2p`, `toll_fence`, `wants`, `diff`, `debug`,
`transitions_list`, `probe_targets`, `doors_near`).
Profil CPU toujours actif (`src/cpu_profile.rs`) : toutes les 60 s le
log du daemon porte une ligne `cpu-profile pool` (requêtes des pools
daemon/marché par `cmd:<cmd>`, `push:<type>`, `market:<op>`) et une
ligne `cpu-profile brain` (étapes `tick.*` du thread `brain-tick` et
phases imbriquées), top 12 par CPU, format `nom=count/cpu_ms/wall_ms`
(CPU du thread lu dans `/proc/thread-self/schedstat`). Une ligne
`cpu-profile nav` (thread `nav-profile`, `src/nav_profile.rs`) donne en
plus, par minute, le CPU des threads du service de navigation
(`plan-pool`, `plan-query`, `plan-wait`, `plan-accept`) et les compteurs
du cache de routes de la même minute (`exact`, `dominated`, `prefix`,
`suffix`, `infix`, `splice`, `negative`, `miss`, `joins`, taux de hit,
`walk_costs`, occupation, évictions, verdicts de poches) ; `plans` compte
aussi les sauts courts du chemin court (`fast`, `fast_zero`,
`fast_declined`, hors cache : `hit` reste le taux du cache sur les autres).
Registre : `src/registry.rs`, source unique `lifescape_scripts::names()`.

État et logs (`src/paths.rs`, racine détectée via le dossier `orchestration/`) :
`run/users/<compte>.json` (état fusionné), `run/users/<compte>.bot.json`
(heartbeat SDK), `run/logs/<compte>.log`, `run/orchestrator.sock`,
`run/pathfinding.sock`. Dashboard `http://localhost:8765/` (`src/web.rs`,
statiques dans `orchestration/static/`, port via `LIFESCAPE_WEB_PORT`).
Pages : `/` (bots), `/items`, `/market`, `/rates`, `/life`, `/economy`,
`/hiscores`, `/errors`, `/guides-money`, `/guides-training`.
Colonne tâche de `/` (`web_job_lives::summarize`) : la méthode du nœud
actif, `done`, `stuck`, sinon `ready` ; depuis le 2026-11-19, quand le
graphe attend (achat posé en attente de vendeur) et qu'une tâche de
remplissage tourne hors graphe, `filler <méthode>` (bulle : le nœud en
attente) au lieu de `ready` (`running_filler`, dernier lancement
`filler:` de l'historique sans fin enregistrée).
Page `/rates` (2026-11-17, `rates.js`, `src/rates_view.rs` pur,
`src/web_rates_view.rs` HTTP) : la vue mesures publiait ~18 000 lignes de
flux brutes (`/api/rates/table`, 11 Mo, ~1 s, gardé pour Top activities).
Elle sert désormais une ligne par activité mesurée (méthode × style, ses
clés pliées) ou par clé exacte (`group=key` : site × outil × tranche),
filtrée, triée et paginée CÔTÉ SERVEUR :
`GET /api/rates/activities?group=activity|key&q=<texte>&skill=&family=
&band=<1er niveau>&measured=1&min_runs=<n>&sort=<colonne>&dir=asc|desc
&offset=&limit=` (défaut : activités, `runs` décroissant, 50 lignes, au
plus 500) → `rows` (méthode, style, famille, tranches, clés et clés
mesurées, sites, runs, âge, confiance, 3 produits et 3 consommations
principaux /6000, XP dérivée par skill, valeur nette /6000, taux d'échec),
`matched` / `total` (« x lignes sur y »), `facets` (familles, skills,
tranches présents), `took_ms`, `index_build_ms`, `index_age_ms`. Filtres
au niveau de la CLÉ avant regroupement : texte (méthode, style, site,
outil, famille, objets, skills ; mots tous présents), skill (XP dérivée
> 0), famille (préfixe de méthode : fight, gather, smith…), tranche,
« mesurées seulement » (n ≥ 3, seuil de l'estimateur : en dessous le
cerveau lit le prior, ligne « estimated (thin) »), runs ≥ n. Une ligne
d'activité = Σ (taux × ticks de travail) / Σ ticks de travail de ses
clés gardées (Σ / Σ, jamais une moyenne de taux), runs et rejets sommés,
échec Σ échoués / Σ terminés, âge de sa clé la plus fraîche : un chiffre
de VUE, le cerveau et les guides lisent toujours la clé exacte.
Détail au clic : `GET /api/rates/detail?method=&style=[&site=&tool=
&band=]` (+ filtres de la page) → tous les flux valorisés (ramassages
< 1/6000 pliés en « other drops »), unités, XP, et les clés en
sous-lignes (cliquables, même détail). `GET /api/rates/trading` : table
trading et bots engagés seuls (sections repliées). Index (clés mesurées,
prix courant de leurs objets) construit depuis le snapshot live sous
verrou, gardé `INDEX_TTL` (30 s) quand des rapports arrivent
(`rates_service::generation`), sinon jusqu'à `PRICE_TTL` (300 s) ; une
requête ne fait que filtrer, regrouper, trier, paginer. Mesure hors ligne
(`orchestrator rates-rebuild <copie> --view [requête ...]`, copie du
2026-11-17, 3 433 clés) : index 53 ms ; page par défaut (mesurées) 88
activités, 50 affichées, 9 ms, 47 Kio ; `group=key` 3 433 clés, 74 ms ;
détail 0,1-3 ms.
Tableau Skills du spectateur (2026-11-16) : lignes heartbeat `skills`
`{id, level, base, xp}` (`level` = niveau actuel boosté/drainé, `base` =
niveau de l'xp, `session_heartbeat.rs`) ; ligne « Total » en bas (option
`footer` de `TableSort`, jamais triée) = somme des `base` et des xp.
Périodes des graphiques `/items` et `/market` (2026-11-15) : 24 h et
7 jours en pas d'1 h (`res=1h`, 24/168 créneaux), 30 jours et 6 mois en
pas d'1 jour (`res=1d`, 30/183 créneaux) ; tables `PERIODS` (`items.js`)
et `VIEWS` (`market.js`), l'historique plus court s'affiche tel quel.
Total `/items` (2026-11-19, décision utilisateur : « retire les coins
du total items ») : la ligne « Total », sa variation 24 h et la courbe
`total` comptent tous les objets sauf les pièces (`holdings::items_total`,
`Store::hourly_points_without` / `daily_points_without`) ; la ligne
« Coins » reste dans la liste. Billets et objets ne font qu'une ligne
(`gamedata::base_obj_id`).

Traçabilité `/items` (2026-11-15) : `GET /api/items/flows?item=<obj id |
nom affiché>&hours=24` (`src/web_items.rs`, `Store::item_flows_since`,
staff et `run/economy-excluded.txt` exclus ; sans `item`, les 60 plus
gros flux tous objets ; même source que `orchestrator economy [h] item`)
alimente sous le graphique de l'objet choisi le tableau « Provenance /
destination » (provenance en clair, quantité signée + entrée / − sortie,
rapports, ligne nette) sur la période des boutons (24/168/720/4380 h) ;
sans données, le tableau reste vide (aucun message).
Indices de supply `/items` (2026-11-18) : 11 familles (`cooking`, `food`,
`melee_equipment`, `range_equipment`, `range_supply`, `magic_equipment`,
`magic_supply`, `logs`, `tools`, `mining_supply`, `smithing_supply`)
dérivées des dumps par `lifescape_gamedata::supply_indices` (F2P,
échangeables, billets repliés sur leur objet, familles disjointes ;
compositions figées par `supply_indices_tests`). `GET /api/items` porte
`indices:[{key,label,count,change24h,ids}]` (`src/holdings_indices.rs`,
mêmes comptes que la liste) ; `GET /api/items/history?res=&index=<key>`
sert la courbe de l'indice (somme par créneau de `holdings_hourly` /
`holdings_daily`, `Store::hourly_points_sum`). Clic sur un indice : sa
courbe et la liste filtrée sur ses objets ; « Total » efface le filtre.
Trous de la courbe (2026-11-24) : les heures sans relevé (démon arrêté)
sont franchies par un segment droit entre les points qui les bordent
(`bridgeGaps` de `chart.js`, `lineRuns`) ; un trou qui coupe le bord
gauche part de la valeur interpolée au bord (`leadEdge`, avec le dernier
point d'avant la fenêtre). Jamais prolongée avant le premier point ni
après le dernier ; l'info-bulle ne montre que des relevés réels. Les
autres courbes (marché, taux, erreurs) gardent leurs trous.
Onglet « Cerveau » de la page Lives (`/life`, `brain.js`, 2026-11-13) :
`GET /api/brain/<compte>` (`src/web_brain.rs`, lecture seule) agrège le
fichier de vie (ambition passée telle quelle si présente, but + jalon
d'origine `goal_origin`, nœuds avec deps, sous-tâches déposées sur
blocage `filed`, méthode et alternatives chiffrées, compteurs), les
souhaits (commande daemon `brain_wishes` = `brain_wishlist::wishlist`),
ordres ouverts (plafond de reprix en mémoire) et deals, le live (niveaux,
combat, pièces, porté), l'historique des buts et les bannissements,
et `retired` (nœuds finis retirés du graphe, `lifecycle_retire` :
totaux faits/abandonnés, quêtes prouvées, 40 derniers enregistrements ;
panneau « Nœuds terminés »). Un fichier pas encore réécrit depuis la
retraite est affiché retraité. La page Lives compte `done` = nœuds faits
du graphe + `memory.done_total`.
Vue progression (2026-11-21, `src/web_brain_view.rs`, plan progression A1 à
A4, lecture seule ; l'ambition brute reste passée telle quelle dans
`ambition`) : `ambition_view` = comment l'ambition a été donnée
(`given.source` : `protected` overall des comptes protégés, `pin`, `operator`,
`a4_rebalance` / `a4_kept` avec la ligne du marqueur
`run/ambition_rebalance_a4c.json` : valeur par 6000 ticks, tenants, objet qui la
porte, montée ; `a4_draw` tirage par la valeur à l'atteinte ou
`operator:natural`, chiffres relus dans la dernière ligne « given by value »
du journal du daemon, jamais inventés ; `legacy` tirage d'avant A4), les
révisions expliquées (`revision_rows`, les règles retirées marquées :
`blocked:`, `change:`, `repair:`, `review:`), chaque jalon avec son vrai état
(`state` : `open`, `active`, `waiting` = refroidi après un palier
d'équipement que personne ne fabrique (A3) ou une offre sans vendeur,
`set_aside` = bloqué 3 fois, `done`) et le refroidissement restant
(`ticks_left`, entrée `cool:milestone:<type>/<id>` de l'historique) ; `goal.origin`
(jalon, jalon de demande `demand:`, réponse à une offre `bid:`, but d'appoint
`side:`, alchimie, remplissage `filler`) ; `cooling` = sites et méthodes
refroidis (`method@x,z,niv`) ; `training` = valeur du temps du but de montée
(A1 : gain mesuré du bot par tick, attrait des montées lucratives en attente
`investment::pull_per_tick`, plus grande des deux = ce que le choix de ligne
d'entraînement utilise) ; `tick_ms` (ticks en minutes). Ces sections ouvrent
le panneau de l'onglet Cerveau et le Détail de la page ; les compteurs à
rebours et `training` restent hors de `version`.
Source en échec ⇒ ligne dans `errors`, jamais une liste vide muette.
`version` = mtime du fichier de vie + empreinte du contenu affiché (hors
comptes à rebours) : le front relit toutes les 5 s et ne redessine le
graphe SVG (colonnes ambition → jalons → but → tâches par plus long
chemin de dépendance) que si elle change. Tests :
`web_brain_tests.rs`, `web_brain_view_tests.rs`, `node orchestration/static/brain.test.js`.
Serveur de jeu via `LIFESCAPE_SERVER_HOST` / `LIFESCAPE_SERVER_PORT`
(défaut `127.0.0.1:43594`), données engine via `LIFESCAPE_ENGINE_DIR`
(défaut `../lostcity/engine`). Par `127.0.0.1`, chaque paquet passe par
`docker-proxy` (relais en espace utilisateur, 8 à 17 % d'un cœur pour 74
bots à 60 ms/tick). Le chemin direct (`LIFESCAPE_SERVER_HOST=172.19.0.2`,
IP du conteneur) supprime ce coût mais a fait monter les désynchros de
protocole (`io: protocol desync`, 0/h par le relais contre ~83/h en direct,
2026-11-24) : rester sur `127.0.0.1` tant que la lecture des trames du
client n'est pas corrigée.

Ne jamais lancer un bot en direct hors orchestrateur (ni rescue manuel) :
sessions orphelines et états faux. Diagnostics aussi via l'orchestrateur
(`check_transitions`, `check_doors`).

Redémarrage (2026-11-12) : `shutdown` ferme les sessions puis termine
vraiment le daemon (la boucle `accept` lit `running`). Un daemon tué
(SIGTERM) laisse les workers connectés ; le suivant les reprend
(`restore_jobs`, statut retenté 3 s puis 10 s) sans jamais supprimer le
socket d'un worker vivant (seul un socket sans écoute est effacé : plus
de worker orphelin). La pause `hold` est portée par le worker et relue à
la reprise. `run/orchestrator.log` (sortie du daemon) est horodaté ; les
erreurs de script y figurent avec leur type et leur message.

Rotation des journaux (2026-11-13, « rotation zippée ») : `log_rotate.rs`.
Un journal de bot (`run/logs/<compte>.log`) qui atteint 100 Mo devient
`<compte>.log.<secondes unix>` puis est compressé en arrière-plan par le
`gzip` du système (`.gz`) ; les 5 archives les plus récentes de chaque
journal sont gardées. `run/orchestrator.log` (stdout du daemon, ouvert en
ajout) est vérifié chaque minute : copie puis troncature sous le verrou
stdout (aucune ligne `logln!` perdue), même compression et même rétention.
Les anciens `*.log.1` sont compressés une fois au démarrage du daemon.
Lire une archive : `zcat run/logs/<compte>.log.<ts>.gz | less`.

Déploiement (2026-11-13) : `restore_jobs` tourne en arrière-plan, après le
hub d'envoi, les chemins, le marché et l'écoute : `status` répond en ~1 s,
la reprise des ~25 workers finit en ~30–60 s (ligne `restore_jobs done in
Ns` du journal). Seul le daemon a changé → ne pas redéployer les workers.
Code worker changé (sdk, scripts, envois) → après `restore_jobs done`,
`redeploy <compte>` par compte de la flotte (liste lue dans `status` une
fois la reprise finie, jamais `run/lives` qui garde d'anciens comptes),
un compte à la fois, jamais en parallèle. `redeploy` = `close` puis `open`
sous un seul verrou de compte (aucun `open` market-absent/brain ne s'y
glisse) ; il attend lui-même le serveur qui garde encore le joueur (`already logged in`,
relogin retenté 2 s jusqu'à 120 ticks, `TIMEOUT_NO_RESPONSE` = 100).
Code ≠ 0 → lire l'erreur, ne pas boucler.

Notes de déploiement du plan d'architecture (à jouer à la mise en production
de la branche `plan-architecture`, une fois le démon arrêté proprement) :
- `run/lifescape-staging.jsonl` (661 Mo, jamais lu) n'est plus écrit depuis le
  lot 1.4 (module `store_queue` supprimé, aucun lecteur dans le dépôt :
  recherche exhaustive des noms et de `LIFESCAPE_STAGING_DIR`). **À supprimer à la
  main** après l'arrêt du démon : `rm run/lifescape-staging.jsonl`. Les rapports
  de tâche restent en base (`rates_ingest`, tables `task_reports`…).

Propriété des workers (2026-11-13, `worker_owner.rs`, `daemon_close.rs`) :
un seul worker vivant par compte, jamais deux. Bug corrigé : un `open`
opérateur et un `open` market-absent simultanés (ryan_park, 20:17) ont
lancé deux workers ; le second a effacé le socket du premier et écrasé
son entrée `procs` (le `Child` lâché sans être tué) : worker orphelin
connecté, `close` ne tuait que le suivi, tout `open` suivant refusé
`login refused: 5 (already logged in)`. Désormais :
- open/close/redeploy/relance watchdog d'un compte sont exclusifs
  (`LifecycleLocks`) ; le second reçoit `already open <compte>: another
  open/close is in flight` (open) ou `busy <compte>: ...` (close) ;
- la vérité sur les workers vient de `/proc` (`<exe> session <compte>`,
  cwd = racine du dépôt), suivis ou non : `open` répond `already open`
  si le worker répond au ping (adoption `restore_jobs` en cours), sinon
  refuse `live worker pid N for <compte> is not owned by this daemon
  (close <compte> reaps it)` ; `spawn` refuse aussi, jamais d'écrasement
  de `procs` ni de suppression du socket d'un worker vivant ;
- côté worker, `claim_socket` refuse de délier un socket servi ;
- `close` : demande de logout acquittée tout de suite par le worker
  (arrêt du script au prochain pump, déchargement borné, bouton logout,
  fin du processus), attente bornée 20 s, puis SIGTERM / 8 s / SIGKILL du
  worker suivi **et** de tout worker non suivi du compte ; réponse
  `graceful` (déconnexion propre ou terminé), `workers`, `untracked` ;
  `ok` seulement quand plus aucun worker du compte ne vit (`close` sur
  un compte sans job mais avec un orphelin le tue aussi) ;
- `shutdown` ferme les sessions en parallèle. Les envois des
workers sont asynchrones (`push_conn`) : un daemon absent ne gèle plus
leur pump. Build `dev` optimisé (`opt-level`), compilation sur 6 cœurs
(`.cargo/config.toml`).

XP (2026-11-22) : page `/xp` (`orchestration/static/xp.html`, `xp.js`,
`xp.test.js` : `node xp.test.js`), lien « XP » dans la barre de chaque
page. Instantané recalculé toutes les 30 min comme les hiscores
(`src/xp_service.rs`, 240 s après le démarrage, persisté `run/xp.json`, servi
depuis le cache par `/api/xp`). Modèle pur `src/xp_rates.rs` ; sommes SQL par
heure/compte/méthode/compétence dans `store/src/xp_rates.rs`. Taux = somme des
XP / somme des ticks de travail (jamais une moyenne de taux) : « contribution
flotte » (XP de la compétence / ticks de toute la flotte) et « efficacité »
(XPs des rapports qui rapportent de l'XP dans la compétence).
Tous les taux sont par 6000 ticks (Σxp/Σticks × 6000, « XP / 6000 ticks »).
Trois graphiques (24 h, 7 jours, tout l'historique, sélecteur propre à chacun
sauf XP flotte et XP / 6000 ticks flotte qui partagent le leur) : XP de la
flotte, XP / 6000 ticks de la flotte, courbes par compétence. Tout l'historique :
pas d'1 h tant que l'historique fait au plus 7 jours, puis 6 h jusqu'à 45 jours,
sinon 1 jour (calculé dans le rafraîchissement de 30 min, `history_chart`).
Tableau 24 h, 7 jours, tout l'historique
(évolution vs période précédente de même durée, sauf tout l'historique).
Comptes exclus : `economy_guard::excluded_accounts()` (admin, admin2, bite).

Hiscores (2026-11-14) : page `/hiscores` (`orchestration/static/hiscores.html`,
`hiscores.js`, lien « Hiscores » dans la barre de chaque page), style des
hiscores de 2004 en CSS original (cadre de pierre, titres dorés serif,
parchemin ; seules images du jeu : les icônes d'objets et de compétences, voir « Icônes ») : colonne de catégories (Overall + 19
compétences rev 274 dans l'ordre des stats 0..17 puis 20 ; Wealth : Coins,
Total value), tableau Rang / Nom / Niveau / XP par 25 avec pagination,
recherche par nom (compte ou nom affiché, préfixe accepté) et par rang,
vue personnelle (`#player=<compte>` : rang/niveau/XP de chaque compétence,
rangs pièces et valeur, 10 objets les plus précieux). État dans le hash
(`#cat=attack&page=2`, `#cat=coins&rank=30`). Calcul : `src/hiscores.rs`
(pur : niveaux, rangs, valorisation) et `src/hiscores_service.rs` (collecte,
cache, planification). Règles :
- joueurs = comptes suivis par le daemon + vies `run/lives` écrites depuis
  moins de 24 h, hors `admin`, `bite`, `run/economy-excluded.txt` et
  `proof-*` ; les Ironmen sont classés (étiquette « iron ») ;
- source : snapshot poussé du job s'il a moins de 10 min (seuls `skills`,
  `inv`, `bank`, `worn` sont copiés), sinon fichier de sauvegarde engine
  (XP serveur ÷ 10, étiquette « offline ») ; une banque live vide (pas
  ouverte depuis le login) prend celle de la sauvegarde ;
- niveau = niveau de base tiré de l'XP (`gamedata::xp`), jamais le niveau
  boosté ; overall = niveau total puis XP totale ; compétence = XP ;
  égalité départagée par nom de compte (rangs uniques) ;
- pièces = sac + banque ; valeur = sac + banque + porté au prix courant
  du marché (`MarketService::reference`, le prix des bots) pour les objets
  du catalogue qui ont un trade ou un ordre au carnet ; pièces = 1, non
  échangeable / hors catalogue / sans trade ni ordre = 0 ; marché absent ⇒
  valeur non classée (`value` null) et ligne dans `errors`.
Fréquence et CPU : thread `hiscores`, un snapshot au plus toutes les 30 min,
le premier 240 s après le démarrage du daemon (après le pic de reprise),
ou 30 min après le snapshot persisté s'il est plus récent. Le snapshot est
gardé en mémoire déjà sérialisé et écrit atomiquement dans
`run/hiscores.json` (rechargé au démarrage : servi tout de suite).
`GET /api/hiscores` renvoie ces octets tels quels (aucun calcul par
requête ; `ok:false` « first snapshot pending » avant le premier). Coût :
ligne `hiscores: snapshot N players, cpu X ms, wall Y ms` dans
`run/orchestrator.log`, entrée `hiscores.snapshot` dans la ligne
`cpu-profile pool` de la minute, et `cost.cpu_ms`/`wall_ms` dans le JSON
(mesure hors ligne, pire cas 58 comptes tous lus sur sauvegarde : ~32 ms
CPU par snapshot, JSON ~65 Ko).
Forme : `{ok, generated_at, next_at, interval_seconds, skills[19], market,
rule, errors[], cost, players[{account, name, iron, source, overall:[rang,
niveau, xp], skills:[[rang, niveau, xp]×19], coins:[rang, pièces],
value:[rang, valeur]|null, top:[{id, name, count, price, value}]}]}`,
joueurs dans l'ordre overall. Le front relit l'API toutes les 5 min.
Tests : `hiscores_tests.rs` (dont la route web servie depuis un snapshot
fixture), `node orchestration/static/hiscores.test.js`.
Icônes (2026-11-24) : l'icône de chaque objet et de chaque compétence à
côté de son nom (Spectate : voir « Onglets de jeu » ci-dessous ; Item holdings :
liste et titre ; Market : liste, titre, dernières transactions ; Hiscores :
catégories, titre du classement, scores personnels, « Most valuable
holdings »). Deux planches de sprites et un index dans
`orchestration/static/icons/` : `items.png` (cases 32×32, le rendu même du
sac en jeu : `ObjType.getSprite` de Client-TS lancé hors navigateur sur le
cache rev 274, configs `engine/data/pack/client/config`, textures, modèles
de `main_file_cache` index 1 ; 3 875 ids dessinés, 3 003 cases distinctes,
une case par image identique), `skills.png` (cases 25×25, sprites
`staticons`/`staticons2` de l'archive `media`, 19 compétences ; slayer et
farming n'existent pas en 274 : case vide), `index.json`
(`{items|skills: {file, hash, cell, cols, slot, count}}`, case 0 = vide).
Générées par `bun gamedata/tools/icons.mts` (Client-TS et lostcity à côté
du dépôt, ~3 s, octets reproductibles : le tirage aléatoire de luminosité
du client est fixé à 0.6, le réglage le plus lumineux) ; non committées (~1 Mo, `.gitignore`), à
régénérer après un changement de cache. Servies par le démon : `/icons.js`
(ligne de page) et `/icons/<nom>.png|index.json` (`web_routes::icon_asset`,
planches en cache long `immutable` car demandées en `?v=<hash>`, index
jamais en cache, 404 si absentes). Front `icons.js` (`LifeIcons`) : la
page écrit tout de suite une case vide à la taille finale
(`LifeIcons.item(id, count)`, `.skill(nom)`), les règles CSS
`background-position` arrivent avec l'index (une par id) : aucune
requête par icône, aucun saut de mise en page, un id sans icône garde la
case vide (jamais l'icône d'un autre objet). Une pile change d'icône avec
son nombre comme en jeu (`countco` : pièces en tas, flèches en botte ;
seuils 2, 3, 4, 5, 25, 100, 250, 1000, 10000, vérifiés par le
générateur). Icônes de compétence seules, sans fond (choix utilisateur 2026-11-24 :
agilité et vol, silhouettes noires, se voient peu sur fond noir).
19 objets nommés sans icône (modèles invisibles : `invis_*`, grinder,
caveorb4dot, viking_dummy_lid, wearable_stool_white) : case vide. Sans
`icons.js` (démon pas encore redéployé) ou sans index, les pages
s'affichent sans icône. Tests : `node orchestration/static/icons.test.js`,
`web_routes` (`icon_asset`, digest, fichiers servis).
Onglets de jeu de Spectate (2026-11-24) : les blocs Inventory et Skills
reproduisent les onglets du client rev 274 au pixel près, taille fixe
(1 pixel de canvas = 1 pixel CSS, jamais lissé) : sac (`inventory`, 4×7
cases, pas de 42×36 depuis 16,8), équipement (`wornitems`, 14 cases avec
leur décalage propre et la silhouette `wornicons` des cases vides, cadres
`miscgraphics`), stats (`stats`, grille de 3 colonnes, niveau
courant/niveau de base en p11 jaune, « Combat Lvl », « Total Lvl », « QP »).
Le générateur (`icons.mts`) lit ces trois interfaces dans le cache
(`client/interface`, ids de `content/pack/interface.pack`) et les dessine
avec un portage de `Client.drawInterface` et les primitives du client
(`Pix2D`, `Pix32`, `PixFont`) sur le fond `invback` : ce qui ne change
jamais est cuit dans une image par onglet, le reste (cases d'objets,
textes `%1` décodés de leur script, calques de survol) part en mise en
page dans `index.json` `tabs`, avec les glyphes des polices p11/p12/b12
(`client/title`) ; tout dans la planche `tabs.png` (~44 Ko).
`index.json` gagne aussi `items.stack` (ids empilables : le client écrit
leur nombre même à 1) et `items.wear` (id → wearpos, de
`gamedata/data/equipment.json`). Le bas de l'onglet équipement (bonus
d'attaque/défense, écrits par le serveur à l'ouverture) n'est pas
reproduit : l'onglet s'arrête à y = 200. Front (`spectator.js`) : fonctions
pures `SpectatorPanels.tabDraw` & co (liste de tracés : panneau, icône de
`items.png` par case, nombre de la pile `invNumber` en jaune à +9 avec
ombre, K dès 100 000, M dès 10 millions ; textes `getIfVar` : niveau,
base, XP, XP du niveau suivant `levelExperience[base-1]` — à 99 le client
lit l'entrée 98 = 14 391 160 —, combat, total = somme des bases hors
slayer, points de quête), puis `GameTabs` trace la liste sur le canvas
seulement quand elle change (pas de scintillement au rafraîchissement de
1 s). Survol : nom de l'objet (couleur `@lre@` des menus, nombre complet
pour une pile) dans une petite boîte aux couleurs du menu du client près
du curseur ; sur une stat, le calque du client « <Skill> XP: / Next Level
At: » remplace le cadre du bas. Au toucher, un appui montre la même chose
jusqu'à l'appui suivant. Emplacements : chaque entrée `inv`/`worn` du
battement porte `slot` (sac 0..27, équipement = wearpos) et le snapshot
`quest_points` ; d'un ouvrier plus ancien, l'équipement se place par
`items.wear` (identique aux sauvegardes serveur sur 8 bots vérifiés) et
le sac se tasse dans l'ordre (trous inconnus), sans « QP ». Tests :
`node orchestration/static/spectator.test.js`, `icons.test.js`.
Guides (2026-11-17) : thread `guides` (`src/guides_store.rs`), premier
jeu 20 s après le démarrage puis un toutes les 300 s (`REFRESH_PERIOD` :
prix = médiane des 5 derniers fills, rendement = 3 runs d'une clé, un
bot change d'activité à un changement de but) ; un jeu = vues catalogue
et par tranche des guides money et training (docs/economy.md « Guides »),
construit dans un seul moteur, publié en `Arc` (`guides_store::current`).
`GET /api/guides/money|training?skill&level&family&quests&held` lit la
dernière version (aucun calcul de guide par requête, ~60-80 ms de
lecture + JSON pour une vue complète) ; `ok:false, pending:true` avant le
premier jeu (la page redemande toutes les 5 s). Réponse : `rows`,
`unrated`, `market_wait_ticks`, `view_level`, `approximate[]`,
`built_at_ms`, `age_ms`, `cycle_ms`, `cycle_cpu_ms`, `refresh_s`. Coût :
ligne `guides: built N money + M training views in X ms (Y ms CPU)` dans
`run/orchestrator.log` à chaque cycle ; temps d'attente du marché et des
quêtes comptés de façon incrémentale (seuls les nouveaux rapports lus,
recomptage complet toutes les 12 cycles). Mesure hors ligne :
`orchestrator rates-rebuild <copie> --guides-store <dossier>
[money|training:requête …]`.
Erreurs (2026-11-13) : page `/errors` (`orchestration/static/errors.html`,
`errors.js`), API `web_errors.rs` : `/api/errors/summary?hours=1..72&source=
&type=&account=&script=&zone=&signature=` (KPIs, tendance vs fenêtre
précédente, erreurs/heure, groupes par signature, taux d'échec par tâche et
des deals, options de filtres), `/api/errors/list?…&limit=` (lignes : heure
UTC, compte, tâche/nœud/méthode, type, message, signature, x/z/niveau,
zone, détail, dossier), `/api/errors/dossier/<fichier>`. Collecteur
`errors_ingest.rs` (table `error_events`, sources task/nav/deal/brain,
voir `docs/decisions.md` ; scan `run/failures` toutes les 30 s
incrémental : seuls les noms jamais vus sont stat'és, passe complète
horaire pour les réécritures en place, aucun dossier supprimé), lignes construites dans `errors_model.rs`,
maths dans `errors_aggregate.rs`. Le spectateur suit `/#<compte>`.
Boucles muettes (2026-11-14, `report_progress.rs`) : chaque rapport porte
`bank_opens` (compteur SDK `Bot::bank_opens`, delta du run) ; un rapport
`no_output` (fenêtre ≥ 1000 ticks au budget, 0 XP, 0 objet) ou
`bank_churn` (≥ 40 banques, moins d'unités que de banques, 0 XP) écrit une
ligne `error_events` de type `no_progress` (source `task`, signature
`no progress <motif>: <activité>`, détail ticks/banques/objets/XP) et le
cerveau compte l'échec du nœud. Les échecs `failed: no progress …` de la
garde du kit et les lignes `brain <c>: nX no progress: …` (fenêtres sans
progrès vers le nœud) sont rangés sous le même type.
Bots inactifs (2026-11-14, `brain_idle.rs`) : un worker piloté par le
cerveau, connecté, ni en pause ni tenu, sans script depuis 5 min écrit
`brain <c>: idle <N>s without a script: <3 dernières décisions du
cerveau>` (répété toutes les 15 min) → ligne `error_events` de type
`idle_bot` (source `brain`, signature = dernière décision normalisée).
Le cerveau enchaîne jusqu'à 6 passes quand une passe change la vie sans
rien lancer (`brain_chain.rs`) ; le tick simple d'un compte part toutes
les 3 s d'horloge (`brain::PLAIN_TICK_SECS`).

Admission retirée (décision utilisateur 2026-11-15, « retire
l'admission ») : plus d'enveloppe E1 ni de pas d'ouverture E10, tout
`open` passe tout de suite ; seul l'avertissement disque D34 reste
(`disk_guard.rs`, loggé, jamais un refus).

## Teneurs de marché : déploiement (plan marché Q6, vague 3)

`orchestrator create-user [pseudo] --role market_maker [--goal kind]
[--no-tutorial]` (`src/create_user_cmd.rs`) : procédure documentée de
création, pseudo naturel généré quand omis (`names.rs`), vie écrite
(rôle, tranche = première libre, capital cible commun), session
ouverte, tutoriel par la porte du cerveau, puis la phase de jeu. Refus
typés : `--iron` (un Iron n'échange jamais), rôle inconnu, toutes les
tranches déjà tenues. Univers 575 objets, capacité 54 par compte :
N = 11 comptes, à créer un par un (chaque création prend la tranche
libre suivante, 0 à 10). Les tranches sont équilibrées par capital
(`lifescape_market::partition`, docs/economy.md) : chaque teneur a la
MÊME cible, `market_maker::common_capital_target` = 103 000 pièces
(besoin par tranche 98 312 à 102 590) :

    for index in $(seq 11); do ./rs/target/release/orchestrator create-user --role market_maker; done

Les 11 comptes rejoignent la flotte (nommés, hors staff : mesures,
holdings et hiscores les comptent ; aucune triche, règle 13). Suivi :
`orchestrator economy [h]` (section « market makers » : en tête la cible
commune et l'étendue des besoins et tailles de tranche ; par teneur
tranche, objets et besoin de la tranche, phase, cible, capital, P&L,
écart moyen coté, part du carnet, stock) et la
carte « Market makers » de `/economy` (`market_maker_report.rs`).
Bascule : dans le journal, `brain <c>: market maker slice s/11: capital
… >= target …, quoting for good` ; puis `launching market make --post
x,z,0 …` et `market maker <c>: … refused` si un ordre est refusé.
Migration des vies d'avant l'équilibrage (2026-11-16) : au chargement,
`brain <c>: market maker role migrated to the balanced slices: slice
s/11, common capital target 103000` (une fois par chargement tant que la
vie n'est pas réécrite) ; indice de tranche, phase et base de P&L gardés.

## Financement des teneurs : `fund-makers` (2026-11-17)

`orchestrator fund-makers [--dry-run] [--force]` (`src/funding_cmd.rs`, plan pur
`src/funding_plan.rs`, dons `src/funding_gifts.rs`) : les comptes les
plus riches de la flotte donnent des pièces aux teneurs encore en phase
de jeu pour qu'ils atteignent la cible commune et basculent. Commande
ponctuelle, idempotente, qui interroge le daemon EN COURS (ne le démarre
jamais : il faut la flotte connectée).

- Teneur (vie au rôle `market_maker`, phase earning, connecté, banque
  streamée) : manque = cible − capital − dons en vol vers lui ; capital =
  pièces du sac + banque (+ portées) de l'instantané live, lues comme la
  bascule les lit (`funding_cmd::live_coins` = `brain_plan::holdings_count`).
  Teneur en cotation, hors ligne ou banque non streamée : listé `skipped`,
  jamais financé. Un manque < 1 000 pièces est laissé à son jeu.
- Donateur : compte connecté avec une vie, jamais admin/admin2, bite ni la
  liste `run/economy-excluded.txt`, jamais un teneur, jamais un Iron
  (n'échange pas), banque streamée. Réserve nommée gardée : pièces
  engagées (bids au repos + deals acheteurs en vol,
  `brain_gear::committed_coins`), prochain achat d'équipement
  (`brain_gear::next_gear_cost`) et d'outil (`brain_tool_shop::
  next_kit_price`), marge 10 % des pièces (au moins 5 000), dons déjà dus.
  Le reste est donnable.
- `--force` (utilisateur 2026-11-17, « force les plus riches ») : le
  prochain achat n'est plus réservé et la marge tombe au plancher 5 000 ;
  seules les pièces engagées (bids, deals acheteurs) restent intouchées
  (un bid rempli doit être payé). Sans `--force` le 17/11, 8 dons
  seulement (204 719 pièces), 622 217 non couverts ; avec, 11 dons
  couvrant les 9 teneurs restants.
- Chaque teneur est financé jusqu'à cible + 2 000 (`SWITCH_BUFFER`) : il
  joue jusqu'à sa passe de bascule et un achat entre-temps ne doit pas le
  refaire passer sous la cible (jim_ford, 17/11 : 102 070 / 103 000 après
  un don à la cible exacte).
- Répartition gloutonne : teneurs par manque décroissant, chacun servi par
  le donateur le plus riche restant, puis le suivant.
- `--dry-run` imprime le plan (`donateur -> teneur : montant`, réserves,
  `skipped`) sans rien créer. Sans l'option, chaque don devient un deal
  « don » (objet coins, prix 0, vendeur = donateur, acheteur = teneur) :
  convocation place de Varrock comme un deal (fenêtre de regroupement,
  départs échelonnés par ETA, partie absente ouverte), `market deliver`
  sur les deux workers (le donateur met les pièces dans la fenêtre, le
  teneur rien, double écran vérifié, réglé sur « Accepted trade. »), le
  teneur compte ce que son sac a gagné (`received`). Échec typé
  (`no_show`, `mismatch` = refus, `short`, `timeout`, `nav_down`) : retenté
  une fois, puis `abandoned`.
- Journal : une ligne JSON par événement dans `run/funding.jsonl`, une
  ligne `funding: gift …` par événement dans le log du daemon ; section
  « funding » d'`orchestrator economy` (par don : essai, donateur,
  teneur, montant, état, `received ok` / `MISMATCH`).

Procédure live :

    ./rs/target/release/orchestrator fund-makers --dry-run   # relire le plan
    ./rs/target/release/orchestrator fund-makers             # créer les dons
    ./rs/target/release/orchestrator fund-makers --force     # si « uncovered » reste > 0
    ./rs/target/release/orchestrator economy 1               # section funding
    grep -E "funding: gift|market maker slice|market make --post" run/orchestrator.log

À vérifier : chaque don `settled` avec `received ok` ; puis par teneur
financé `brain <c>: market maker slice s/11: capital … >= target …,
quoting for good` à sa passe suivante, `launching market make --post
x,z,0`, ses premières cotations au carnet (motif `market_make`, section
« market makers » d'`economy` : ordres ouverts, écart, part du carnet)
et ses livraisons à la fontaine (deals `settled`). Relancer
`fund-makers --dry-run` ensuite : plus aucun don (idempotence).

## Quarantaines, rapports en vol (2026-11-17)

Au démarrage (donc à chaque déploiement), une fois `restore_jobs` fini, le
daemon re-teste sur `admin` chaque méthode en quarantaine (D1,
`quarantine_retest.rs`, ligne `quarantine retest: …` par méthode, sortie
dans `run/logs/method_retest.log`). Un rapport poussé mais pas encore plié
(`brain_inflight`) n'est jamais un « no report: worker went idle » ; le
graphe est plié avant l'ingestion des taux. `rates: no profit row` est noté
une fois par (méthode, bande) et par heure.

## Bots échoués, deals expirés (tour 2, 2026-11-17)

`brain_stranded` : deux dossiers NoRoute `Disconnected` depuis la même case
vers deux destinations, dont la recherche n'a pas dépassé 16 cases de
l'origine, rendent le bot `Stranded` sur cette case : plus aucun lancement
tant qu'il s'y trouve, ligne unique `brain <c>: stranded at (x,z,l): …`
(error_events `stranded`, décision opérateur), ordres retirés (raison
`stranded`). Quitter la case (ou redémarrer le daemon) lève l'état.
D8 : trois deals d'affilée expirés (`timeout` rapporté par le bot lui-même)
retirent tous ses ordres (raison `timeout_streak`, deals appariés en
`no_show`, contreparties remises au carnet) ; un deal réglé remet le
compteur à zéro. Ligne `market: <c> let 3 deals expire in a row …`.
Lancement refusé (P2-5) : une réponse `busy: …` / `no session …` du worker
prouve que le script n'a pas démarré ; le nœud revient prêt sans échec
(`brain <c>: nX launch refused, back to ready (uncounted)`, historique
`launch_refused`) et repart quand le worker est libre. Une erreur de
transport reste jugée par le détecteur d'inactivité.
Re-test des quarantaines (tour 2, P2-10, D7) : une file unique vidée en
parallèle par `admin` et `admin2` ; un compte occupé (exit 3) remet la
méthode en file et la reprend 300 s plus tard ; un verdict `setup` (harnais,
p. ex. `check_dispose`) n'est pas un verdict : re-file (2 fois au plus),
jamais « still quarantined ». Au chargement de `method_health.json`, une
quarantaine que la règle des 3 comptes ne poserait pas (fiches d'avant P4,
`accounts {}`) est levée en probation (compteurs gardés).

Avoirs live lus par id (tour 3, point 7) : `live_items::live_units` /
`entry_is` comptent une pile `inv`/`bank`/`worn` par son id d'objet (son
billet compris), le nom seulement pour une entrée sans id ; plusieurs
objets partagent un affichage (« Emerald amulet » enfilée et non enfilée).
Lecteurs : `brain_plan::raw_holdings_count`, `live_owned`,
`brain_ambition::live_count`. Une ligne de rapport (nom affiché) ne compte
pour la cible d'un nœud que si la méthode du run la nomme ainsi
(`brain_fold::gained_toward`, `rates_names::debug_in`).

## Découpage en couches (phase A, 2026-11-23)

La crate `lifescape-orchestrator` est découpée en couches, d'abord à l'intérieur de la crate (phase A, sans changement de comportement), puis en crates séparées (phase B). Ordre, du bas vers le haut :

| couche | contenu |
|---|---|
| `core` | `logln!` + `log_sink`, `paths`, `clock`, `client`, `log_markers`, `cpu_profile`, `res_stats`, `store_pool`, `names`, `api_shapes`, `economy_guard`, journaux |
| `world` | cartes (`png`, `mapcache`, `minimap`, `zones`...), `plan_service` |
| `fleet` | `jobs` (tables de jobs, registre de supervision, `live_of`, `merge_light`), trait `Fleet`, `registry`, `site_crowd`, `worker_owner`, `daemon_pool` |
| `market` | moteur du marché (`market_service`, `_orders`, `_reprice`, `_convene`, `_daemon`...), `funding_gifts`, `market_host` (trait `MarketHost`), `market_facts` (trait `AccountFacts`) |
| `stats` | `rates_*`, `report_*`, `flow_causes`, `holdings`, `hiscores`, `guides_store`, `guides_model`, `rates_ingest`, `errors_model` |
| `session`, `brain` (même niveau) | worker `session*`; `brain*`, agents de marché (`market_maker`, `market_seller`, `market_demand`, `market_discovery`), `bid_why`, `method_health` |
| `web` | `web*` |
| `daemon` | `daemon*`, `supervise`, `push_hub`, `errors_ingest`, `rares_cmd`, `funding_*`; porte `Orchestrator` |
| `bin` | `main`, `cli`, commandes hors ligne, `dump_*`, `verify_*` |

Règle : un module ne référence que sa couche ou une couche plus basse. Ce qui doit remonter passe par un trait enregistré au démarrage : `Fleet` (le démon vu du cerveau, du marché et des statistiques), `MarketHost`, `AccountFacts` (le cerveau vu du marché), `log_sink` (le démon vu de `logln!`). Ces enregistrements se font en tête de `main`, avant tout thread; `log_sink::assert_registered` échoue bruyamment s'ils manquent. Le contrôle (`layering_check.py`, à brancher dans le dépôt) compte les références de code de production qui vont vers une couche égale ou supérieure : 63 avant la phase A, 0 après.

### Phase B : crates séparées (état au 2026-11-23, étapes B1 à B10, terminée)

Les neuf couches sont des crates de `rs/crates/`, une par couche, chacune ne dépendant que de celles du dessous. `lifescape-orchestrator` les ré-exporte dans son `lib.rs` (`pub use lifescape_orch_core::{paths, ...};`), si bien que `crate::paths::x`, `crate::jobs::Job`, `crate::logln!` s'écrivent partout comme avant; chaque crate ré-exporte de la même façon celles du dessous (y compris `logln`).

| crate (paquet) | contenu | dépend de |
|---|---|---|
| `orch-core` (`lifescape-orch-core`) | `logln!`, `daemon_clock`, `log_sink`, `paths`, `clock`, `client`, `log_markers`, `cpu_profile`, `error_stamp`, `log_rotate`, `disk_guard`, `res_stats`, `store_pool`, `names`, `api_shapes`, `nav_profile`, `economy_guard` | json, store, nav |
| `orch-world` | `png`, `mapcache`, `mapbuild`, `minimap`, `staticmap`, `zones`, `plan_service` | core |
| `orch-fleet` | `jobs`, `fleet` (trait `Fleet`), `registry`, `site_crowd`, `worker_owner`, `daemon_pool` | core |
| `orch-market` | moteur du marché, `funding_gifts`, `market_host`, `market_facts`, `market_own` (mémoire du vendeur sur le moteur), `market_test_support` | core, world, fleet |
| `orch-stats` | `rates_*` (dont `web_rates`, sous-module de `rates_service`), `report_*`, `flow_causes`, `time_value`, `xp_*`, `hiscores*`, `holdings*`, `live_items`, `guides_store`, `guides_model`, `rates_ingest`, `errors_model`, `errors_aggregate`, `trade_report`, `economy_outcomes` | core, world, fleet, market |

| `orch-stats-model` (`lifescape-orch-stats-model`) | le modèle pur des statistiques : `report_meta`, `report_contract`, `report_flows`, `flow_causes`, `report_progress`, `rates_parse` (analyse des rapports, prédicats de méthode, `NORMALIZE_TICKS`, `LEVEL_BAND`), `rates_estimator`, `errors_model`, `errors_aggregate` : ni global, ni fil, ni service | core, world |
| `orch-session` | worker `session*`, `push_conn`, `heartbeat_flows` (pompe du battement) | core, world, fleet, market, stats-model (JAMAIS `orch-stats`) |
| `orch-brain` | `brain*`, agents de marché (`market_maker` avec `register_account_facts`, `market_seller`, `market_demand`, `market_discovery`, `market_maker_report`), `bid_why`, `method_health`, `autopilot_impl` (le cerveau vu du démon et du web), `funding_cmd`, `funding_plan`, `rares_cmd`, `web_brain`, `web_brain_view`, `web_job_lives`, bancs `par_harness`; `tests/fixtures/lives` | core, world, fleet, market, stats |
| `orch-web` | `web*`, `web_rates_view`, `hiscores_web_tests` (plus de `web_brain*`, `web_job_lives` : au cerveau) | + session (`heartbeat_flows`) ; JAMAIS le cerveau (trait `Autopilot`) |
| `orch-daemon` | `daemon*`, `supervise`, `push_hub`, `errors_ingest`, `retention`; porte `Orchestrator` et `impl Fleet` | + session, web ; JAMAIS le cerveau (trait `Autopilot`) |
| `orchestrator` | binaire unique `orchestrator` (`main`, `cli`, commandes hors ligne, `dump_*`, `verify_*`, `migrate_cmd`, `purge_cmd`...) et `lib.rs` qui ré-exporte tout (`crate::x::y` inchangé) | toutes |

DAG des dépendances (une flèche = « dépend de ») :

```
core <- world, fleet
market -> core, world, fleet          stats -> + market
session -> core, world, fleet, market, stats
brain   -> core, world, fleet, market, stats          (session et brain sont frères)
web     -> brain, session, stats ...  daemon -> web, brain, session ...  orchestrator -> toutes
```

Le binaire reste unique : les workers lancent `current_exe() session`, le déploiement lance `rs/target/release/orchestrator`. Le démarrage de `main` est inchangé : `register_log_sink`, `assert_registered`, `register_account_facts` d'abord, avant tout thread.

Lancer les tests d'une crate (profil incrémental, jamais le profil debug, il remplit le disque) : `RUST_TEST_THREADS=6 nice -n 19 cargo test --profile release-test --manifest-path rs/Cargo.toml -p lifescape-orch-<crate>` (`brain`, `web`, `daemon`, `session`...; `-p lifescape-orchestrator` pour ce qui reste). Suite complète : même commande avec `--no-fail-fast` sans `-p`. Les fixtures de vies sont dans `rs/crates/orch-brain/tests/fixtures/lives` (`migrate_cmd_tests` les lit par `../orch-brain/...`); le test `brain_d17_tests` balaie `orchestrator` et toutes les `orch-*`.

Reste à faire (facultatif, étape C) : un trait `Autopilot` (`on_supervise`, `note_operator_stop`, `request`, `explain_for`, `wishes_reply`) enregistré par le cerveau, pour que démon, web et cerveau deviennent des crates sœurs (aujourd'hui web et démon dépendent du cerveau, le démon du web). Une modification d'un fichier du cerveau recompile donc brain, web, daemon puis le binaire.

Tests entre crates : la fonctionnalité cargo `test-support` (de `orch-market` et `orch-stats`), activée par les `dev-dependencies` des crates au-dessus, ouvre les aides de test (`market_test_support`, `market_facts::test_support`, bascules `cfg!(test)` du marché : première commande au carnet continu, patience 0,5, cotation à l'unité). Sans elle, le code de production est celui du démon. Les faits de compte de test (`market_facts::test_support::register`) remplacent l'ancien repli sur ceux du cerveau : les tests qui cotent un objet jamais échangé l'appellent. Le contrôle `layering_check.py` lit maintenant toutes les crates `orch-*` et refuse un fichier rangé dans une autre crate que celle qui lui est assignée.

## Isolation des tests (2026-11-23)

Règle pour toute nouvelle crate `orch-*` : si elle dépend (même indirectement) de `lifescape-orch-core`, `lifescape-sdk` ou `lifescape-store`, elle déclare dans ses `[dev-dependencies]` ces dépendances avec `features = ["test-isolation"]` (le résolveur 2 garde la feature hors du binaire `--release`) et ajoute `tests/test_isolation.rs` (copie de celui d'`orch-brain`). N'utiliser jamais `cfg!(test)` pour isoler un chemin d'une autre crate : seul `cfg!(any(test, feature = "test-isolation"))` est fiable. Les chemins d'état passent par `paths::run_dir()` (jamais `root().join("run")`); `root()` reste réservé aux ressources du dépôt en lecture seule (gamedata, static, scenarios). Le garde-fou `paths::assert_not_live` / `Store::open` panique sur tout chemin sous le vrai `run/`; un test qui lit une copie fixe `LIFESCAPE_ROOT` ou `LIFESCAPE_LIVES_DIR`.

## Mesure de référence et profil des fils (plan d'architecture, lot 0.1, 2026-11-24)

Deux outils hors démon, en lecture seule, dans `tools/` :

- `tools/baseline.py [--hours 1] [--end UNIX] [--json]` : ouvre `run/lifescape.db` en `?mode=ro` et lit, par horloge Unix (fenêtre `[fin - heures, fin)`, par défaut la dernière heure pleine), les rapports par heure et la part de chaque `stop` (`goal_reached`, `blocked`, `failed`, `budget_exhausted`…), les ticks de travail et de pause par heure, l'XP par heure (`task_yields.xp`), les `error_events` par heure et par type, et le délai rapport → prise en compte (seule donnée tirée du journal du démon, lu sur sa fin : ligne `task_report … (push)` puis `folded report` du même bot). À lancer pour l'heure d'avant un déploiement et pour l'heure d'après, puis à comparer.
- `tools/cpu_sample.py <fenêtres> <secondes>` : CPU par groupe de fils du démon (nom du fil sans son numéro final), des ouvriers, du serveur de jeu et du relais, lu dans `/proc`. Deux relevés à une heure d'écart donnent des chiffres voisins.

### Profil du démon complet (lot 0.2, 2026-11-24)

Cinq lignes `cpu-profile` par minute (le journal du démon) :

- `cpu-profile threads (60s): cpu_ms <groupe>=<ms>x<fils> … exited=<ms> total=<ms> threads=<n>` (`orch-core/src/thread_profile.rs`) : le CPU de TOUS les fils du démon, par nom de fil (numéro final retiré). Rien n'est sans attribution : `total` est le CPU du processus (utime + stime), `exited` ce qu'ont consommé les fils morts dans la minute (`worker-connect`, `session-conn`…), et somme des groupes + `exited` = `total`. Les fils sans nom (dont le principal) forment le groupe `orchestrator`.
- `cpu-profile brain` : inchangée, plus la phase `resize_session_inputs` (le suspect de la part non étiquetée du cerveau).
- `cpu-profile tally` : compteurs d'appels du fil du cerveau, `count` = appels dans la minute (les rapporter à `ticks`) : `locked_items` (2 requêtes en base chacun), `live_copy` (copie profonde de l'état vivant `JobInner::live_copy`), `ticks`.
- `cpu-profile supervise` : `sweep` = un tour complet du superviseur sur tous les bots (sommeil de 500 ms exclu), `once` = un `supervise_once` (appel `status` bloquant compris).
- `cpu-profile fold` : délai rapport → prise en compte par le cerveau (reçu par le canal des battements, `note_report_received`, jusqu'à `note_report_folded`), un genre par tranche (`le_1s`, `le_5s`, `le_10s`, `le_20s`, `gt_20s`), `count` = rapports, `wall_ms` = délai cumulé (moyenne = `wall_ms / count`). Remplace le script `latency.py` : plus besoin de relire le journal.

### Trame du tableau de bord sans copie (lot 1.2, 2026-11-24)

La liste des bots de la trame d'1 s ne passe plus par `status` (qui appelait `Job::describe()` sur les 74 bots : copie profonde de l'état vivant de 35 à 85 Ko chacun, pour n'en garder que 9 colonnes). Le démon répond à la commande `job_list` : `Job::list_row()` ne copie que les petites valeurs (`combat`, `position`, `zone`, `hitpoints`, état, script) de l'instantané, avec les mêmes cas limites que `describe` (instantané absent : fichier de battement ; fiche gelée : ligne « contended » aux colonnes nulles), triées comme `status` (`jobs::sort_status_rows`). Les colonnes sont `jobs::LIST_FIELDS`. La liste (avec but et tâche des fichiers de vie) est partagée entre les onglets pendant 500 ms (`web_ws::shared_job_list`) : un onglet seul la reconstruit à chaque trame, N onglets au plus deux fois par seconde. Le rafraîchissement reste d'1 s. `status` (CLI) est inchangée. Preuve : `the_list_frame_equals_the_described_frame_byte_for_byte` (trame identique octet pour octet à celle de `describe()` sur 8 bots : instantané complet, au repos, en pause, sans instantané avec fichier de battement, sans instantané ni fichier, instantané vide ou nul, fiche gelée).

### Battements, échantillonneur d'avoirs, événement aléatoire (lot 1.3, 2026-11-24)

- Octets d'un battement : le hub (`push_hub`) passe au démon la longueur de la ligne reçue avec la trame analysée (`Orchestrator::ingest_push_sized`), plus de re-sérialisation complète de chaque trame pour la compter. L'ouvrier écrit `stringify` + saut de ligne : la longueur de la ligne est celle que comptait `res-stats` (`push_bytes`). `ingest_push` (socket CLI, banc hors ligne) compte encore par `stringify`. Preuve : `the_received_line_length_equals_the_reserialised_length` (4 formes de trames de ouvrier, échappements, unicode, flottants, entiers extrêmes).
- `holdings-sampler` : réveil à la minute pour un point par heure ; il décrit les bots (`described_jobs`) et somme les sauvegardes seulement quand un point est dû (`maybe_sample_lazy`, la base dit d'abord si le dernier point est assez vieux). Sans fichier de base, rien n'est créé pour une flotte vide, comme avant. Les points écrits sont ceux de l'ancien chemin.
- `clear_event_if_gone` (superviseur) lit l'instantané en place sous le verrou (`event_is_gone`) au lieu de le copier (35 à 85 Ko par bot et par tour). Preuve : `the_in_place_read_gives_the_verdict_of_the_copy` (même verdict que l'ancien code sur toutes les géographies : labyrinthe, mime, chaque tuile d'échec, cube en objet ou en nom, `updated` absent ou périmé, instantanés non objets).

### État vivant en `Arc` (lot 1.5, 2026-11-24)

`JobInner::live` est un `Option<Arc<Value>>` : le dernier instantané poussé (35 à 85 Ko d'arbre JSON) est partagé, plus jamais copié par un lecteur. `live_shared()` / `live_or_null()` rendent le pointeur (le cerveau, le marché, le superviseur, `describe`, `live_of`) ; `live_copy()` (copie profonde) n'est gardée que pour qui exige un arbre à soi et compte dans la ligne `cpu-profile tally` (`live_copy` doit tomber à 0 par passe, `live_shared` compte les lectures). Un battement complet remplace le pointeur et **déplace** l'arbre de la trame reçue dans le job (`ingest_push_owned`, plus de copie de `frame.snapshot`) ; un battement léger fusionne en place (`Arc::make_mut`) : l'arbre n'est copié que si un lecteur tient encore l'ancien pointeur, qui garde alors son instantané cohérent. `LaunchPass.live`, `Holdings` et les instantanés de la flotte (`brain_ambition::ensure`, qui copiait aussi l'état par bot) portent le pointeur. Les lecteurs qui recopient seulement un tableau (`inv`, `bank`) en gardent la copie : hors du périmètre du lot.

### Rétention : dossiers d'échec et ordres annulés (décision 3, 2026-11-24)

Le fil `retention` du démon (`orch-daemon/src/retention.rs`) passe dix minutes après le démarrage puis toutes les heures : il supprime les fichiers `*.json` de `run/failures/` plus vieux que **7 jours** (date de modification), et les ordres **annulés** de la base plus vieux que **14 jours** (dernière mise à jour) avec leur journal `order_amends`, par lots de 2 000 ordres (une transaction courte, 250 ms de pause, le carnet reste écrivable). Une ligne `retention: purged …` n'apparaît que si quelque chose est parti. Sont gardés pour toujours : ordres ouverts, appariés et réglés, échanges (`deals`), exécutions. Un ordre annulé n'est supprimé que si toute sa lignée (`root_id`) est finie : aucun frère encore ouvert, apparié ou réglé, aucun mis à jour après la coupure (la mesure `market_criteria` lit une lignée d'achat en entier ; on ne la coupe pas en deux).

Commande opérateur : `orchestrator retention --check` compte sans rien supprimer (peut tourner pendant que le démon tourne) : `retention (dry run): would remove N cancelled orders older than 14 days (+M order_amends), K failure dossiers older than 7 days (X MB)`. Sans `--check`, la purge à la main refuse tant que le démon tourne (il purge seul) : arrêt propre d'abord.

Plus longue fenêtre de lecture des ordres annulés (relevée le 2026-11-24) : l'apprentissage du vendeur (`seller_learning`, `cancel_reason = 'horizon'`) lit **24 h** (`market_own::LEARNING_WINDOW_SECS`) ; les issues d'ordres de la page économie lisent 1 h, 24 h, **7 jours** et « tout l'historique » ; `market_criteria` lit les lignées touchées dans la fenêtre de `orchestrator economy [heures]` (24 h par défaut). 14 jours dépasse donc la plus longue fenêtre automatique (7 jours). Seuls la fenêtre « tout l'historique » de la page et un `economy` demandé sur plus de 14 jours voient moins d'ordres annulés qu'avant. Les dossiers d'échec ne sont lus que pour afficher le détail d'une erreur (`web_errors`) et pour alimenter `error_events` (curseur par date de modification : supprimer un vieux fichier n'en recrée pas la ligne) : au-delà de 7 jours le lien du dossier d'une ancienne ligne d'erreur ne s'ouvre plus.

### Vue de passe : lectures du compte numérotées par changement (lot 3.1, 2026-11-24)

Les verrous du marché (`brain_locks::locked_items`), les pièces engagées, les annonces, les faits d'entretien et les relectures `// lot 3:` lisent tous `Store::account_orders` et `Store::open_deals_for_account` : deux requêtes en base par lecture, sous le verrou commun du marché, 5 à 6 fois par passe complète et des centaines de fois dans `trade.publish`. Ces deux lectures sont maintenant servies par une copie tenue **dans le `Store`** (`store/src/account_views.rs`), numérotée par `Store::change_stamp()` = (lignes changées par cette connexion, `PRAGMA data_version`) : le premier compteur bouge à tout INSERT/UPDATE/DELETE de la connexion (déclencheurs compris), le second à toute validation d'une AUTRE connexion du même fichier. Un numéro changé vide la copie entière ; un numéro inchangé sert la copie. Le numéro est lu avant la requête : un changement entre les deux donne une ligne plus récente que son numéro, relue à l'appel suivant, comme un changement juste après une lecture sans copie.

Écart au plan, assumé : le plan disait « lues au début de la passe, relues aux points `// lot 3:` ». La copie numérotée relit exactement quand quelque chose a changé (toute action de marché de la passe, mais aussi tout autre fil), donc ne sert jamais plus ancien que la base ; les relectures `// lot 3:` restent aux mêmes endroits et rendent les mêmes valeurs, sans requête quand rien n'a bougé. L'état vivant est déjà lu par pointeur (lot 1.5). Seule différence connue : une suppression par la rétention (`retention`, connexion séparée) ne touche que des ordres annulés, que ces deux lectures ne rendent pas.

Preuve : `account_views_tests` (400 étapes aléatoires : ordres, annulations, états de commandes, accords, validations d'une seconde connexion ; copie == base après chaque étape, deux lectures de suite) ; les bancs de lancement et d'entretien (monde brut, chemins de production) vérifient `copie == base` à chaque passe lue (`assert_account_views_fresh`) ; compteurs `account_view_stats()` (servies depuis la copie / lues en base).

### Vue du marché partagée, numérotée par génération (lot 3.2, 2026-11-24)

`brain_bids::MarketReader::live` recopiait tous les carnets pour chaque bot (`wait_answer` 868 ms, `bids.goal_for` 237 ms par appel) et faisait une requête de volume sur 24 h par objet candidat. `brain_market_view` publie deux vues partagées :

- `SharedBooks` : les ordres de chaque carnet (acheteurs avec leur propriétaire, vendeurs), construits sous le verrou du moteur et publiés en `Arc`. Génération = nombre de carnets + somme des compteurs `Book::version` (bougés par tout appel `&mut` qui peut changer un ordre : `place_guarded`, `relist`, `relist_matching`, `uncross`, `run_auction`, `cancel`, `amend`, `set_batch`). L'exclusion des offres d'achat du bot reste par bot : `ItemBook::view_for(compte)` la fait à la lecture d'un objet, jamais dans la copie partagée. Le lecteur (`MarketReader`) ne construit plus que les objets qu'on lui demande.
- Volume 24 h : les exécutions d'un objet sur un jour sont copiées une fois par `Store::executions_stamp()` (le journal ne fait que croître ; une réparation le réécrit avec des numéros plus grands) et sommées pour la fenêtre à l'horloge de l'appel : la fenêtre glissante reste exacte à toute heure.

Les deux sont rangées sous `MarketService::instance_id()`. Seule différence : l'ordre des lignes de prix égal était celui d'une table de hachage (non spécifié) ; il suit maintenant l'identifiant d'objet (déterministe).

Preuve (`brain_market_view_tests`) : 400 étapes aléatoires (ordres de six comptes dont deux ne différant que par la casse, annulations, croisements) : la copie publiée == une copie reconstruite, et la vue de chaque demandeur == l'ancien calcul (`views_from`) objet par objet ; `MarketReader::live` == lecteur de référence par bot sur `view`, `bid_books`, `value_books` ; volume 24 h == `volume_between` à 6 horloges dont une avant la copie ; **choix de lot** (`choose_in`, métier propre et marché entier) identique pour chaque vie sur un marché de test peuplé : 18 choix (9 vies committées), dont 16 avec un lot ; vies réelles : voir `decisions.md`.

### Tirage retenté seulement si ses entrées ont changé (lot 3.3, 2026-11-24)

`brain_succession::redraw` (le tirage d'un but fermé, `succession.redraw` : 1 941 appels, 2 091 s de CPU en 19 h) était retenté à chaque passe tant que rien n'était tirable. `brain_redraw_gate` calcule l'empreinte de ses entrées en six familles de hachages : la vie (but, origine, ambition, mémoire, queue de l'historique, graphe), les interdictions et refroidissements (leurs ÉTATS, donc l'empreinte bouge exactement quand une échéance tombe, et les méthodes en quarantaine), le bot (niveaux, piles, verrous, quêtes finies), le marché (génération des carnets, journal d'exécutions, service), les guides et les taux (heure de construction, `rates_service::generation`), la flotte (meilleurs niveaux, dernier point d'avoirs horaire, sites et places des autres, créances et achats des autres, mesure de valeur du temps). Une relance dont l'empreinte égale celle de la dernière tentative qui n'a rien tiré, faite il y a moins de `MAX_SKIP_SECS` = 120 s, ne pourrait que ne rien tirer : c'est elle que la porte saute. La borne d'âge couvre toute entrée que l'empreinte ne nomme pas.

**Interrupteur : variable d'environnement `LIFESCAPE_REDRAW_GATE`** (lue une fois, au premier tirage) :
- `shadow` (**défaut**, aussi pour toute valeur inconnue ou absente) : l'ancien code décide, la porte calcule à côté. Un saut permis que l'ancien code a pourtant suivi d'un but tiré ou d'une vie modifiée est une DIVERGENCE : ligne `redraw-gate: DIVERGENCE <compte> ...` dans le journal du démon et compteur. Résumé toutes les 10 minutes : `redraw-gate: mode=shadow retried=… would_skip_agreed=… would_skip_drew=… would_skip_changed=… skipped=0`.
- `on` : une relance sautable rend `(None, false)` sans tirer.
- `off` : ni empreinte ni porte (le code d'avant).

Bascule : lancer le démon sans la variable (shadow), attendre une heure en vrai, lire `would_skip_drew` et `would_skip_changed` dans les résumés (`grep 'redraw-gate:' run/orchestrator.log`) ; à **zéro** divergence, relancer avec `LIFESCAPE_REDRAW_GATE=on`. Gain attendu : `would_skip_agreed` sur `retried + would_skip_agreed` est la part des tirages qu'`on` économise.

Preuve hors ligne (`brain_redraw_gate_tests`) : l'empreinte bouge avec chaque famille et seulement avec elles (vie, niveau, ordre du marché, site d'un autre bot, échéance d'une interdiction) ; les trois modes ; et, par le chemin de production (`brain_succession::redraw` sur des mondes bruts qui changent entre deux passes : enchère, niveau, interdiction levée ou posée, ambition donnée ou retirée), la porte en mode shadow ne permet jamais un saut que l'ancien tirage dément.

### Mémos des fonctions pures (lot 3.4, 2026-11-24)

Trois relectures que chaque passe refaisait alors qu'elles ne dépendent que d'entrées qui bougent rarement :
- **Choix des lignes du guide d'argent** (`brain_money::bot_rows`, appelé par `brain_market::fill_ctx` → `guide_coins` à chaque nœud de chaque passe, et par `brain_d17::best_row`) : sans stock, c'est une fonction pure de (jeu de guides, niveau du bot dans chacun des 23 savoir-faire). Le choix `(niveau de la vue, indice de ligne)` est mémorisé par jeu de guides (adresse + heure de construction ; un nouveau jeu vide la table, 4 096 entrées au plus) ; les lignes rendues sont celles du jeu lui-même. `guide_coins` ne lit plus les lignes que pour un nœud d'argent (les autres ne s'en servaient pas). `bot_rows_uncached` reste la référence.
- **`resize_session_inputs`** : sortie rapide (`planner::session_resize::has_open_climb_inputs`) quand aucun nœud de montée de niveau ouvert n'a de dépendance « obtenir » ouverte : le redimensionnement ne rend alors aucune ligne et ne change rien ; les lecteurs (vue du bot, taux, guide) ne sont plus construits. Le libellé `cpu_profile` `resize_session_inputs` est conservé (l'étiquette ouvre avant la sortie).
- **`method_health::quarantined()`** : le registre des vérifications en direct (`run/method_checks.jsonl`) était relu et analysé à chaque appel dès qu'une méthode est en quarantaine ; il l'est maintenant seulement quand la taille ou l'heure de modification du fichier change (`checks_uncached` reste la référence).

Preuve (mémo == sans mémo) : `the_memoised_rows_equal_the_uncached_rows` (600 tirages de niveaux × 2 lectures, 3 jeux de guides en alternance : froid, chaud, après changement de jeu, au retour : mêmes lignes, mêmes adresses), `the_fast_exit_equals_the_full_resize` (576 graphes des vies committées dont un tiers avec une montée de forgeron nourrie : 507 sorties rapides, 40 redimensionnements, même réponse et même vie), `the_memoised_checks_equal_a_fresh_read_after_every_change` (pas de fichier, ajout, ligne `setup`, réécriture de même taille, suppression).

### `BrainDeps` : les lectures passent par des poignées données à la passe (lot 3.5, 2026-11-24)

`brain_deps::BrainDeps` regroupe le marché (`Option<Arc<MarketService>>`), les taux (`Arc<Snapshot>`), les guides (`Option<Arc<GuideSet>>`), la foule de la flotte (`dyn Crowd`) et l'horloge (`dyn Clock`). `BrainDeps::live()` lit les singletons du démon une fois, à l'ouverture de la passe de lancement (`brain_launch_steps::open`) ; la passe le porte (`LaunchPass.deps`) et le donne à `LiveFx` et `LiveUpkeepFx`. Les tests construisent le leur (`BrainDeps::with_market(..).with_crowd(..).with_clock(..)`) : un marché de test, une foule (`TestCrowd`) et une horloge qu'ils maîtrisent, sans rien installer de global pour ces lectures.

Modules convertis (un commit par groupe) :
1. Entrées de lancement : `brain_launch_inputs::{gather, gather_market, refresh_clock}`, `brain_locks::locked_items_in`, `LiveFx::{refresh_market, refresh_clock, loot_facts, cleanup_facts}`.
2. Contexte de résolution et entretien : `brain_plan::ready_ctx_in` (taux), `brain_market::fill_ctx_in` (marché, guides, `guide_coins`), `LiveUpkeepFx` (`orders`, `cancel`, `facts`).
3. Publieurs de contexte : `brain_trade::publish`, `brain_money::publish_in` / `publish_keeping` (guides, marché), `brain_train::fill_ctx` (guides), le remplisseur (`brain_filler::{window_ctx_in, publish_window_in, launch_filler}` : foule, marché, guides). Les anciennes signatures `ready_ctx`, `fill_ctx`, `window_ctx`, `publish_window`, `brain_money::publish` restent comme enveloppes (poignées du démon) pour les mesures de l'orchestrateur (`cpu_bench_tests`).

4. Fin du lot (2026-11-24, « 3.5 achevé ») : tirage de but (`brain_bids`, `brain_succession`, `brain_ambition`, `brain_redraw_gate` : empreinte comprise), matériel (`brain_gear`, `brain_tools`, `brain_tool_shop`, `brain_wishlist`, `brain_stock`, `brain_sales`, `brain_quest_gate`, `brain_arbitrage`), effets de marché (`brain_market`), alchimie (`brain_alch`), agents de marché (`market_demand`, `market_discovery`, `market_maker`, `market_seller`), verrous (`Holdings::of`, `holdings_count`, `brain_plan::carried_tools`, par `brain_locks::locked_items_in(deps.market, ..)`) et prix (`rates_price::price_of_in(deps.market, ..)`, `price_of` reste l'enveloppe des appelants hors cerveau). Un module converti prend `deps: &BrainDeps` en dernier paramètre ; les points d'entrée lisent les singletons UNE fois : `brain_fold::handle_report` (`BrainDeps::live()` à l'entrée d'un rapport), la passe de lancement (`LaunchPass.deps`, étapes et `LiveFx`/`LiveUpkeepFx` par `self.deps`), les rappels du moteur de marché (`LifeFacts::{public_anchors, seller_quote}`) et les commandes du démon (`explain_for`, `fund-makers`) ; `Commerce.deps` porte les poignées aux pas de commerce. `MarketReader` garde ses poignées (`deps: Option<BrainDeps>`, `None` hors ligne). `BrainDeps`, `Clock`, `Crowd` sont publics (les commandes du démon et du web les construisent).

**Singletons restant dans `orch-brain`** (relevé 2026-11-24 après le lot 3.5 achevé, hors tests, commentaires et références ; avant : entre parenthèses, relevé du 1/3) :
- `market_service::shared()` : 2 (44) : la lecture unique de `BrainDeps::live` et l'enveloppe `brain_locks::locked_items` (appelée par le seul banc de CPU). Hors cerveau, 7 de plus à la frontière : `orch-stats` (`time_value`, `rates_price::price_estimate` et l'enveloppe `price_of`), `orch-web` (`web_brain`), `orch-daemon` (`funding_cmd`, `rares_cmd`), `orch-market` (arrêt).
- `guides_store::current()` : 1 (6) ; `rates_service::live_snapshot()` : 1 (5) ; `site_crowd::others/seats` : 2 (5), les deux dans `FleetCrowd` ; toutes dans `BrainDeps::live` / ses enveloppes. Les écritures de la foule (`engage`, `note`, `disengage`) restent globales : ce sont des effets, pas des lectures.
- `live_tick_ms()` : 5 (12) : `WallClock` et quatre feuilles pures (`brain_ambition::stamp_now`, `brain_stuck::seconds_to_ticks`, `brain_npc_sale`, `market_maker::ticks_as_secs`) dont les appelants sont nombreux ; l'horloge des passes de lancement, d'entretien, de commerce, de tirage et de marché passe par `deps.clock`.
- Statiques `OnceLock` / `Mutex` : inchangés (136) : ce sont l'ÉTAT du démon (tableau des créances, foule, table de santé des méthodes, caches de choix, journaux), pas des poignées.

Les bancs (`brain_raw_world_tests`, `brain_launch_equiv_tests`, `brain_upkeep_equiv_tests`, `brain_redraw_gate_tests`, `brain_market_view_tests`) n'ont plus besoin de la surcharge de fil du marché : `market_daemon::test_service` est supprimée, `RawWorld::deps` (marché de test, foule `TestCrowd`) est la seule voie, et `market_daemon::shared()` ne lit plus que le registre du démon.

## Réactivité : battements, réveils du cerveau, `status` sur silence (phase 4, 2026-11-24)

La phase 4 enlève au balayage du superviseur (8 à 11 s sur 74 bots) le rôle de messager : le rapport de tâche arrivait au démon en moins de 20 ms mais n'était pris en compte que 8,5 s plus tard en moyenne (4,6 % du temps de la flotte à attendre d'être remarqué). Cinq lots, un commit chacun ; 4.3 et 4.4 ne sont pas neutres (les décisions arrivent plus tôt) et se jugent sur la mesure d'une heure.

### Le canal des battements (lots 4.1 et 4.2)

- Chaque ouvrier garde UNE connexion `push.sock` pour sa vie. Côté démon, `push_hub` a un fil `push-accept` et **un fil `push-read` par ouvrier, bloqué dans `read`** (pile de 512 Kio, std seul) : il ne se réveille que sur des octets, découpe les lignes, parse et appelle `dispatch` ; il s'arrête à l'EOF. Une reconnexion du même compte remplace le flux (l'ancien est arrêté, l'ancien lecteur ne retire pas son successeur). L'ancien balayage de 20 ms (un fil, lectures non bloquantes) a disparu ; l'objection de E8 contre les fils de sondage par compte ne vise pas ces lecteurs, qui ne sondent rien. Les trames d'un compte restent traitées dans l'ordre (un seul lecteur par compte) ; les comptes entre eux sont concurrents.
- Les trames portent la session : l'enveloppe de la trame pleine (`type: snapshot`) a `phase`, `status`, `sent` (hors de l'arbre `snapshot`), la trame légère a `phase` en plus de `status`. `Job::note_frame` garde la vue (`JobInner.frame`) et applique phase et statut si le job est supervisé et si la trame a été envoyée après la dernière réponse `status`, commande ou mort (`phase_at`).
- Comparaison muette trame / `status` (`frame_check`) avant d'appliquer la réponse : ligne toutes les 60 s `supervise frames (60s): compared= same= diverged= (young= old= phase= status=) raced= stale= no_frame=` et trace des 20 premières divergences (`supervise <compte>: frame/status divergence`). `young` : trame de moins de 1,5 s (transition plausible entre la trame et la réponse) ; `old` : vrai écart ; `raced` : une trame est arrivée pendant l'appel ; `stale` : dernière trame de plus de 5 s ; `no_frame` : jamais de trame (ouvrier ancien).

### Le rapport et les événements réveillent le cerveau (lots 4.3 et 4.4)

- La boîte du cerveau est typée (`brain::Mail` : `Tick`, `Report`, `Wake`) ; les 4 fragments `brain-tick-N` traitent les messages d'un compte dans l'ordre (hachage stable du compte).
- Rapport : `Orchestrator::ingest_push_and_wake` (appelé par le fil `push-read`) range le rapport puis `brain::on_report_push` prend l'indicateur `report_pending` (`Job::take_report`, indicateur et rapport sous le même verrou) et poste `Mail::Report`. Le drainage du superviseur reste en secours ; **le premier à prendre l'indicateur gagne**, jamais deux fois. `inbox full` remet l'indicateur à vrai (comme avant).
- `brain-clock` (un fil) : bloque sur `wake_hub` (couche flotte : `post(compte, Idle|Fill)`, dédoublonné, condvar) et poste aussitôt `Mail::Wake` ; une fois par seconde il poste `Mail::Tick` pour chaque compte supervisé dont la dernière passe (tick, rapport ou réveil) a `SAFETY_TICK_SECS` = 10 s. Le superviseur ne porte plus de tick (`on_supervise` : surveillance d'inactivité + rapport drainé). Événements : phase passée à `idle` (lue dans une trame ; rattrape le rapport qui précède d'une trame le passage à `idle`) et ordre apparié (`market_convene::record_fill`, acheteur et vendeur). Un seul passage en attente par compte ; la décision garde la sémantique d'une décision par passe, `brain_chain` et la porte de retirage (`LIFESCAPE_REDRAW_GATE`, défaut shadow) sont inchangés.

### `status` seulement sur silence, plus de fil par appel (lot 4.5)

`status_poll::plan` décide à chaque tour pour chaque bot : processus suivi mort -> mort traitée sans appel ; bot silencieux (dernière trame de plus de `SILENCE_SECS` = 5 s, ou jamais de trame) -> appel, au plus un par 5 s ; trame à l'écart de la dernière réponse (transition) -> un appel, au plus un par 2 s ; sinon un appel de garde toutes les 60 s ; sinon rien. N = 5 s : l'ouvrier bat au moins une fois par seconde (`HEARTBEAT_WALL_SECS`, boucle de repos de 1 s), soit 5 battements de marge, très en dessous des 45 s du chien de garde. Le tour du superviseur devient court (aucun appel bloquant en régime normal) : les tâches par bot (garde de repos, chien de garde, état écrit) tournent à chaque tour de 500 ms, toutes à coût négligeable. Ligne toutes les 60 s : `supervise status (60s): calls= silent= transition= backstop= skipped=`.

Le chien de garde (`daemon_watch`) lit les `updated` des trames, pas `status` : inchangé, 45 s sans battement redémarre l'ouvrier (3 redémarrages en 5 minutes parquent le job) ; la décision est maintenant une fonction pure testée (ouvrier gelé, en reconnexion, tué : `status_poll::plan` rend `Death`).

`worker-connect` : les appels bornés (`connect` unix sans délai en std) passent par `bounded_call` : fils auxiliaires **réutilisés** (jusqu'à 8 en attente), un appel qui n'en trouve aucun en lance un, un fil bloqué dans un `connect` coincé n'est pas « libre » et ne retarde aucun autre compte. Le gabarit E14 (`worker-connect` dans `cpu-profile threads`) montre maintenant la taille du groupe (quelques fils) et non un fil par appel.

### À regarder pendant l'heure en vrai

Référence : fenêtre d'une heure avant le déploiement, lue dans `run/lifescape.db`. Lignes de `run/orchestrator.log` :
- `cpu-profile fold` : délai rapport -> prise en compte, attendu **sous 1 s** (8,5 s avant) ; `cpu-profile supervise` : `sweep` (attendu en millisecondes, plus de 8 à 11 s), `once`, compteurs `wake_report`, `wake_idle`, `wake_event`.
- `supervise frames`, divergences : attendu `old=0` (tout le reste est de la course) ; plus de quelques `old` par heure, c'est que la trame ne dit pas la vérité et que 4.5 ne doit pas rester.
- `supervise status` : `calls` par minute attendu autour de 74 (garde) + transitions, au lieu de ~440 ; `silent` proche de 0 hors redémarrage.
- `cpu-profile threads` : groupes `push-read` (74 fils, quasi nul), `brain-clock`, `brain-tick`, `worker-connect` (quelques fils) ; `total` du démon non supérieur à la référence.
- `idle_bot` : pas d'augmentation attendue ; ticks de travail, rapports/h, XP/h, erreurs par type : au moins la référence. Une régression de ces mesures, avec les compteurs ci-dessus, désigne le lot à corriger.

## Frontières typées (phase 5, 2026-11-24)

Le JSON reste sur le fil ; en mémoire, chaque protocole a une énumération et un seul module de codage (analyse 2.2 d). Lots livrés ici : 5.1, 5.2, 5.3, 5.10 (voir `docs/decisions.md` pour les preuves).

### Phase et statut d'un job (lot 5.1)

`orch-fleet::job_state` : `Phase` (`Idle`, `Running`, `Failed`) et `Status` (`Starting`, `Idle`, `Running`, `Busy`, `Finished`, `Failed`, `Blocked`, `Stopped`, `Reconnecting`, `Disconnected`), chacune avec `Other(String)` pour un mot inconnu (un worker plus récent) : `parse` ne rate jamais et `as_str` redonne le texte reçu, donc le texte du fil, du tableau de bord et des fichiers d'état est inchangé. `JobInner.phase/status`, `FrameView`, `LastPoll`, `watchdog_verdict` et les lectures du cerveau (`brain_idle`, `job_idle`) comparent des énumérations, plus des chaînes. L'état de session du ouvrier (`orch-session`, son propre `Inner`) garde ses chaînes : c'est un autre processus, son changement attend un `redeploy`.

### Trames de battement typées (lot 5.2)

`orch-fleet::frame_codec` : `Frame::{Snapshot, Light, TaskReport, Event}` (`SnapshotFrame`, `LightFrame`, `ReportFrame`, `EventFrame`), un seul module de codage. Le démon décode la ligne UNE fois à sa porte (`Orchestrator::ingest_push_owned` / `ingest_push_and_wake` → `Frame::decode`, l'arbre d'un instantané est DÉPLACÉ hors de la ligne, jamais copié) puis `ingest_frame` ne manipule que des types : `Frame::session_facts()` (phase, statut, heure d'envoi) alimente `Job::note_frame`, `LightFrame::merge_into` remplace `merge_light` sur le chemin du démon, `EventFrame` va à `errors_ingest::observe_event`. Les erreurs de décodage sont typées (`FrameError::{NotAnObject, UnknownType}`) et gardent les mêmes réponses (`account is required`, `no session for X`, `unknown push type "x"`, dans le même ordre). L'ouvrier écrit par le même module (`snapshot_frame`, `light_value`, `event_frame`, `push_task_report` construisent la trame typée et l'encodent). Restent en `Value` : le rapport de tâche (document du planificateur, lu par son propre lecteur), `merge_light`/`light_value` côté web (`web_ws`, son propre cache) et le test `vital` de `push_conn` (clé `type` seulement).

### Commandes ouvrier et erreurs d'appel typées, table des commandes (lot 5.3)

- `orch-fleet::worker_call` : `CallError::{Unreachable, TimedOut, Protocol, Refused(Refusal)}` (`Injoignable`, `Délai`, `Protocole`, `Refus`) et `Refusal::{Busy, NoSession, Other}`. `Fleet::send_worker` rend `Result<Value, CallError>` ; le texte est celui d'avant (`Display`, `text()`), donc journaux et clé `error` des réponses ne changent pas. `Refusal::parse(texte)` lit le texte d'un refus (`busy: ...` devient `Busy`, `no session ...` / `... died, open ...` `NoSession`) et `never_ran()` remplace `refusal_never_ran` côté cerveau (P2-5 : un lancement refusé n'a pas tourné, le noeud revient prêt sans échec compté ; une erreur de transport ne prouve rien). `CallError::is_socket_stale()` porte la condition « socket abandonnée » de la reprise des ouvriers.
- `orch-fleet::worker_command` : `WorkerCommand::{Ping, Status, Run, Unload, Pause, Resume, Hold, Close, Settle, Call, Methods, PoseMarket}` ; `encode` est le seul endroit qui écrit les clés de ces requêtes, `decode` le seul qui les relit. La boucle de commandes de l'ouvrier lit toujours ses clés comme avant : elle passe par `decode` avec le canal durable (lot 5.9, `redeploy`).
- `orch-daemon::daemon_commands` : la commande de la socket du démon est un `OperatorCommand` (28 mots + `Unknown`), lu une fois par `OperatorCommand::parse` ; `Orchestrator::dispatch` n'a plus qu'un bras par commande (de 235 à ~90 lignes), chaque bras appelant son propre traitement (`status_reply`, `job_list_reply`, `accounts_reply`, `show_reply`, `live_reply`, `logs_reply`, `rares_reply`). Le fil reste `{"cmd": ...}` : CLI et tableau de bord inchangés.

### Configuration typée, lue une fois (lot 5.10)

`orch-core::config::Config` : `LIFESCAPE_SERVER_HOST` (`127.0.0.1`), `LIFESCAPE_SERVER_PORT` (`43594`), `LIFESCAPE_WEB_PORT` (`8765`), `LIFESCAPE_REDRAW_GATE` (`shadow`, `RedrawGate::{Off, Shadow, On}`), `LIFESCAPE_RATES_PATH` (défaut `run/rates-snapshot.json`). `main` la lit UNE fois (`Config::from_env`), l'imprime (`startup_config::lines` : une ligne `config NOM=valeur (env|default)` par réglage, puis les délais et cadences qui sont des constantes du code, lus sur les constantes elles-mêmes : appel d'ouvrier 40 s, ouverture 150 s, tick de sécurité du cerveau 10 s, silence 5 s / filet 60 s / écart de transition 2 s / écart de silence 5 s, chien de garde 45 s / fenêtre 300 s / 3 relances, tick serveur mesuré en direct) et la passe à ce qu'elle règle : `web::start_web(.., config.web_port)`, `brain_redraw_gate::install(config.redraw_gate)`, `rates_service::install_snapshot_path(config.rates_path)`, `Orchestrator::with_config` (le démon donne `LIFESCAPE_SERVER_HOST/PORT` aux ouvriers qu'il lance ; l'ouvrier lit sa configuration une fois à son démarrage, `session::run`). Une valeur qui ne se lit pas retombe sur son défaut, comme les lectures éparses d'avant. Hors de la structure, volontairement : les chemins (`LIFESCAPE_ROOT`, `LIFESCAPE_ENGINE_DIR`, `LIFESCAPE_SAVE_DIR`, lus par des outils sans démon), la trace des paquets (`LIFESCAPE_PKTLOG`, interrupteur du SDK) et les variables de débogage des outils de construction.

### Étape C : `Autopilot`, le démon et le web ne dépendent plus du cerveau (lot 5.4, 2026-11-24)

`orch-fleet::autopilot` : le trait `Autopilot` (corps inertes par défaut, `NoAutopilot` tant que rien n'est enregistré), implémenté par `orch-brain::autopilot_impl::BrainAutopilot` et enregistré par `main` avant tout fil (`autopilot_impl::install()` puis `autopilot::assert_registered()`, à côté de `register_log_sink` et `register_account_facts`). Le démon et le web appellent `autopilot::current().<méthode>`. Méthodes : `on_supervise`, `on_report_push` (superviseur et porte des rapports), `note_operator_stop` (`unload`, `close`), `request` (commande `ambition`), `explain_for` (`bid-why`), `wishes_reply` (onglet Cerveau), `brain_view` (`/api/brain/<compte>`), `life_line` (but et tâche de la liste des bots), `maker_rows` (fiche des teneurs), `fund_makers`, `rares_lines`, et les crochets du collecteur d'erreurs (`remember_line`, `is_collected_brain_line`, `observe_no_route`). `funding_cmd`, `funding_plan`, `rares_cmd` (qui lisent les vies et le matériel) et `web_brain`, `web_brain_view`, `web_job_lives` (qui lisent la vie du bot) sont passés dans `orch-brain` ; `fund_makers` prend un `&dyn Fleet`. Contrôle : `scratchpad/an/layering_check.py` (affectation des crates mise à jour, plus un contrôle des dépendances déclarées dans les `Cargo.toml` : 0 violation, ni `orch-daemon` ni `orch-web` ne déclarent `orch-brain`). Recompilation après une ligne modifiée dans `brain_alch.rs` (`cargo test --no-run --workspace`, `release-test`) : 7,8 s (brain, web, daemon, orchestrator) avant, 4,5 s (brain, orchestrator) après. Les tests du démon et du web qui n'ont pas besoin d'un cerveau tournent sans lui (`NoAutopilot`) ; deux lignes de test passent par un double (`errors_ingest`, `web_lives`) et le vrai chemin est prouvé côté cerveau (`the_maker_life_file_drives_the_card`).

### Étape D : `orch-stats-model` et services (lot 5.5, 2026-11-24)

`orch-stats` est coupée : le modèle pur (liste ci-dessus) passe dans `orch-stats-model` ; `orch-stats` garde les services (service des rendements `rates_service` et ses lectures/écritures, `guides_*`, `holdings*`, `hiscores*`, `xp_*`, `time_value`, `trade_report`, `economy_outcomes`, `live_items`) et ré-exporte les modules du modèle à leurs anciens chemins (`crate::flow_causes::x` inchangé partout ailleurs). `rates_parse` devient un module de plein droit (il était un sous-module de `rates_service`) et porte les trois constantes que le modèle partage (`NORMALIZE_TICKS`, `LEVEL_BAND`, `MIN_MEASURED_REPORTS`) ; `rates_service` les ré-exporte. `orch-session` ne dépend plus d'`orch-stats` : il appelle `rates_parse::{parse_report, is_trading_method}` au lieu de `rates_service::`. Le contrôle des couches (`layering_check.py`) place `orch-stats-model` entre le marché et les statistiques et refuse `orch-session` -> `orch-stats`. Les tests du modèle déménagent avec lui (55 tests ; `orch-stats` en garde 90).

### Écrivain unique de la base, écritures groupées (lot 5.8, 2026-11-24)

`orch-core::store_writer` (décision 1 : `synchronous=FULL` inchangé, on regroupe seulement). Une connexion écrit (`StoreWriter`, ouverte par `MarketService::open`, enregistrée par `market_daemon::start` comme l'écrivain du processus), un fil `store-writer` valide, les lecteurs gardent leurs connexions (`store_pool`, une par fil) et n'attendent plus jamais une écriture. Avant : `Mutex<Store>` du marché, toute lecture et toute écriture dessus, chaque écriture validée (une synchronisation du disque) verrou tenu.

- `MarketService::lock_store()` donne la connexion écrivain (`WriteGuard`) : un seul porteur à la fois, donc un bloc « lire puis écrire » sous un même garde reste atomique face aux autres écrivains. Au `drop`, le garde n'écrit pas sur le disque lui-même : il rejoint le GROUPE courant (une transaction `BEGIN IMMEDIATE` ouverte par le premier écrivain), demande la validation au fil `store-writer` et ATTEND qu'elle ait eu lieu. Le fil valide tout le groupe d'un coup (un seul `COMMIT`, une seule synchronisation) puis réveille tous les écrivains en attente. Les groupes se forment dès que deux écrivains se chevauchent ; un écrivain seul valide aussitôt.
- `MarketService::read_store()` donne la connexion de lecture du fil (pool). Soixante-six lectures du cerveau, du marché et des statistiques y sont passées (script de classement : aucune méthode d'écriture du `Store` n'y est appelée). Dans une `batch()` du même fil, ou si le pool ne s'ouvre pas, `read_store()` rend la connexion écrivain (ses propres lignes non validées restent lisibles).
- Écriture acquittée = validée ou l'appelant est prévenu : le garde ne rend la main qu'après le `COMMIT` ; un `COMMIT` en échec annule tout le groupe, incrémente `failed_commits()`, journalise `store-writer: commit failed, N write(s) lost` et rend l'erreur aux gardes qui l'attendaient (`WriteGuard::commit()` la renvoie, le `drop` la journalise : `store-writer: a write was not committed`). Aucun accusé de réception avant le disque.
- Lecture de sa propre écriture : après l'accusé, toute connexion (le pool compris) voit la ligne ; dans le garde, l'écrivain voit ses lignes non validées (même connexion). Les sites qui lisent après avoir écrit sont donc sans surprise ; les seuls cas à traiter étaient la passe de reprix (une `begin_batch` tenue sur plusieurs gardes, remplacée par `store_writer().batch()` : les gardes d'une passe n'attendent pas un à un, le lot valide et attend une fois à sa fin, ses lectures passent par l'écrivain) et `continuous_candles` (peut reconstruire, `&mut`, reste sur l'écrivain).
- Imbrication : les écritures du `Store` qui ouvraient leur propre transaction (`fail_stale_deals_relisting`, `record_holdings`, `purge_expired_orders`, `rebuild_candles`, `rebuild_all_candles`, `record_errors`, `in_batch`) passent par `open_scope`/`close_scope` : transaction si aucune n'est ouverte, sinon point de sauvegarde dans le groupe (un échec n'annule que l'appel).
- Les autres écrivains du démon passent par le même écrivain quand il est enregistré (`store_writer::write_to`, sinon connexion du pool comme avant) : rapports de tâche (`rates_ingest::ingest_report_persisted`), échantillonneur d'avoirs, collecteur d'erreurs (`through_writer`, qui remplace sa connexion à part), rétention (un groupe court par lot de 2 000 ordres, l'écrivain libre pendant les pauses).
- Rien d'autre que de la base sous le verrou de l'écrivain (correction du lot 5.8, 2026-11-24). Le collecteur d'erreurs scannait les dossiers d'échec (des milliers de fichiers : liste, stat, lecture, analyse, crochet du pilote) verrou tenu : le marché ne pouvait pas écrire ordres ni échanges. Maintenant `scan_dossiers` travaille sans verrou et ne prend l'écrivain que deux fois, par le trait `StoreAccess` (horloge de départ, puis insertion de tous les lots d'un coup) ; `flush` est coupé en `prepare` (lignes, contexte vivant du job : pur) et `apply` (insertions et repli des rapports). `ingest_report_persisted` analyse le rapport et son contrat avant le verrou (une fois, plus à chaque nouvelle tentative) ; `op_history` ne garde l'écrivain que pour `continuous_candles` (peut reconstruire) et lit exécutions et parties sur une connexion du pool, JSON construit hors verrou. Preuve : `a_write_proceeds_while_a_dossier_scan_is_in_progress` (un « dossier » est un tube nommé dont la lecture bloque : le balayage est suspendu dans une lecture, une écriture du marché passe sous 5 s, puis le balayage repart et insère tout) ; condensat de relecture (`the_writer_leaves_the_same_rows_as_direct_writes`, `Store::table_digest`) inchangé.
- Écart à la lettre du plan : ce n'est pas une file d'instructions vers un fil qui les exécute. Les instructions s'exécutent sur le fil appelant sous le verrou de l'écrivain (elles empruntent l'état du moteur, pas de `unsafe` ni de fermetures `'static` possibles) ; seule la VALIDATION est portée par un fil unique qui regroupe. Le gain visé (lectures libres, une synchronisation pour plusieurs écritures) est le même.
- Preuves : `the_writer_leaves_the_same_rows_as_direct_writes` (même séquence de comptes, ordres, échanges, exécutions, rapports et annulations : mêmes lignes dans toutes les tables par `Store::table_digest`, en direct, par l'écrivain, et dans une `batch`), `an_acknowledged_write_is_committed_and_visible_to_readers`, `concurrent_writers_are_grouped_and_every_row_lands` (8 fils x 25 écritures : 200 lignes sur disque), `a_batch_waits_once_at_its_end`, `a_guard_reads_its_own_write`.
- À regarder après déploiement : `store-writer:` dans le journal (aucune ligne attendue), temps système et attente disque de `brain-tick`, le CPU du fil `store-writer`.

### Canal ouvrier durable (lot 5.9, 2026-11-24)

Le canal des battements (`push.sock`, ouvrier -> démon) devient bidirectionnel : le démon y écrit ses commandes à l'ouvrier, l'ouvrier y répond. Plus de connexion ni de fil d'aide (`bounded_call`) par appel, plus d'attente de 100 ms (travail) à 1 s (repos) sur la boucle d'acceptation de l'ouvrier.

- Trames (`orch-fleet::frame_codec`) : le `hello` de l'ouvrier annonce `"durable": true` (un ouvrier ancien ne l'écrit pas : il garde le chemin par appel) ; commande `{"type":"cmd","id":N,"request":{..}}` (la requête est celle d'un appel par connexion, `WorkerCommand::encode`) ; réponse `{"type":"reply","id":N,"reply":{..}}` sur le même flux.
- Démon (`push_hub`) : `PushHub::call` écrit la commande sur le flux durable du compte (un seul écrivain à la fois), attend la réponse portée par son `id` (appels concurrents d'un même compte séparés), `durable_call` est le point d'entrée de `Orchestrator::send_worker`. Les réponses ne passent pas par la distribution des trames. Pas de flux durable pour le compte, ou écriture impossible : `Unavailable`, rien n'est parti, `send_worker_per_call` répond comme avant (connexion, délai de connexion borné, mort de l'ouvrier sur connexion bloquée, nettoyage du socket périmé : inchangés). Commande partie sans réponse à temps : `TimedOut` (même texte qu'avant), JAMAIS rejouée par le chemin par appel (la commande a pu s'exécuter). Connexion terminée sans réponse : réponse vide, comme une connexion par appel fermée sans réponse (`close`, `unload` : l'ouvrier peut partir avant d'écrire).
- Ouvrier (`orch-session`) : `push_conn::enable_commands` avant le premier battement (le `hello` dit `durable`), un fil bloqué en lecture (`session-commands`) met les commandes dans une boîte ; la boucle de service (`session::serve`) les prend à chaque tour comme elle prenait une connexion (`serve_durable` : un fil par commande, réponse écrite sur le flux tenu) et son repos est coupé net par l'arrivée d'une commande (`push_conn::rest`, qui remplace le `sleep`). Une boucle bloquée ne répond ni aux commandes durables ni aux connexions par appel : les délais d'appel du démon disent toujours un ouvrier figé d'un ouvrier sain. Le dispatch lit une commande typée (`WorkerCommand::decode`, `session_cmds::dispatch_typed` ; `set_locks`, hors commandes opérateur, et les mots inconnus gardent la lecture par clés). La boucle d'acceptation par appel reste en place (repli).
- Chaque connexion du flux est reprise par la suivante (un ouvrier qui se reconnecte, un démon relancé : l'ouvrier recompose le flux à son prochain battement, jusque-là `Unavailable` et chemin par appel) ; la fin d'une connexion échoue ses appels en attente.
- Preuves : `a_durable_call_gets_its_own_reply` (8 appels concurrents, chacun sa réponse, rien dans le flux des trames), `without_a_durable_connection_the_call_is_unavailable` (aucun octet écrit à un ouvrier ancien), `a_silent_worker_times_out_and_a_lost_connection_fails_the_call` (ouvrier figé : délai ; ouvrier tué : fin immédiate puis flux retiré et chemin par appel), `a_reconnected_worker_answers_on_its_new_stream`, `commands_read_off_the_stream_wake_the_serve_loop` (côté ouvrier : lecture, boîte, repos coupé), `the_durable_channel_frames_round_trip`.
- DÉPLOIEMENT : les ouvriers changent (`orch-session`, `frame_codec`) : à redéployer compte par compte (`redeploy`) après le démon ; avant leur redéploiement ils gardent le chemin par appel (leur `hello` ne dit pas `durable`). Un démon ancien avec des ouvriers nouveaux : l'ouvrier écrit un `hello` avec une clé inconnue, que l'ancien démon ignore, et ne reçoit jamais de commande sur le flux (la boucle d'acceptation sert encore).
- À regarder après déploiement : appels `status`/`job_list`/`run` plus rapides (le délai moyen d'un `send_worker` ne contient plus l'attente de la boucle d'acceptation), fils `worker-connect`/`session-conn` du démon en baisse (groupe `exited` du profil), aucun `session unreachable` nouveau.

### Phase 6 : ouvriers (2026-11-24)

- 6.1 Trace sans copie par battement. `publish_tick` (crochet de la pompe, un appel par tick) copiait toute la trace de déplacement (2 000 à 4 000 positions, 93 % d'un instantané) sous le verrou à chaque appel pour ne la lire qu'à un instantané complet (tous les 100 ticks ou 30 s). Maintenant `pump_heartbeat` la lit dans la session seulement dans la branche complète ; le contenu de l'instantané poussé est le même (même trace, même instant, `heartbeat_value` inchangée), le démon, le web et le cerveau ne voient aucune différence. Mesure (`bench_publish_tick_with_a_long_trail`, trace de 3 000 positions, aucun battement dû) : 1 675 à 1 814 ns par battement avant, 178 à 200 ns après ; copie évitée : 72 Ko et une allocation par battement (24 octets par position), soit 100 copies au lieu d'une par instantané complet. Les deux verrous successifs de `last_heartbeat_*` sont fusionnés en un.
- 6.2 Caches reconstruits au changement. `World::inv_rev` : un numéro unique pris à chaque changement des inventaires (`touch_inv` dans `inv_full`, `inv_partial`, arrêt de transmission, graine de la banque, reconnexion, écrans d'échange ; un `World` neuf ne répète jamais un numéro). Le code qui édite `inv` directement (tests) appelle `touch_inv`. (a) Cache de banque : `refresh_bank_cache_on_change` ne reconstruit que si la révision a bougé depuis la dernière construction (`Shared::last_bank_rev`) ; banque inchangée, rien n'est reconstruit, et si le gel du point de départ d'une exécution est encore à faire (`need_live`), la banque en flux est relue dans le cache. Le chemin de fin d'exécution garde la reconstruction complète. (b) `observe_ledgers` : la révision et les pièces portées du dernier passage complet sont gardées (`ledger_seen`) ; inventaires inchangés, ni le sac, ni la table des objets portés, ni la somme des pièces ne sont refaits, seules tournent les écritures qui dépendent du tick (`ItemLedger::observe_same_pack`, `CoinLedger::observe` sur les pièces gardées, événements de course, XP) ; `ItemLedger::observe` ne copie plus ni ne diffère un sac égal au précédent. Preuves : `ledgers_are_identical_with_and_without_the_unchanged_shortcut` (40 pas, flux de pièces, d'objets, événements, compétences identiques au passage complet à chaque appel), `bank_cache_on_change_matches_the_full_refresh` (ouverture, modification, fermeture, vidage : cache et réponse identiques à la reconstruction complète). Mesures (bancs `--ignored`) : banque fermée à 256 objets, 18 381 ns par battement avant, 202 ns après ; `observe_ledgers` sans changement (sac de 28, 12 portés, 25 compétences), 4 139 ns par trame avant, 1 007 ns après.
- 6.3 Lectures de la prise par blocs. `Connection::read_exact` lisait octet par octet utile (opcode, longueur, corps : trois lectures et deux allocations par trame) et `available()` basculait la prise en non bloquant puis en bloquant à chaque contrôle (trois appels système, une fois par trame du drainage). Maintenant : une lecture prend ce que la prise contient (jusqu'à 64 Kio, tampon réutilisé) et le tampon est servi par un curseur (`buffer_head`, aucun décalage) ; `available()` ne touche la prise que si le tampon est vide, et la laisse en non bloquant ; une lecture qui doit attendre (rien en attente) et toute écriture remettent le mode bloquant, donc le mode ne change que quand il change vraiment (`Connection::nonblocking`). Les erreurs, les messages, le délai de lecture de 500 ms, le keepalive forcé et la reprise « tout ou rien » d'une lecture expirée sont inchangés. Le décodage reste indépendant du découpage : `decoding_is_independent_of_segmentation` et son jumeau `pumped_decoding_is_independent_of_segmentation` (recv puis available/recv comme la pompe, 7 découpages), plus `a_send_after_available_still_goes_through`. Mesure (`bench_available_and_burst`, boucle locale) : `available()` sur prise calme 1 956 à 2 289 ns par appel avant, 325 à 459 ns après (3 appels système contre 1) ; rafale de 48 trames consommée comme la pompe : 71 à 90 µs avant, 49 à 65 µs après. Appels système par trame d'une rafale : 6 avant (3 lectures et 3 de `available`), de l'ordre de 4 par pompe après quelle que soit la taille de la rafale (une lecture de bloc, une sonde).
