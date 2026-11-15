# Économie (lot L5 : market + simulateur)

But : prix issus de l'offre et de la demande entre bots, rendements mesurés
en continu, visibles au dashboard et dans une base interrogeable.

## Market

Double enchère continue, un carnet par objet échangeable F2P : priorité prix
puis ancienneté, exécutions partielles au prix de l'ordre au carnet, modifier
= annuler + replacer (perte d'ancienneté). Carnet jamais croisé (sauf
un compte contre lui-même, et la paire d'un deal échoué pendant son
refroidissement de 3000 ticks) : tout ordre qui (ré)entre — placement,
amend, relist après échec, réinsertion au boot — est TAKER et paie le
prix de l'ordre déjà au repos ; `market_uncross` guérit le reste (boot,
puis chaque tour de convocation). Code pur dans
`rs/crates/market/` : `book` (carnet + appariement), `guards` (prix ≥ 1 +
couverture), `deals` (machine à états), `price` (prix courant = médiane des 5
derniers fills bornée par les prix d'impact du carnet, bougies OHLCV), `trends` (signaux publics), `zip` (cotation + plafonds
marchands), `sim` (simulateur à graines fixes).

Stratégie de cotation inspirée de ZIP (PRD3, demande 2026-11-13) : un
ordre posé avec un `ceiling` (acheteur) ou un `floor` (vendeur) fait un
pas de 5 % (≥ 1 pièce) par fenêtre de patience (2000 ticks depuis le
2026-11-14, `zip::PATIENCE_TICKS`, voir § Coût CPU du repricing) tant qu'il
reste au carnet (`market_reprice`, `lifescape_market::demand_step`), sa
direction lue dans la fenêtre : vente servie (même en partie) → monte,
vente au repos → descend jusqu'à son plancher, achat au repos → monte
jusqu'à son plafond, achat servi → redescend vers son prix d'ouverture.
Acheteur : ouvre à l'ask s'il le couvre, sinon à son propre dernier
prix d'achat de l'objet, sinon au prix courant (`remembered_bid`),
plafond = ce que son portefeuille paie (plus de plafond « juste »).
Vendeur : plancher = sa réserve, son PROPRE coût (§ Réserve vendeur),
ouverture à la référence échangée (ou au meilleur bid plus riche), à
`max(meilleur bid, 2 × réserve)` pour un objet jamais échangé, jamais
sous la réserve (`seller_open`). Une vente monte sans plafond propre
(seule la demande l'arrête). Voir § Prix par l'offre
et la demande et § Réserve vendeur. Les PNJ n'ancrent plus rien : aucun bot ne
vend à un marchand, et l'achat marchand est réservé aux objets non
productibles en F2P (D-A, voir `decisions.md`). Marchands (`market:flip`, ~10 %) :
sélection sur écart/volume, plafonds `max_capital_share` (10 %),
`max_stock_items` (500), `max_book_share` (25 % du côté).

Couverture vérifiée à l'ordre (objet possédé, pièces disponibles, registre
en mémoire ; les holdings live des connectés se brancheront sur le même
contrôle). Aucune bande de prix (décision utilisateur 2026-11-15 : l'offre et la
demande seules font les prix) : `guards::check_price` refuse seulement un
prix < 1 ; plus de plafond 5× ni de plancher 0,2× autour de la référence,
ni au placement, ni à l'amend, ni dans la marche (`market_reprice`) ou
l'ouverture vendeur. Deals livrés physiquement place
de Varrock (anneau autour de la fontaine, voir § Place ci-dessous) :
`matched → convened(case d'échange, échéance) → both_arrived →
trading → settled | failed{no_show|mismatch|death|timeout|nav_down}`.
Échéance = 2× trajet ETA max + marge (ticks) ; départs échelonnés par
ETA nav (cache par zone de départ, un plan par zone) pour une arrivée
simultanée ; regroupement 300 ticks (une convocation par bot, paires
regroupées en un seul échange) ; une convocation = case + heure de
départ seulement (jamais la position de l'autre). Les ticks sont des
époques par login : la convocation porte des décalages
(`deliver +<départ> +<échéance>`, `market_convene::deliver_args`), que le
worker ancre sur sa propre horloge à la réception
(`market_handover::anchor_deliver_args`) ; un `+N` non ancré est refusé
(`Usage`). Le balayage des expirés suit l'horloge murale du daemon
(mesure live du tick, sans interrupteur dev/prod). Le registre règle à
l'appariement ; un `failed` ultérieur rembourse le transfert, replace
la partie présente **avec sa priorité** (seq d'origine conservée) et ne
touche la fiabilité qu'en cas de faute (absent, refuseur ; mort,
timeout, nav_down : neutre). Sans nav : aucune convocation,
appariements conservés, expirations en `nav_down` sans pénalité.

Tout deal se termine (2026-11-13) : `settled` seulement sur le message
serveur « Accepted trade. » (fenêtre fermée sans lui = refus/départ du
partenaire, `ExchangeOutcome::Declined`) ; un worker qui meurt après
`trading` rapporte quand même (acceptation vue → settled, sinon
`timeout`) et le balayage expire aussi `trading` ; un compte reste
convoqué tant qu'un deal de sa convocation est en vol ; chaque côté
interroge le daemon toutes les 100 ticks (un partenaire `short` libère
l'autre aussitôt). Partie absente d'un deal `matched` (aucune session) :
le daemon l'ouvre (`open_job`, admission et cadence respectées, une à la
fois, `market_absent.rs`) ; compte staff/privé, ouverture refusée ou
absence > 3000 ticks → `no_show`, ordres de l'absent annulés, côté
honnête replacé avec sa priorité.

## Place de Varrock (échange physique)

Centre : fontaine (3213, 3429, niveau 0), bassin x 3211-3215 / z 3427-3431.
Anneau calculé au démarrage du market service via l'op `tiles_flags`
(cases marchables autour du bassin, hors bustes (3210,3429), (3213,3426),
(3213,3432), (3216,3429) et étals (3220,3425), (3220,3431)) ; banques
`lifescape_gamedata::banks` (Varrock ouest x 3186 / z 3436-3446, Varrock
est z 3419 / x 3252-3256). Chaque deal reçoit une paire de cases
adjacentes libre, répartie autour de la fontaine ; 2e anneau si plein,
puis file d'attente. Rendez-vous : attente jusqu'à l'échéance, le plus
petit id de compte envoie la demande. Échange SDK `trade_exchange` :
billets (`bank_main:com_93` = notes, `com_94` = items) pour les gros
lots, redépôt après l'échange, les DEUX écrans vérifiés avant accept.

Référence (PRD3, remplacée le 2026-11-14 par le prix courant, § Prix
courant) = VWAP 24 h ; sinon dernier print ; sinon milieu du
carnet ; sinon **1 pièce** (jamais le prix magasin). Ce repli public
reste honnête (« pas de donnée ») mais aucun vendeur n'en part : un
objet non échangé (référence non `traded`) ouvre à `2 × réserve`. Bougies OHLCV en horloge (1 h / 7 jours,
1 jour / 6 mois et plus), recalculables depuis `executions` (source de
vérité), tick serveur stocké avec chaque exécution. Tendances calculées
uniquement sur données publiques (profondeur du carnet + exécutions :
pénurie, momentum, score lissé borné) ; aucun état d'un autre bot n'y
entre jamais.

Service daemon `orchestrator/src/market_service.rs` (+ `market_seed.rs` :
catalogue F2P + historique de démonstration) : socket `run/market.sock`
(JSON-lines, une requête une réponse), API `quote/book/history/trades/
list/place/amend/cancel/my_orders/my_deals/deal_arrived/deal_report/fund/
tick/stats` ; persistance via `lifescape-store` (ouvert par le daemon
seul). Page `/market` (2026-11-13) : liste `list?active=1` = biens avec
au moins un ordre ouvert OU au moins une exécution passée (pas de
plafond ; carnet vide jamais échangé caché) ; `recent` porte le `name`
de chaque deal, le flux et le titre ne dépendent plus de la liste. Les
ETA de convocation passent par le service de navigation (un plan par
zone de départ, `market_eta`).

Échecs de remise (revue 2026-11-13, deals 1100–1285) :
- `timeout` dominant : la pause market tourne DANS la tâche en cours
  (souvent une fenêtre d'argent de 1500 ticks) et le crochet de budget
  comptait la pause en vol comme du travail → `interrupted: run stopped`
  au milieu de la remise, souvent sur la case. La pause est facturée dès
  son premier tick (`Bot::begin_pause` / `pause_ticks_now`, `budget_due`).
- Convocation calée sur l'instantané d'un login précédent (worker rouvert
  à t=1, départ 61826) : corrigé par les décalages ci-dessus.
- `mismatch` à tort (1116, 1124) : une relance (marche d'approche ou
  `op_player`) croise la fenêtre que le partenaire vient d'ouvrir et la
  ferme (« Other player declined trade. »), les deux côtés lisaient un
  désaccord. `trade_request` relit le flux avant tout clic ; une fenêtre
  fermée avant les lots est `ExchangeOutcome::WindowClosed` /
  `SellerOffer::Closed` : la paire se retrouve (`MEET_RETRIES` = 3) tant
  que le daemon garde le deal.
- Restent hors marché : « nearest bank did not open » (marche vers la
  banque refusée par une porte/un loc, nav) et paires adjacentes qui ne
  se voient jamais (`cannot see`, liste joueurs SDK ; la ligne journalise
  désormais `debug_players`).
Échecs de remise (revue 2026-11-13 soir, deals 1643–1799, après le
déploiement de 21:19 CET) :
- `short` = lot promis deux fois. Un ordre entièrement apparié ne
  laisse aucune ligne au repos, mais ses biens (ou ses pièces) restent en
  banque jusqu'à la remise ; le registre les ré-miroitait depuis le
  snapshot à chaque tour, le cerveau les relistait et la relance
  s'appariait aussitôt (bct5rxpg 1716 + 1722 pour 12 flèches,
  fiona_park 1698 + 1721, 1692 + 1720) ; même chose côté pièces avec les
  offres permanentes reposées après un balayage complet (bd23dqpq « 98/98 »
  puis « 97/98 », 1643–1652 ; bct5rxpg 1772, 1789). Corrigé : le miroir
  du registre est net des deals en vol (`market_audit::net_of_deals`),
  `op_place` ne couvre que le non-engagé (registre moins les ordres au
  repos du même côté, `market_audit::uncommitted`) ; côté cerveau, le
  surplus est net des verrous (`brain_sales::free_stacks`), les pièces
  engagées comptent les deals acheteurs en vol (`brain_gear::deal_coins_from`)
  et l'offre permanente attend la remise d'un achat en vol
  (`brain_market::buying_in_flight`). Ni piles ni billets en cause : les
  quantités retirées en banque étaient exactes, la banque avait moins.
- `no_show` will_field (×7) : `spin` bloqué depuis 20:19 UTC. Le
  level-up a cassé la chaîne make-X avec de la laine restante ;
  `produce::drain_chained` (cible `u32::MAX` pour `want` 0) attendait
  sans fin, modale de chat ouverte, donc jamais de point sûr pour la
  pause market. Une chaîne qui ne consomme rien pendant `ITEM_TICKS`
  s'arrête (`produce::chain_stalled`).
- `no_show` t01 (1671/1672/1684) et cascade (1687 `timeout`, 1689
  `no_show` contre bd23dqpq pourtant présent) : un worker inactif pompe
  sur le thread de service (`idle_pump`) et la porte de pause y lançait
  la livraison en ligne, bloquant l'accept du socket pendant tout le
  trajet ; chaque pose du daemon expirait, la liste `unposed` renvoyait
  la convocation périmée (1664, déjà close) à chaque tour, qui écrasait
  la nouvelle et relançait 500 ticks de banque Falador ↔ Varrock.
  Corrigé : la porte ne s'ouvre qu'en présence d'un thread de script
  (`gate_tick`, sinon `drain_idle_market`) ; le daemon ne renvoie que la
  pose la plus récente par compte et seulement si un de ses deals est
  ouvert (`market_daemon::prune_unposed`) ; une livraison sans deal
  ouvert rend la main sans aller en banque.
- Cascade de groupes : un bot à plusieurs partenaires attendait un absent
  sur sa case pendant qu'un autre partenaire l'attendait, immobile, à
  quelques cases (bd23dqpq/nina_rhodes, 1689). Un partenaire hors de vue
  depuis `SERVE_FIRST_AFTER_TICKS` (100) cède le tour au premier groupe
  suivant dont le partenaire attend sur son point (`market_handover::
  serve_first`, au plus 2 détours par groupe, jamais une fois la fenêtre
  ouverte).
- `trade gave no harpoon` (bdr53jee) : la pause market dépose tout sauf
  les outils ; l'achat en magasin interrompu reprenait sans ses pièces
  (« You don't have enough coins »). La livraison rend désormais le sac
  d'avant la pause après les échanges (`market_lots::carried_before`,
  `redeposit`), formes billets comprises, sans jamais retirer deux fois
  ni sortir le verrouillé.
- Trajet coupé par une pause (bct5rxpg, bdr53jee, b5xntyz6 : « step
  refused »/porte jamais ouverte avec « last game: Accepted trade. ») :
  la livraison tourne dans le pump d'un segment en cours ; au retour la
  jambe périmée échouait, avec dossier et arête bannie. Une pause finie
  pendant le plan (`Bot::pauses_done`) explique l'échec : replan depuis
  la position, sans ban ni dossier, échéance recalculée
  (`travel::paused_mid_trip`).
Les holdings live ne sont pas encore branchées au registre.

## Simulateur

Graines fixes, agents ZIP synthétiques (offres/demandes tirées des
catalogues). Mesuré : convergence des prix à ±10 % de l'équilibre en 2
appariements sur le scénario symétrique (12 acheteurs / 12 vendeurs) ;
choc d'offre (+6 vendeurs au rabais) sans oscillation entretenue (16
inversions sur 120 appariements, prix final dans la bande) ; deux
marchands resserrés sur carnet clairsemé : 124 appariements contre 93
sans eux (+33 %).

