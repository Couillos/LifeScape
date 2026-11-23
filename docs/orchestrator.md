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
parchemin ; aucune image du jeu) : colonne de catégories (Overall + 19
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
| `core` | `logln!` + `log_sink`, `paths`, `clock`, `client`, `log_markers`, `cpu_profile`, `res_stats`, `store_pool`, `store_queue`, `names`, `api_shapes`, `economy_guard`, journaux |
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
| `orch-core` (`lifescape-orch-core`) | `logln!`, `daemon_clock`, `log_sink`, `paths`, `clock`, `client`, `log_markers`, `cpu_profile`, `error_stamp`, `log_rotate`, `disk_guard`, `res_stats`, `store_pool`, `store_queue`, `names`, `api_shapes`, `nav_profile`, `economy_guard` | json, store, nav |
| `orch-world` | `png`, `mapcache`, `mapbuild`, `minimap`, `staticmap`, `zones`, `plan_service` | core |
| `orch-fleet` | `jobs`, `fleet` (trait `Fleet`), `registry`, `site_crowd`, `worker_owner`, `daemon_pool` | core |
| `orch-market` | moteur du marché, `funding_gifts`, `market_host`, `market_facts`, `market_own` (mémoire du vendeur sur le moteur), `market_test_support` | core, world, fleet |
| `orch-stats` | `rates_*` (dont `web_rates`, sous-module de `rates_service`), `report_*`, `flow_causes`, `time_value`, `xp_*`, `hiscores*`, `holdings*`, `live_items`, `guides_store`, `guides_model`, `rates_ingest`, `errors_model`, `errors_aggregate`, `trade_report`, `economy_outcomes` | core, world, fleet, market |

| `orch-session` | worker `session*`, `push_conn`, `heartbeat_flows` (pompe du battement) | core, world, fleet, market, stats |
| `orch-brain` | `brain*`, agents de marché (`market_maker` avec `register_account_facts`, `market_seller`, `market_demand`, `market_discovery`, `market_maker_report`), `bid_why`, `method_health`, bancs `par_harness`; `tests/fixtures/lives` | core, world, fleet, market, stats |
| `orch-web` | `web*`, `web_rates_view`, `hiscores_web_tests` | + brain, session (`heartbeat_flows`) |
| `orch-daemon` | `daemon*`, `supervise`, `push_hub`, `errors_ingest`, `rares_cmd`, `funding_cmd`, `funding_plan`; porte `Orchestrator` et `impl Fleet` | + session, brain, web |
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
