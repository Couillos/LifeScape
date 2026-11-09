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
de Varrock (anneau autour de la fontaine, cases attribuées par index) :
`matched → convened → both_arrived → trading → settled |
failed{no_show|mismatch|death|timeout|nav_down}`, échéance = horloge + 1 h
(les ETA nav viendront avec les bots live). Le registre règle à
l'appariement ; un `failed` ultérieur rembourse le transfert et dégrade la
fiabilité des deux parties.

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
exécutions ; canvas maison, zéro dépendance), alimentée en démonstration
simulée (24 objets, 36 h d'historique, profondeur de part et d'autre de la
référence) tant qu'aucun bot ne trade. Pages rendements, vie, économie :
inchangées ce lot. Base `lifescape-store` (SQLite, daemon seul) : ordres,
deals, exécutions, bougies, rapports, rendements, fiabilité, tendances.