## Rendements

Chaque fin de tâche produit une ligne par objet dans `task_reports` /
`task_yields` : `{script, méthode, objet, provenance, quantité, ticks,
contexte}`, où le contexte est le niveau du skill principal (bande de
5 niveaux), le tout horodaté en ticks. Seuls les flux de farm comptent
dans un rendement : produits de skill, drops, spawns fixes au sol et
objets ramassés, provenance étiquetée (`produced`, `loot`,
`ground_spawn`, `picked`). Les flux de boutique et d'échange
(`shop_bought`, `traded`) sont stockés bruts mais exclus des taux ; les
`consumed` (appâts, nourriture, intrants d'une transformation)
nourrissent le coût des intrants.

Provenance de production (2026-11-14, `orchestrator/src/report_flows.rs`) :
toute activité de transformation étiquette ses produits `produced` et
les intrants qu'elle a mangés `consumed`, lus dans les données du jeu
que le planificateur utilise (`method_way::production_ways`,
`obtain_legs::production_legs` + `map_method`), jamais une liste : un
objet gagné est produit quand une de ses voies de production a
travaillé pendant le run (XP de sa compétence montée, ou jambe du
planificateur lancée par le script même du run : meunerie, traite,
tonte, cueillette sans XP) et a mangé un de ses intrants (à travers les
intermédiaires du même run : argile → argile molle → pot cru → pot) ;
le brûlé est le sous-produit de sa recette ; les bûches brûlées, les
runes d'un sort payé et l'objet alché sont `consumed` ; les pièces de
l'alchimie sont `produced`. Live bst29bh9 `smith` avant : « Iron bar
lost −4, Iron full helm ground_spawn +2 » ; après : `consumed` / `produced`.
Les taux (`rates_service`, clé = script) apprennent donc les unités par
6000 ticks de chaque méthode de transformation (`brain_bids::units_per_6000`,
`bid_response`, `profit`), et le coût propre d'une unité fabriquée
(§ Réserve vendeur) compte ses intrants.

Ingestion : la session pousse le rapport, le superviseur du daemon le
verse dans le store (`jobs::ingest_report_into`) puis dans l'agrégat
persisté (`rates_service`, `run/rates-snapshot.json`, rechargé au
redémarrage). Écriture différée (2026-11-14, `rates_flush.rs`) : un
rapport marque l'agrégat « sale », le thread `rates-flush` réécrit le
fichier (tmp + rename) au plus une fois par minute, et l'arrêt propre
(`shutdown`) l'écrit avant de sortir. Un crash ou un SIGTERM (non
interceptable en std sans `unsafe`) perd au plus la dernière minute
d'agrégat ; les lignes brutes du store restent complètes. Avant : 790 Ko
réécrits à chaque rapport (~15 Mo/min, ~25 ms de CPU par rapport). Les lignes
brutes restent requêtables pour recouper plus tard.

