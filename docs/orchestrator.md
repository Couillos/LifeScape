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
Tableau Skills du spectateur (2026-11-16) : lignes heartbeat `skills`
`{id, level, base, xp}` (`level` = niveau actuel boosté/drainé, `base` =
niveau de l'xp, `session_heartbeat.rs`) ; ligne « Total » en bas (option
`footer` de `TableSort`, jamais triée) = somme des `base` et des xp.
Périodes des graphiques `/items` et `/market` (2026-11-15) : 24 h et
7 jours en pas d'1 h (`res=1h`, 24/168 créneaux), 30 jours et 6 mois en
pas d'1 jour (`res=1d`, 30/183 créneaux) ; tables `PERIODS` (`items.js`)
et `VIEWS` (`market.js`), l'historique plus court s'affiche tel quel.
Traçabilité `/items` (2026-11-15) : `GET /api/items/flows?item=<obj id |
nom affiché>&hours=24` (`src/web_items.rs`, `Store::item_flows_since`,
staff et `run/economy-excluded.txt` exclus ; sans `item`, les 60 plus
gros flux tous objets ; même source que `orchestrator economy [h] item`)
alimente sous le graphique de l'objet choisi le tableau « Provenance /
destination » (provenance en clair, quantité signée + entrée / − sortie,
rapports, ligne nette) sur la période des boutons (24/168/720/4380 h) ;
sans données, le tableau reste vide (aucun message).
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
Source en échec ⇒ ligne dans `errors`, jamais une liste vide muette.
`version` = mtime du fichier de vie + empreinte du contenu affiché (hors
comptes à rebours) : le front relit toutes les 5 s et ne redessine le
graphe SVG (colonnes ambition → jalons → but → tâches par plus long
chemin de dépendance) que si elle change. Tests :
`web_brain_tests.rs`, `node orchestration/static/brain.test.js`.
Serveur de jeu via `LIFESCAPE_SERVER_HOST` / `LIFESCAPE_SERVER_PORT`
(défaut `127.0.0.1:43594`), données engine via `LIFESCAPE_ENGINE_DIR`
(défaut `../lostcity/engine`).

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
