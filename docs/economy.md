# Économie (lot L5 : market + simulateur)

But : prix issus de l'offre et de la demande entre bots, rendements mesurés
en continu, visibles au dashboard et dans une base interrogeable.

## Market

Double enchère continue, un carnet par objet échangeable F2P : priorité prix
puis ancienneté, exécutions partielles au prix de l'ordre au carnet, modifier
= annuler + replacer (perte d'ancienneté). Code pur dans
`rs/crates/market/` : `book` (carnet + appariement), `guards` (bandes +
couverture), `deals` (machine à états), `price` (VWAP → mid → magasin,
bougies OHLCV), `trends` (signaux publics), `zip` (cotation + plafonds
marchands), `sim` (simulateur à graines fixes).

Stratégie de cotation inspirée de ZIP : marge ajustée sur les dernières
cotations, plancher = réserve (rachat magasin, alchimie, valeur d'usage),
plafond = magasin PNJ ou temps économisé. Les bornes PNJ ancrent
l'économie : personne ne vend sous le rachat magasin, personne n'achète
au-dessus du magasin qui stocke. Marchands (`market:flip`, ~10 %) :
sélection sur écart/volume, plafonds `max_capital_share` (10 %),
`max_stock_items` (500), `max_book_share` (25 % du côté).

Couverture vérifiée à l'ordre (objet possédé, pièces disponibles, registre
en mémoire ; les holdings live des connectés se brancheront sur le même
contrôle). Bande anti-bug : rejet au-dessus de 5× la référence, achat
rejeté sous 0,2×, ventes jusqu'à 1 pièce. Deals livrés physiquement place
de Varrock (anneau autour de la fontaine, voir § Place ci-dessous) :
`matched → convened(case d'échange, échéance) → both_arrived →
trading → settled | failed{no_show|mismatch|death|timeout|nav_down}`.
Échéance = 2× trajet ETA max + marge (ticks) ; départs échelonnés par
ETA nav (cache par zone de départ, un plan par zone) pour une arrivée
simultanée ; regroupement 300 ticks (une convocation par bot, paires
regroupées en un seul échange) ; une convocation = case + heure de
départ seulement (jamais la position de l'autre). Les ticks sont des
époques par login : départs et échéances sont exprimés dans l'horloge
de chaque bot, le balayage des expirés sur l'horloge murale du daemon
(mesure live du tick, sans interrupteur dev/prod). Le registre règle à
l'appariement ; un `failed` ultérieur rembourse le transfert, replace
la partie présente **avec sa priorité** (seq d'origine conservée) et ne
touche la fiabilité qu'en cas de faute (absent, refuseur ; mort,
timeout, nav_down : neutre). Sans nav : aucune convocation,
appariements conservés, expirations en `nav_down` sans pénalité.

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

Référence = VWAP glissant des dernières exécutions ; sinon milieu du
carnet ; sinon prix magasin. Bougies OHLCV en horloge (1 h / 7 jours,
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
seul). Reliquat connu : les ETA de convocation passent par une constante
d'horloge, pas par le service de navigation ; les holdings live ne sont
pas encore branchées au registre.

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
`consumed` (appâts, nourriture) nourrissent le coût des intrants.

Ingestion : la session pousse le rapport, le superviseur du daemon le
verse dans le store (`jobs::ingest_report_into`) puis dans l'agrégat
persisté (`rates_service`, `run/rates-snapshot.json`, rechargé au
redémarrage : aucune mesure ne se perd avec le process). Les lignes
brutes restent requêtables pour recouper plus tard.

Agrégation : rendement normalisé par 6000 ticks, par triplet (méthode,
objet, tranche de 5 niveaux), avec n, moyenne, écart-type (population)
et moyenne mobile exponentielle (alpha 0,3, ordre d'arrivée). Mêmes
mesures pour l'XP par 6000 ticks, qui dimensionne les durées de montée
de niveau. Sous 3 rapports, la page sert l'estimation de démarrage à
froid (tables de skills du jeu et pratiques de l'époque, marquée
`prior`) ; à 3 rapports et plus, la mesure remplace l'estimation.

Rentabilité d'une activité et d'une tranche : Σ rendement × prix de
vente estimé (VWAP 24 h du market quand le carnet a des exécutions,
sinon prix magasin, cas marqué `price_prior`) − intrants consommés −
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
bougie en cours reste dedans. Page rendements : tableaux méthode ×
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