Agrégation : rendement normalisé par 6000 ticks, par triplet (méthode,
objet, tranche de 5 niveaux), avec n, moyenne, écart-type (population)
et moyenne mobile exponentielle (alpha 0,3, ordre d'arrivée). Mêmes
mesures pour l'XP par 6000 ticks, qui dimensionne les durées de montée
de niveau. Sous 3 rapports, la page sert l'estimation de démarrage à
froid (tables de skills du jeu et pratiques de l'époque, marquée
`prior`) ; à 3 rapports et plus, la mesure remplace l'estimation.

Rentabilité d'une activité et d'une tranche : Σ rendement × prix de
vente estimé (VWAP 24 h, sinon dernier print, sinon meilleur bid au
repos, sinon 1 pièce ; `price_prior` tant qu'aucune transaction ne le
porte) − intrants consommés −
friction des pauses market (pauses mesurées par rapport, valorisées au
taux de l'activité : une pause ne rapporte rien) + bonus de tendance
(score public × chiffre × 0,1). Page `rates` : tableaux méthode × objet
× tranche, rentabilité courante, nombre de bots engagés par script
(information pour l'opérateur humain uniquement, jamais lue par les
bots).

## Tendances et marchands

Signaux publics (carnet, exécutions) : pénurie et momentum, score lissé
borné ; bonus de rentabilité pondéré, anti-oscillation par lissage, durées
aléatoires et profils. Marchands (`market:flip`, ~10 %) : poste fixe place
de Varrock, plafonds de risque, garde-fou 25 % du carnet.

## Front et base

Page `market` (prix au-dessus des bâtons de volume, 6 mois / jour et
24 h / heure, dernier point temps réel par le WS, carnet + dernières
exécutions ; canvas maison, zéro dépendance). La fenêtre demandée est
recalée sur les bornes des bougies (`web::snap_range`) pour que la
bougie en cours reste dedans. Bougies continues, sans trou (règle
utilisateur 2026-11-13) : chaque bougie ouvre à la clôture de la
précédente (haut/bas élargis à cette ouverture), une fenêtre sans
échange est une bougie plate grise à la dernière clôture (volume 0), de
la première exécution connue jusqu'à la fenêtre en cours. Le chaînage est
fait à la lecture côté serveur (`store::candle_chain`, op `history`,
`prev_close` = dernier prix avant la fenêtre) ; la table `candles` garde
les agrégats bruts par fenêtre. Le front rechaîne la queue live
(`LifeChart.carryFlat`) et ses créneaux finissent sur la fenêtre en cours
(`LifeChart.liveSlots`). L'anneau des prints (`trades`, `tick`, WS) est
rechargé depuis `executions` au démarrage du daemon. Page rendements : tableaux méthode ×
objet × tranche de niveau, rentabilité courante, nombre de bots engagés
par script (information pour l'opérateur humain uniquement, jamais lue
par les bots). Page `life` (`/api/lives`, `/api/life/<compte>`) : but,
profil, engagement en cours, graphe des tâches (statuts colorés,
méthode choisie et alternatives avec coûts estimés, avancement,
ticks dépensés), historique des issues. Page `economy`
(`/api/economy`) : masse monétaire (holdings, hors Iron), deals
(réglés, ratés, taux d'échec), top activités par profit/6000 ticks,
tendances publiques 24 h (pénurie, momentum, score), marchands
(capital engagé en achats ouverts, stock en ventes ouvertes, deals
réglés/ratés, ordres et deals ouverts, part maximale du carnet, profit
de référence `market:flip`). Les bots ne lisent aucune de ces pages.

Base `lifescape-store` (SQLite, daemon seul) : ordres, deals,
exécutions, bougies, rapports, rendements, fiabilité, tendances.

## PRD3 : économie centrale

- Ventes par défaut : le cerveau liste le surplus échangeable des bots
  inactifs (`brain_sales`, toutes les 600 s) par LOTS (§ Réserve
  vendeur), prix qui marche ensuite avec la demande sans passer sous la
  réserve ; protégés : pièces, nourriture ≤ 100, keep-list du graphe,
  intrants de tout ce que le plan du bot travaillera
  (`brain_sales::planned_methods` : méthodes et alternatives des nœuds
  niveau/argent ouverts, tranche d'un nœud niveau pas encore résolu,
  tranche de chaque jalon niveau et production de chaque jalon produire
  non finis de l'ambition ; bûches d'un futur allumeur, poisson cru
  d'un futur cuisinier, minerai/barres d'un fondeur/forgeron, runes
  d'un sort de combat) et un produit qu'une autre méthode ouverte
  consomme, outils, porté, pièces de l'échelle d'équipement et palier
  suivant, envies. Les produits fabriqués (barres, forgé, artisanat,
  cuisine) sont vendus. Les annonces sans bornes (posées avant un
  redémarrage) sont réadoptées à chaque passe, marchands compris
  (`market_seller::adoption`) : sous la réserve → relevée à la réserve,
  au-delà du lot de demande ou sans coût connu → retirée (stock rendu à
  la banque, la passe suivante liste un lot), au-dessus de l'ouverture
  du jour → descendue à elle, sinon marche jusqu'à la réserve (voir
  § Butin).
- Achats : `market:buy` posé par le cerveau (offre avec plafond) puis
  PARQUÉ (`park_for_fill`, D-SIDE) : l'offre attend au carnet pendant que
  le worker lance le nœud prêt suivant, sinon une fenêtre d'argent de
  1500 ticks (`brain_filler`) ; jamais un worker inactif ; à côté d'une production, offre permanente (D-B)
  annulée quand le nœud aboutit.
- Équipement : échelle par paliers (§ Équipement par paliers) ; les
  envies (`brain_wishlist`) ne portent plus que les outils pour les buts
  mêlée/non combattants (plafond 25 % du portefeuille par outil), et
  l'équipement des buts distance/magie (50 % partagés) ; une offre au
  repos garde sa marche (`market_reprice::keep_bid`), seul son plafond
  suit le portefeuille ; jamais vendues.
- Pièces comme objectif : un achat non couvert crée `have_coins`, résolu
  d'abord par les PNJ qui lâchent des pièces (hommes ~8,2/kill, gobelins
  ~2,2, tables de drop F2P : seule entrée de pièces sans acheteur, les
  magasins n'achetant jamais), puis la méthode d'argent la plus rentable
  (rates par script) ; le choix de site redescend la liste si le niveau
  de combat manque.
- Intrants des skills de transformation : un `blocked{missing}` (cuisinier
  sans poisson cru, allumeur sans bûches, fondeur sans minerai) crée une
  obtention dimensionnée à la charge d'inventaire (`planner::need_count` :
  27 intrants, 13+13 pour le bronze ; un outil = 1 ; une obtention de N
  barres demande N × l'intrant de la recette ÷ les chances de succès :
  2 minerais de fer par barre de fer, `Way::success` 0,5 lu dans
  `smelting.rs2`), résolue marché
  d'abord ; la progression de niveau remet le compteur de blocages à zéro
  (une charge par blocage est le rythme normal).
- Demande (2026-11-13) : `market_demand::signals()` (offres d'achat au
  repos par objet : nombre, acheteurs, quantité non servie, meilleure
  offre, âge, poids) et `chain_profits()`/`kind_profits()` nourrissent les
  ambitions (voir `docs/planner.md` § Ambitions) ; lecture seule.
- Couverture : pièces et stock en banque comptent (banque connue gardée
  par le SDK après fermeture, voir `docs/protocol.md`) côté cerveau
  (`holdings_count`) et registre (`market_service::ledger_holdings`).
  Côté cerveau, le stock verrouillé par les propres ventes/deals vendeur
  du bot (`locks_for`, les verrous poussés aux workers) n'est PAS détenu :
  `holdings_count`, outils portés (`carried_tools`), base des quotas
  (`brain_locks`, 2026-11-13 : b3gybcev, marteau en vente lu « détenu »,
  forge bannie). Les outils de travail (haches, pioches, filets, cannes,
  appât, tinderbox, marteau, burin, aiguille, cisailles, couteau) ne sont
  jamais mis en vente et leurs annonces existantes sont annulées par la
  passe de ventes (jamais réadoptées).
- Demande vue par un bot (jalons demande, buts d'appoint, tirages) : les
  offres de TOUS les autres comptes, jamais les siennes
  (`market_demand::signals_for`) ; l'affectation de flotte lit la demande
  globale.

## Équipement par paliers (2026-11-13)

Remplacé le 2026-11-14 pour le CHOIX des pièces par § Équipement : le
meilleur abordable (tous les emplacements à la fois) ; les mécaniques de
pas (`insert_gear_step`, retrait des pas morts, `next_ready`) restent.

Règles pures dans `planner/src/gear_ladder.rs`, câblage `brain_gear.rs`
(une passe par compte toutes les 60 s, bots non Iron, but mêlée ou non
combattant).
- Paliers bronze → fer → acier → mithril → adamant → rune (F2P,
  forgeables) ; le noir compte comme l'acier s'il est possédé mais n'est
  jamais un palier (pas de recette : objet marchand D-A).
- Emplacements dans l'ordre : arme (cimeterre, sinon épée longue), corps
  (plastron, sinon cotte), jambes (jambières, sinon jupe), casque
  (heaume, sinon casque moyen), bouclier (kite, sinon carré).
- Le palier visé est le SUIVANT de ce que le bot possède (sac, banque,
  porté) dans le premier emplacement que ses niveaux ouvrent (attaque
  pour l'arme, défense pour l'armure, verrou de quête → famille
  suivante) ; jamais la meilleure pièce absolue.
- Plafond = `GEAR_SHARE_PCT` = 50 % du portefeuille LIBRE (sac + banque
  moins ce qu'engagent les autres achats au repos du bot, prix × reste,
  ouverts ou partiellement appariés : `brain_gear::free_coins`) ; même
  règle pour les envies (25 % par outil) et les achats du graphe (le
  `have_coins` qu'ils déposent rajoute l'engagé à la cible) : chaque plafond
  se prend sur ce que les autres laissent, plus jamais sur tout le
  portefeuille à chaque fois (revue 2026-11-13) ;
  un seul palier à la fois, il monte avec les pièces ; l'autre moitié
  reste aux intrants, outils et achats du graphe.
- Décision (`gear_move`) : pièce possédée non portée → nœud
  `equip_best` seul (une fois par heure et par pièce) ; vendeur au
  carnet ou portefeuille trop mince pour enchérir → nœuds
  `obtain_items` (marché seul, jamais `shop:buy`) + `equip_best`
  (marqueur `gear:ladder`), lancés avant le nœud suivant du but
  (`next_ready`) : un ask ≤ plafond est pris, un ask plus cher bloque en
  `missing:coins:<2×ask>` → `have_coins` (hommes, gobelins, puis la liste
  de sites) ; aucun vendeur → offre permanente D-B (`keep_bid`) à la
  référence, qui monte jusqu'au plafond pendant que le bot continue son
  but.
- Un palier mort (un nœud de sa chaîne abandonné) est retiré à CHAQUE
  passe de lancement, avant `graph_state`, quel que soit le style du but
  (`gear_ladder::withdraw_dead_steps`) : jamais un but clos `stuck` ni une
  compétence bannie pour une cause d'équipement. Un but qui quitte la
  mêlée annule ses offres d'échelle (ses envies distance/magie restent).
- Un palier dont le vendeur a disparu au moment de résoudre (et
  portefeuille suffisant), ou abandonné, est retiré du graphe
  (`lifecycle::withdraw_gear_step`, historique `gear_withdrawn`, ids
  jamais réutilisés) : un palier ne bloque jamais le but.
- Les offres sur d'autres pièces portables (ancien palier, anciennes
  envies « meilleur équipement ») sont annulées ; les pièces gardées
  (meilleure portable par emplacement et paliers supérieurs, palier
  visé) ne sont jamais vendues, les paliers inférieurs le sont.
- Limites connues : distance/magie gardent les envies ; les Iron n'ont
  pas d'échelle (achat au marché uniquement).

## Prix par l'offre et la demande (2026-11-13)

1 pièce n'est que le prix d'amorçage public. Règles
(`lifescape_market::zip`, `market_reprice`) : ouverture à la référence
échangée (VWAP 24 h → dernier print), `2 × réserve` sinon, puis un pas
de 5 % (≥ 1) par fenêtre de 2000 ticks selon les exécutions de la
fenêtre (voir § Market). Un vendeur qui ne vend pas descend jusqu'à sa
réserve (son propre coût, § Réserve vendeur) et y reste ; un vendeur
qui vend monte ; un acheteur qui n'est pas servi monte jusqu'à ce que
son portefeuille paie. Le prix suit donc la masse monétaire : plus de
pièces → plafonds plus hauts → prix plus hauts.

## Réserve vendeur, mémoire des fills, anti-dumping (2026-11-14)

Constat 2026-11-14 : 104 carnets actifs sur 117 à 1 pièce, 84 % des
deals à 1, masse monétaire ×25 en 11 h. Causes : plancher vendeur =
référence/2 avec référence = VWAP 24 h pondéré en quantité (un gros dump
abaisse le plancher : saule 13 → 6 → 2 → 1) ; objets jamais échangés
ouverts à 1 ; stock de flip des marchands relisté à la référence
(ryan_park : 81 % du côté vente à 1) ; lots entiers jetés d'un coup.

Toutes les entrées sont les données PROPRES du bot (ses rapports, ses
fills) ou publiques (carnet, prints) ; jamais l'état privé d'un autre.

- Réserve (`brain_reserve`) = ticks qu'une unité a coûté AU BOT × ses
  pièces par tick, arrondi au supérieur (≥ 1). Ticks par unité mesurés
  sur ses 200 derniers rapports : les ticks d'un rapport sont partagés
  entre ses lignes de farm (`produced`, `loot`, `ground_spawn`, pièces
  ramassées) au prorata `quantité × valeur de jeu` (pièces à 1) ; un
  rapport de récolte ou de production donne tous ses ticks à son
  produit, un rapport de combat sa part de butin à chaque drop. Une unité
  fabriquée ajoute les intrants qu'elle a mangés (lignes `consumed` de
  ses rapports, même partage, `measured_inputs_per_unit`), chacun à SON
  coût propre (ticks mesurés × pièces par tick + ses propres intrants,
  3 niveaux, sinon a priori, jamais sous son dernier prix d'achat ;
  `OwnCosts::unit_cost`) : casque de fer = ses ticks + 2 barres, une
  barre = ses ticks de fonte + son minerai. Non
  mesuré : l'estimation du planificateur (`est_ticks` / quantité d'une
  alternative de farm d'un nœud d'obtention du graphe du bot), sinon le
  taux de démarrage à froid du script (`rates_service::prior_for`).
  Pièces par tick : la meilleure méthode du bot, (pièces ramassées ou
  vendues au magasin + unités farmées, nettes de la vente magasin, × son
  propre dernier prix de vente) / ticks, jamais sous
  `market_fit::MIN_COIN_RATE` (0,1, le plancher du coût unitaire de
  `bid_response` : le temps n'est jamais gratuit ; saule à 43,6 ticks →
  5 pièces). Un objet ACHETÉ coûte au moins son propre dernier prix
  d'achat. Plafond partagé avec la réponse aux offres : la réserve ne
  dépasse jamais le coût de FABRICATION que `bid_response` donne au bot
  (`bid_response::production_cost` = chemin « make » de `unit_cost`,
  pièces + ticks × `market_fit::coin_rate` ; calculé une fois par passe
  par `brain_bids::production_costs_for`, mêmes niveaux, bans, quêtes,
  rendements et carnet que le tirage), arrondi bas, ≥ 1 : un quota compte
  les bids au-dessus de ce coût, le producteur ne garde donc jamais le
  stock que ces bids achèteraient. Butin (`loot`, `ground_spawn`,
  `drop`) : coût nul, réserve `Found` à 1 (§ Butin) ; aucune trace
  propre : coût de fabrication si le bot sait le faire, sinon trouvé.
- Ouverture (`zip::seller_open`) : référence échangée (ou meilleur bid
  plus riche) ; objet jamais échangé : `max(meilleur bid, 2 × réserve)` ;
  jamais sous la réserve ; ramenée à 5× d'une référence échangée (bande).
- Mémoire des fills (`market_seller`, `Engine::own_sells` + `deals`) :
  après une vente RAPIDE (servie ≤ `FAST_FILL_TICKS` = `PATIENCE_TICKS`
  = 2000 ticks après la mise en vente, soit dans la première fenêtre,
  avant tout pas de descente, durée mesurée
  depuis `QuoteBounds::listed_wall` ; un ask qui croise un bid à l'arrivée
  = 0), le lot suivant ouvre à ≥ 105 % de ce prix. Après redémarrage le
  prix revient du store (deals non échoués du bot), la vitesse non (pas
  de hausse). Une offre d'achat ouvre à son propre dernier prix d'achat
  (`remembered_bid`), l'ask couvert restant pris d'abord.
- Anti-dumping : un lot = `market_fit::sale_lot` (bids des AUTRES
  comptes + volume 24 h, au moins 28, plafonné au stock libre ; une
  seule règle avec la réponse aux offres) ; le reste reste en banque, le
  lot suivant part quand le précédent n'a plus de ligne au repos
  (vendu). Jamais sous la réserve, jamais d'expiration qui brade. Ce que
  le plan du bot consommera n'est pas du surplus : jalons ouverts de
  l'ambition par `market_fit::own_use_items` (`cleanup_keep`), nœuds
  ouverts du graphe par `brain_sales::planned_methods` (tranche d'un nœud
  niveau non résolu, runes d'un sort de combat). Mêmes règles pour le
  `market sell` et le `market cleanup` des workers (op `seller_quote`,
  niveaux live lus au superviseur pour le plafond de fabrication).
- Marchands (`flip_quote`) : achat au meilleur bid + 1 seulement quand
  l'écart `(ask − 1) − (bid + 1)` couvre la marge minimale = ticks par
  unité flippée (mesurés sur ses rapports `market`, sinon 6000/100,
  l'estimation de l'engagement `market:flip`) × ses pièces par tick ;
  vente d'un lot de demande à l'ask − 1, jamais sous son prix d'achat +
  marge (ou sa réserve pour un stock non acheté), à 2 × ce plancher
  sans ask ; jamais réf ± 1. Le stock de flip (objets que le marchand a
  achetés) n'est jamais relisté par `maybe_list_surplus`.
- Coût CPU du repricing (2026-11-14, ~55 amends/s et ~10 000 lignes de
  log en 3 min après le redémarrage) : patience 2000 ticks (était 200) ;
  un ordre dans sa fenêtre est sauté avant toute lecture du store ; un
  ordre parqué à une borne (réserve, plafond) ferme sa fenêtre sans
  amend ni log ; une référence lue une fois par objet et par passe ; les
  amends d'une passe partagent une transaction (`Store::begin_batch` /
  `commit_batch`) ; une ligne de synthèse par passe (`market-reprice: N
  moved, P at a bound, R refused`). Adoption idempotente : un ordre
  adopté (ou relevé, retiré) une fois n'est jamais repris
  (`Engine::adopted`, `market_reprice::handled`), une ligne de synthèse
  par compte et par passe.
- Plafond acheteur (décision utilisateur) : le plafond « juste »
  `min(pièces/qté, max(3 × réf, 20))` est retiré
  (`resolve_obtain::bid_ceiling` = pièces / quantité) ; aucun plafond lié
  à la richesse.

Simulations hors ligne (`market/src/sim_demand.rs`,
`orchestrator/src/economy_sim_tests.rs`) : un acheteur (plafond 60)
fait monter son offre depuis 1 ; le producteur (intrants 20) démarre
dès que l'offre couvre ses intrants et vend avec marge ; un ask servi à
chaque fenêtre monte de 1 à ~10 (ce que paient les acheteurs), servi une
fenêtre sur trois il reste plus bas ; une surabondance reste à ~1. Bout
en bout : cimeterre de fer (2 barres à 15), combattant à 200 pièces
(plafond 100) → le forgeron vend à 31, marge positive ; à 40 pièces
(plafond 20 < 30) aucune production tant que `have_coins` ne porte pas
le portefeuille à 62.

Crochet de production (`planner/src/profit.rs`, pour les ambitions et
les producteurs) : `chain_profit(objet, prix, unités_par_6000)` →
`ChainProfit { way, sale_price, inputs, input_cost, margin,
units_per_6000, profit_per_6000 }` ou une erreur typée
(`NotProduced`, `MissingSalePrice`, `MissingInputPrice`, `BadYield`).
`prix` = prix marché par nom debug : celui de l'objet (meilleur bid au
repos si un acheteur attend, sinon la référence) et de chaque intrant
(référence, ou meilleur ask s'il faut l'acheter) ; jamais un prix de
magasin. Rendement = rates mesurés (`task_yields`) ou estimation de
démarrage. La meilleure recette dont tous les intrants ont un prix est
retenue ; les intrants récoltés soi-même sont comptés au prix de marché
(coût d'opportunité). `profit_per_6000 > 0` avec une offre au-dessus de
`input_cost` = signal de produire (entraîner le skill, acheter les
intrants).

## Producteurs qui répondent aux offres (point 7, 2026-11-14)

À CHAQUE tirage de but (`brain_succession::draw_next`, avant le jalon
de l'ambition, pas seulement quand les compétences sont bannies), le
cerveau lit le carnet PUBLIC (`brain_bids` : ordres au repos du moteur,
ses propres offres exclues, volume 24 h des exécutions) et demande au
planner (`planner/src/bid_response.rs`, pur) s'il existe une offre
d'achat que ce bot sait fabriquer avec profit :
- Coût unitaire (`unit_cost`, récursif sur la recette, 3 niveaux) : le
  moins cher de ACHETER (asks au repos qui couvrent la quantité) et
  FABRIQUER (jambe de production praticable MAINTENANT : niveau atteint
  et compétence non bannie, quête faite, outils détenus ou achetables —
  ask au repos, ou magasin PNJ si D-A l'autorise —, site hors guilde ;
  ses ticks = 6000 / unités par 6000, mesurées sinon a priori, sinon 600
  fabrication / 200 récolte ; chaque intrant à SON coût unitaire). Un
  stock détenu coûte ce que sa vente rapporterait (meilleur bid public)
  et aucun temps. Total = pièces + ticks × taux du bot.
- Taux du bot (`market_fit::coin_rate`) : meilleur profit publié (page
  rates) parmi les scripts que ses niveaux ouvrent (bande ≤ niveau),
  plancher 0,1 pièce/tick (le temps n'est jamais gratuit) ; les flips
  ne comptent pas.
- Profondeur : bids qui paient plus que le coût total ; moins les asks au
  repos à ≤ 125 % du meilleur bid (offre déjà en route : ces vendeurs
  descendent vers les bids) ; moins le stock libre du bot (il le vend
  sans fabriquer) ; le bot en prend la MOITIÉ (`PRODUCER_SPLIT`, arrondi
  haut, ≤ 100). Revendre l'objet acheté n'est pas produire (marchands).
- Règle de score : taux brut = (revenu des bids pris − pièces dépensées)
  / (ticks de production + trajet réel du lot, § Rentabilité de tout le
  marché ; l'ancien forfait de 800 ticks par réponse est retiré). L'offre
  passe DEVANT l'ambition seulement si ce taux ≥ 2 × le taux du bot
  (`BID_PREEMPT`) et si le net après le temps ≥ 50 pièces
  (`BID_MIN_PROFIT`). Entre plusieurs, le meilleur taux, incliné de
  ±15 % par un mot propre au bot et à l'objet.
- Étalement (sans état d'un autre bot) : un bot ne répond à un objet que
  dans une fenêtre de 6000 ticks sur deux choisie par son mot
  (`answers_now`, rotation par fenêtre) ; avec la moitié de la
  profondeur par producteur, les bots capables ne se ruent pas tous sur
  le même carnet.
- But : `GoalKind::Produce{objet, N}` (jambes de production seules,
  intrants par les sous-tâches, marché d'abord), origine `bid:<objet>` :
  aucun jalon, ambition intacte, le tirage suivant la repèse. Log
  `brain <compte>: answers bids: produce N <objet> (best bid …, unit …
  coins + … ticks, … coins/tick vs own …, net …; plan)`. Le produit fini
  sort de la keep-list et part par la vente par défaut (`brain_sales`,
  ouverture au meilleur bid, pas de 5 % ensuite).
- Iron : jamais.

## Saturation et usage propre (« les bots jouent comme des joueurs », 2026-11-14)

`planner/src/market_fit.rs` (pur, carnet public + état du bot) :
- `saturation_factor(vue, lot)` = (profondeur des bids + volume 24 h) /
  (asks au repos + lot), borné [0, 1] ; 1 sans ask (le premier vendeur
  fixe le prix) ou sans donnée. Les méthodes d'argent récoltées
  (`gather:<…>/<objet>`) valent leur profit publié × ce facteur
  (`brain_market::fill_ctx` → `saturated_profit`) : 33 364 bûches de
  saule à 1 pièce pour 4 319 échangées par jour ⇒ ×0,13, une méthode
  dont le produit se vend passe devant. Les combats (butin mêlé) gardent
  leur profit.
- `own_use_items(ambition)` : ce que les jalons ouverts du bot
  consommeront (chaque tranche d'un jalon de niveau, de son niveau live à
  la cible, `sizing::tranche_consumption` : bûches de l'allumeur, poisson
  cru du cuisinier, minerais de la fonte, os du prêtre, runes, flèches ;
  intrants d'un quota et les leurs : barres puis minerais). Ajouté à
  `brain::cleanup_keep` ⇒ ni vendu (`brain_sales::working_keep`) ni
  jeté au rangement : un allumeur coupe ses bûches pour les brûler.
- `sale_lot(stock, vue)` = ce que les acheteurs prennent (bids + volume
  24 h), au moins un inventaire (28), jamais plus que le stock : taille
  d'une annonce de joueur. Câblé (2026-11-14) dans la vente par défaut,
  l'adoption des annonces, `seller_quote` et `flip_quote`
  (`market_seller`, § Réserve vendeur).
- Origine des volumes absurdes (revue 2026-11-14, `task_yields`) :
  plumes ≈ 115 000 en montées de combat sur les poulets (butin, jamais
  lâché) ; saule ≈ 8 900 en tranches de bûcheronnage, 6 500 en `make_money`
  et ≈ 11 500 en obtentions : b8jxrdd2 n93/n109 = `obtain willow_logs
  2359` d'une tranche d'allumage 30→45 (`tranche_consumption`), chaîne
  abandonnée puis 2 035 bûches listées à 1 (corrigé par l'usage propre) ;
  paul_rhodes n60 `obtain oak_logs 308` a coupé 3 187 saules en 106 839
  ticks (le script coupe un autre arbre que la ressource du nœud : bug
  script hors de ce lot). Les quotas des échelles (100–200) sont sains.

## Prix courant : les 5 derniers fills (2026-11-14)

Décision utilisateur : plus aucune décision de bot ne lit la VWAP 24 h.
Un bot price comme un joueur qui regarde les dernières ventes :
`lifescape_market::current_price` = médiane des 5 DERNIERS fills de
l'objet (quel que soit leur âge ; nombre pair = moyenne des deux du
milieu), bornée par les PRIX D'IMPACT du carnet vivant (§ Borne
d'impact : un bid d'impact plus haut est le prix du moment, un ask
d'impact plus bas aussi). Jamais échangé : milieu du carnet, sinon son
seul côté, sinon 1 pièce, `traded: false` (les règles d'ouverture
s'appliquent : `seller_open` ouvre à `max(meilleur bid, 2 × réserve)`).
Branché partout où la référence décidait : `MarketService::reference`
(ouvertures `opening_ask`/`opening_bid`/`remembered_bid`, repricing,
vente, réserve, gear, résolution des nœuds), `rates_price::price_estimate`
(profits publiés, marges de demande `market_demand`, chaînes
`planner::profit`), la cotation `quote` et le prix de la page marché.
Le « % 24 h » de la page reste un affichage (VWAP du jour contre la
veille), aucun bot ne le lit.

CPU (« attention au cpu ») : jamais de scan de l'historique d'un objet.
Le service garde un anneau en mémoire par objet (`Engine::last_fills`,
5 fills (prix, quantité)), chargé une fois par une lecture indexée
`LIMIT 5` (`Store::last_fills`, index `executions(item_id, clock)`), puis
poussé à chaque fill réglé (`settle_print` → `Engine::note_fill`). Avant,
chaque `reference()` lisait TOUTES les exécutions de l'objet depuis 0.
Le volume 24 h du `seller_quote` et des bids est un `SUM` indexé
(`Store::volume_between`), plus les lignes. `price_estimate` (rates) : 3
lectures indexées (5 fills, puis les meilleurs bids et les meilleurs
asks jusqu'à couvrir la taille d'impact, `Store::book_levels_covering`,
`LIMIT 64` ordres par côté, arrêt dès la taille couverte).

### Borne d'impact (2026-11-14)

Défaut corrigé : borner par le MEILLEUR bid/ask laissait une poussière
fixer le prix — une unité listée à 1 pièce ramenait à 1 le prix d'un
objet échangé par lots de 100, pour tous les bots (le cliquet à 1 pièce
historique). Comme le « mark price » des dérivés (prix d'impact), la
borne lit maintenant le prix qu'il faut pour remplir un lot réel :

- taille d'impact N = médiane des QUANTITÉS des 5 mêmes derniers fills
  (`lifescape_market::impact_size`, au moins 1, plafond `IMPACT_CAP` =
  1000) ; jamais échangé → N = 1 ;
- ask d'impact = prix de la N-ième unité en remontant les asks au repos
  depuis le meilleur ; bid d'impact pareil sur les bids
  (`impact_price`) ; un côté dont la profondeur totale est < N ne borne
  PAS (un côté mince ne fait pas le prix) ;
- prix courant = médiane des 5 derniers prix bornée dans
  [bid d'impact, ask d'impact] (bid d'abord, puis ask, l'ask gagne si
  le carnet est croisé) ;
- jamais échangé (aucun fill) : règles inchangées (milieu des deux
  côtés, sinon le seul côté, sinon 1), avec N = 1 donc les meilleurs
  prix.

Exemples (tests `price.rs`, `market_impact_tests.rs`) : plumes en lots
de 100 à 4, une unité à 1 → prix 4 ; 150 unités à 2 → prix 2 ; 10 unités
bidées à 9 → prix 4 ; 90 de plus à 7 → prix 7 (la 100e unité). Les
bornes de la bande anti-bug ×5/×0,2 lisent toujours `guard_anchor` =
cette référence, `traded` garde son sens (vrai dès un fill).
Coût : `MarketService::reference` parcourt dans le carnet mémoire les
quelques meilleurs ordres de chaque côté jusqu'à N unités
(`Book::levels_covering`), une passe, jamais le côté entier.

## Équipement : le meilleur abordable, tous les emplacements (2026-11-14)

Constat live : 25 combattants niveau 40-58 en heaume moyen de bronze et
dague de fer ; seuls des cimeterres de BRONZE montaient au carnet (18
offres, 29 → 7321), un emplacement à la fois (`gear_ladder` : palier
suivant de l'arme d'abord), jamais un plastron, des bottes ou une
amulette ; les obtentions d'anciens buts `equip_best` (`iron_platebody`,
`amulet_of_power`, `black_cape`, `leather_boots`…) ouvraient à 1 et
étaient annulées après 6000 ticks à 1-20 pièces (« unfilled after 6000
ticks », ~1 000 à 1 600 fois chacune dans les lignes « graph stuck »).

Règle (`planner/src/gear_plan.rs`, pur ; câblage `brain_gear.rs`, une
passe par compte toutes les 60 s, tout style de but, jamais Iron) :
- Emplacements : arme, corps, jambes, tête, bouclier, pieds, mains, cou,
  cape. Candidats par style (calculés une fois) : objets F2P échangeables,
  obtenables sans quête, score de style > 0 (`gear::score` : mêlée =
  bonus + 2 × force, distance = arc et cuir, magie = bâton et robes) ;
  mêlée et magie à une main (bouclier), distance à l'arc (pas de
  bouclier) ; munitions et runes restent des consommables des tranches.
- Le bot veut, par emplacement, la meilleure pièce que ses niveaux
  ouvrent et qui bat ce qu'il possède (sac, banque, porté). Le choix de
  l'arme suit le score (hache de bataille, cimeterre ou épée longue).
- Budget = 50 % (`GEAR_SHARE_PCT`) du portefeuille libéré de ses achats
  NON équipement ; prix attendu = médiane des 5 derniers fills, sinon la
  valeur de haute alchimie (personne ne vend dessous) ; tant que la somme
  dépasse le budget, l'emplacement le plus cher descend à sa pièce
  suivante moins chère (ou sort). Plafond de chaque envie = sa part du
  budget (attendu × budget / total) ; l'offre ouvre AU PRIX ATTENDU
  (`market_reprice::keep_bid_from`, jamais à 1) et monte avec la patience.
- TOUS les emplacements enchérissent à la fois. Un ask au repos sous le
  plafond, ou une pièce que seul un magasin vend (D-A), devient un nœud
  `obtain_items` + `equip_best` (un pas ouvert à la fois, `shop:buy`
  ajouté pour le seul cas D-A). Une pièce possédée meilleure que la
  portée → `equip_best`. Les offres sur des pièces que le plan ne veut
  plus sont annulées.
- Gardé (jamais vendu) : par style et emplacement, la meilleure pièce
  portable et toute meilleure pas encore portable (`kept_gear`) ; les
  pièces remplacées sont du surplus (vente par défaut, ou alchimie d'un
  lanceur). La liste d'envies (`brain_wishlist`) ne garde que les outils.

## Trousse permanente : hache, pioche, marteau, tinderbox (2026-11-14)

Constat live : personne au-delà d'une pioche de bronze ; 185 offres
« wish … pickaxe » jamais remplies (aucune recette F2P ne forge de pioche
en rev 274, aucun monstre F2P n'en lâche : seul Nurmof, `pickaxeshop`,
Dwarven Mine, vend fer → rune) ; puis décision utilisateur : chaque bot
(Iron compris) possède en permanence une hache, une pioche, un marteau et
une tinderbox, et ne les revend plus en changeant de but.
Règle (`planner::tool_kit`, passe `brain_tool_shop.rs` de 60 s, banque
diffusée, avant le plan d'équipement) : trousse de base d'abord (marteau,
tinderbox, pioche, hache manquants : meilleur palier ≤ 25 % du portefeuille
libre, sinon le moins cher ≤ 50 %), puis améliorations (meilleur palier que
le niveau ouvre au-dessus du palier qui travaille, ≤ 25 % `TOOL_SHARE_PCT`).
Chaque achat devient un pas d'équipement (`file_step` : un pas à la fois,
cooldown `WITHDRAWN_RETRY_SECS`, jamais de bannissement) : `shop:buy`
(Nurmof, magasins généraux) ou, pour une hache, ask couvert puis Bob's
Brilliant Axes (exception D-A `kit_shop_route`, bronze → acier). Aucune
offre marché sur un outil vendu en boutique (`brain_wishlist::biddable`),
les anciennes sont annulées ; les haches mithril+ (forgées seulement)
gardent l'offre de la liste d'envies. Garde (`brain_tools::kept_tools`) :
meilleure hache/pioche détenue + meilleure qui travaille, 1 marteau,
1 tinderbox ; le reste se vend. Les outils inutilisés restent en banque :
chaque script ne garde au sac que son outil (`ToolSpec::keep_ids`), le
voyage en banque dépose le reste une fois, rien ne les retire hors besoin.
La Dwarven Mine est un étage de boutiques routé
(`access::shop_floor_routed`, trappe de Falador) : la cueillette et les
fermes de drop y restent refusées.

## Producteurs : entraînement pour une offre (2026-11-14)

Constat live : personne ne forgeait de cimeterre à 2 812 pièces parce
que TOUTE la flotte avait forge 1 (sara_miller 18, harry_page 10,
bh99tw2f 7) et que `bid_response` n'ouvrait que les jambes praticables
au niveau ACTUEL (cimeterre de bronze : forge 5). Les tirages, eux, sont
fréquents (~120 par heure pour ~25 bots) : la fenêtre n'était pas le
frein. Depuis : une jambe jusqu'à 15 niveaux au-dessus
(`MAX_TRAIN_LEVELS`) d'une compétence de production (forge, artisanat,
cuisine, minage, pêche, bûcheronnage, allumage, runes) compte pour une
offre (`unit_cost_training`) ; l'entraînement (méthode de tranche
`reach_method`, actions par la table d'XP serveur, rythme du bot,
intrants à leur coût unitaire) est un coût unique du lot, ajouté aux
pièces et ticks de l'opportunité (`training_cost`). Le but `Produce`
bloque sur le niveau (`missing:level:…`) et s'entraîne d'abord. Le
plafond de la réserve vendeur (`production_cost`) reste sans
entraînement.

## Alchimie (2026-11-14)

`planner/src/alchemy.rs` (pur) + `orchestrator/src/brain_alch.rs` :
- Sorts de la table serveur : haute alchimie (magie 55, 1 nature + 5
  feu, 65 XP), basse (21, 1 nature + 3 feu) ; un bâton de feu paie le
  feu. Valeur = `alch_high`/`alch_low` des configs (coût × 0,6 / 0,4).
  Net d'un lancer = valeur − runes au prix courant ; 1 lancer par 5 ticks.
- Méthode d'argent : la meilleure alchimie du stock propre du bot (hors
  keep-list, gear gardé, verrous d'annonces), seulement si le net bat la
  vente (meilleur bid, sinon prix courant), est publiée `alch:<objet>`
  dans `method_profit` des nœuds argent/pièces (`brain_alch::publish`) ;
  `resolve_money` la classe avec les autres ; `required_tool` exige une
  rune de nature et du feu (bâton accepté).
- Tirage : un stock à alcher qui vaut le temps du bot comme une offre
  (taux ≥ 2 × son taux, net ≥ 50, runes de nature en main pour ≥ 10
  lancers) devient un but `MakeMoney` d'origine `alch:<objet>`
  (ambition intacte).
- Ventes : un lanceur ne liste pas ce qui s'alche mieux que ça ne se
  vend (`alch_keep`) et ne vend jamais sous le net du lancer (plancher
  `alch_floor`) : la valeur d'alchimie devient le plancher naturel du
  marché.
- Marché (`market_pass`, toutes les 600 s) : runes de nature pour le
  stock à alcher (offre au prix courant + la moitié de la marge ; le
  marché seulement : un spot de combat les lâche, D-A), et l'ask d'un
  autre compte le plus loin sous `net − 5 ticks × taux` est acheté pour
  l'alcher.
- 2026-11-15 : les billets (`cert_*`) comptent comme leur objet dans le
  stock à alcher (`stacks_of` ; avant, un lot acheté arrivé noté lisait
  « missing » et le nœud abandonnait) ; un lanceur garde ses runes de
  nature, de feu et son bâton (`caster_means`, ventes et `cleanup`) ; il
  enchérit sur les runes de nature pour la meilleure occasion, son stock
  ou à défaut le meilleur ask à alcher (plafond = prix courant + moitié
  de la marge, `nature_ceiling`) ; les achats à alcher descendent la
  liste par profit par lancer DÉCROISSANT (`rank_by_profit`) tant que
  runes, pièces et lot suivent.
- Entraînement, classement (décision utilisateur 2026-11-15 : « la
  méthode la plus rentable », remplace le moindre coût par XP) : toute
  méthode qui gagne des pièces passe avant toute perte, la meilleure en
  pièces par tick (temps non compté) ; si toutes perdent, la plus petite
  perte en pièces par XP (`training::sort_best_first`, `rank_key`).
- Entraînement (2026-11-14, `training.rs`, `brain_train.rs`, voir
  docs/planner.md « Entraînement au prix ») : dès magie 21, chaque
  tranche compare au lancement le sort de combat et l'alchimie (stock
  propre ou asks d'autres comptes au plus à la valeur d'alchimie) en
  pièces par XP, perte comprise : l'alchimie entraîne même à petite perte
  quand elle coûte moins par XP que les sorts. Le lot (lancers que stock,
  runes et bourse couvrent) part en `alchemy <objet> <lot> --level magic
  N` ; runes de nature au marché seulement (drops F2P, aucune boutique
  F2P hors Wilderness), bâton de feu acheté quand il coûte moins que les
  runes de feu de la montée. Aucun bot n'a magie 21 aujourd'hui (max 10) :
  vérifié par simulation (magie 25 et 60, `brain_train_tests.rs`).

## Rentabilité de tout le marché (2026-11-14)

Demande utilisateur : « le brain doit être capable d'évaluer la
rentabilité en fonction de toutes les dépendances et des rates (si
existants), pour tout le marché » ; une forte demande d'un produit doit
faire produire et vendre, et tirer le prix de ses composants vers le
haut. Constat live (11:40) : seuls les bids `pot_flour` trouvaient
preneur ; bst29bh9 (forge 22, minerai de fer en banque) perdait 559 sur
3 heaumes de fer à 123 à cause du forfait de 800 ticks ; le bouclier de
fer (forge 27) portait toute la montée 22→27 (−12 431) ; personne
n'enchérissait sur les barres ni les minerais au prix que les produits
justifient.

Évaluation (`planner/src/bid_response.rs`, `market_value.rs`, câblage
`brain_bids.rs`) — à chaque tirage, pour CHAQUE objet de valeur
(offres d'autres comptes, prix courant échangé avec volume 24 h, plancher
d'alchimie du bot lanceur) qu'une jambe de production fabrique :
- Coût unitaire récursif (profondeur 4 : amulette enfilée, amulette,
  barre, minerai) : acheter (asks qui couvrent la quantité) ou fabriquer
  (niveau, bans, quête, outils, rendements appris sinon a priori), chaque
  intrant à son propre coût ; stock détenu au prix de sa vente.
- Frais fixes du LOT, jamais par réponse : site choisi comme le brain le
  ferait (`market_value::leg_site` = `combat::pick_site_gated` au niveau
  de combat du bot, sites sûrs et ouverts seulement) ; marche (Manhattan,
  1 tick/case) de chaque étape vers l'étape suivante (minerai → fourneau
  → enclume → fontaine), péage Al Kharid aller-retour (sauf Prince Ali)
  et bateau de Karamja, outils achetés une fois, trajet fontaine → site
  pour les intrants achetés (une remise : le plus long), trajet de la
  position du bot au premier site, remise à la fontaine (`HANDOVER_TICKS`
  = 100). Le coût par unité étale ces frais sur le lot (`total_for`).
- Revenu = ce que le marché absorbe (`market_value::absorbed_lot`) :
  bids au-dessus du coût, moins les asks à ≤ 125 % du meilleur bid (offre
  en route) ; plus les ventes au prix courant sur la journée (50 % du
  volume 24 h moins les asks déjà au repos à ce prix ou dessous) ; moins
  le stock libre du bot et les unités RÉCLAMÉES par d'autres producteurs
  (tableau public, ci-dessous) ; le bot en prend la moitié
  (`PRODUCER_SPLIT`, ≤ 100) ; un lanceur ajoute son plancher d'alchimie
  (net des runes et de 5 ticks de lancer au taux du bot) jusqu'à un
  inventaire (28).
- Entraînement = investissement (`training_charge`) : coût brut de la
  montée (temps au taux du bot + intrants) moins la valeur des produits
  faits en montant (prix courant ou meilleur bid × saturation), puis une
  PART seulement : rien quand l'ambition du bot monte déjà cette
  compétence (échelle du catalogue ou jalon niveau ouvert,
  `brain_bids::planned_levels`), un lot sur `TRAINING_AMORTIZE_LOTS` (4)
  sinon. `MAX_TRAIN_LEVELS` (15) inchangé.
- Règle de choix inchangée : taux ≥ 2 × taux du bot et net ≥ 50 ;
  meilleur taux incliné ±15 % ; étalement une fenêtre sur deux. Log :
  `brain <compte>: answers the market: produce N <objet> (k to bids, best
  …; k at the current price; k alched; revenue …, unit …, lot … ticks,
  … coins/tick vs own …, net …, train …; plan)`.
- CPU : un choix par compte et par fenêtre d'étalement mis en cache 60 s
  (`CHOICE_TTL_SECS` ; un tirage réessayé à chaque passe le réutilise) ;
  prix courants et volumes lus à la demande et mémorisés par lecteur ;
  objets sans jambe de production écartés avant toute lecture du store ;
  volume lu seulement pour un objet avec bids, prix échangé ou alchimie.

Taux du bot réalisable (`brain_bids::rate_rows`, `realizable_rows`) :
le taux de `market_fit::coin_rate` lit les profits publiés MULTIPLIÉS par
la part que le marché absorbe de chaque produit de la ligne ((bids +
volume 24 h) / (asks + rendement d'une fenêtre), pondéré par la valeur ;
pièces ramassées entières ; un produit sans aucun acheteur ne vaut rien
à l'heure). Live : quelques fills de cimeterres de bronze publiaient
`smith` tranche 1 à 65 929 / 6000 ticks, soit 11 pièces/tick pour TOUS
les bots (forge ≥ 1) : aucun lot ne battait ce temps. Table partagée,
reconstruite toutes les 60 s (une lecture du carnet). La page rates
reste brute (information opérateur).

Tableau des commandes (`market_claims.rs`) : un bot qui prend un lot
publie « N unités de X réclamées » ; tout bot lit le total réclamé par
les autres (jamais qui, jamais autre chose) et le retire de ce que le
marché absorbe. Public comme le tableau des commandes d'artisanat d'un
vrai jeu : D22 tient (voir `decisions.md`). En mémoire, expire après 2 ×
la durée estimée du lot (10 min à 6 h), libéré au tirage suivant du bot.

Demande dérivée (`planner/src/derived_demand.rs`,
`brain_market::derived_reserve`) : quand un nœud d'obtention nourrit un
produit du graphe (barres d'un heaume, minerai d'une barre), son prix de
réserve = (valeur du produit × 0,8 − temps de l'étape au taux du bot −
autres intrants au meilleur ask / prix courant) / unités par produit,
récursif (le minerai vaut la valeur DÉRIVÉE de la barre). Valeur du
produit = meilleur bid d'un autre compte, sinon prix courant. L'offre
permanente posée à côté de la production (D-B) a pour plafond
`min(portefeuille, réserve)` et ouvre à au moins la moitié de la réserve
(`derived_bounds`) : une barre à 2 pièces que personne ne remplit ne
fait bouger aucun mineur, une barre à 23 si. Fondeurs et mineurs y
répondent par le même moteur (les barres et minerais ont des jambes de
production). Une réserve ≤ 0 (produit qui vaut moins que ses intrants)
plafonne l'offre à 1. Log `standing bid … (ceiling C, derived reserve R)`.
Toutes les recettes F2P passent par ces chemins (forge, fonte, cuisine,
artisanat, poterie, cuir, bijoux, filage, runes, récoltes).

Ambitions (`ambition_catalog::kind_weights`, `serves`) : une offre
compte pour un type quand son échelle liste l'objet OU que sa
compétence primaire le fabrique à n'importe quel palier (adamant hors de
portée de toute la flotte → forgerons), et pour moitié par étape de
recette quand sa compétence en fabrique un intrant (minerai, charbon →
mineurs) ; une marge connue négative ne tire personne. L'échelle du
forgeron va jusqu'à l'adamant (minage 70, forge 78 dague/heaume/
cimeterre, forge 88 plastron/jambières).

Vendeur : un ask ancré sur un fill isolé descend (test
`market/src/outlier_tests.rs`, cas du cimeterre de bronze) : le prix
courant est borné par l'ask d'impact (1 unité → 1149, pas 2747), un
nouveau lot ouvre là, et les asks au repos marchent de 5 % par fenêtre
jusqu'à leur réserve (≤ 140 en 60 fenêtres). Bande ×5 inchangée.

## Butin : coût nul, lots entiers, prix qui écoule (2026-11-14)

Constat live (15:40, frank_lane, mêlée aux guerriers d'Al Kharid) :
banque 52/60 types dont 88 heaumes moyens de bronze, 41 dagues de fer,
838 flèches de bronze ; une seule annonce de dague (1 @628 pour ~30),
aucune pour les heaumes ni les flèches ; bâton 2 @1644, flèche d'acier
1 @729, rune de loi 2 @330 ; « holding 1 bronze_dagger (no own cost
known) ». Causes vérifiées :
- la réserve partageait les ticks du combat entre les drops au prorata
  de leur valeur : une flèche d'acier « coûtait » 3 541, une flèche de
  bronze 12 (hors bande 5× d'un prix de 2 → gardée), un drop absent des
  200 derniers rapports n'avait aucun coût → gardé ;
- le heaume PORTÉ protégeait par son nom toute la pile de la banque
  (idem pour la meilleure pièce gardée par `kept_gear`) ;
- une annonce de 1 unité excluait l'objet des passes suivantes : la pile
  grossissait derrière un éclat jamais vendu ;
- une annonce réadoptée gardait son prix ancré sur l'ancienne réserve et
  ne descendait que de 5 % par fenêtre.

Règles :
- Coût propre (`brain_reserve`) : seules les unités `produced` portent
  des ticks (récolte, fabrication) ; le butin, les drops et les
  apparitions au sol sont TROUVÉS : le combat se paie en XP ou en
  pièces, ils ne coûtent rien (`own_origin`, réserve `Found` à 1). Un
  objet à la fois fabriqué et trouvé garde le coût de sa part fabriquée
  (ticks et intrants × part fabriquée). Sans trace propre : estimation
  du plan ou a priori d'une récolte (pêche, bûcheronnage, minage ; plus
  jamais l'a priori de combat), sinon coût de fabrication `bid_response`
  si le bot sait le faire, sinon trouvé. Un achat garde son prix payé
  comme plancher. Un intrant trouvé ne coûte rien non plus
  (`unit_cost`).
- Ouverture d'un objet trouvé (`lifescape_market::found_open`) : objet
  échangé → prix courant (médiane des 5 derniers fills bornée par
  l'impact) ou meilleur bid plus riche ; jamais échangé → juste sous
  l'ask le moins cher (ou dans un bid plus riche), carnet vide → valeur
  de haute alchimie (donnée publique, le prix attendu de `gear_plan`),
  sinon 2 × plancher ; jamais au-dessus de la bande 5× d'un objet
  échangé ; mémoire d'un fill rapide inchangée. Plancher 1, ou le net
  d'alchimie d'un lanceur (`alch_floor`) ; les lanceurs achètent sous ce
  net (`brain_alch`), c'est le plancher naturel du marché. La patience
  (5 % par fenêtre de 2000 ticks) descend l'ask jusqu'au plancher.
- Lots entiers : `sale_lot` inchangé (bids + volume 24 h, au moins un
  inventaire, jamais plus que la pile). Une annonce intacte qui n'offre
  qu'un éclat d'une grosse pile (≤ moitié du lot de la pile entière,
  `market_seller::undersized`) est retirée et la pile part en un lot
  dans la même passe (`brain_sales::regroup_slivers` ; objets non
  gardés, lignes `open` seulement, jamais le stock de flip d'un
  marchand).
- Réadoption (après redémarrage) : un ask au-dessus de l'ouverture du
  jour descend à elle (`Adoption::Lower`), jamais sous la réserve.
- Équipement porté ou gardé : UNE copie de rechange reste (une mort fait
  perdre le porté), le reste est du surplus
  (`gear_plan::spare_gear_units`) ; une pièce que le plan garde entière
  (obtention, quête) n'est pas touchée.
- Gardé (inchangé) : nourriture jusqu'à 100 unités, outils de travail,
  intrants des méthodes ouvertes, gear gardé, objets d'ambition et de
  quête ; un plan à distance garde ses munitions (`fight:ranged/` →
  flèches de la tranche), un combattant de mêlée vend les siennes.

Vérification hors ligne (`loot_check_tests.rs`, ignoré, mêmes variables
que `live_check` + `LIFESCAPE_CHECK_ACCOUNT` optionnel) : sur une copie
de la base et un `/api/status`, les annonces regroupées, abaissées et
les lots que chaque bot posterait.

## Entraînement en achetant les composants (2026-11-15)

Cooking, crafting, firemaking et prayer rejoignent la forge et la magie
dans l'entraînement au prix (`planner/src/training_production.rs`, voir
`docs/planner.md` § Magie, ranged, demande servie) : chaque recette
ouverte est chiffrée à ce que coûtent ses intrants au bot (stock détenu
à sa valeur de vente, sinon achat au marché ou fabrication, temps du bot
compris à son taux de gain) moins ce que le marché paie le produit. La
plus rentable gagne, sinon la moins chère par XP : un bot qui gagne
mieux en farmant de l'argent achète les composants bon marché au lieu de
les récolter.

