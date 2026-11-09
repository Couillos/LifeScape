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

Une ligne par objet et par fin de tâche `{méthode, objet, provenance,
quantité, ticks, contexte}` ; tout compte (produits, loot, spawns au sol,
ramassés), provenance étiquetée. Agrégation par 6000 ticks, tranches de 5
niveaux, n + moyenne + écart-type + moyenne mobile ; mêmes mesures pour
l'XP. Rentabilité = Σ rendement × prix − intrants − friction market
(`market_pause_ticks` mesurés) + bonus de tendance. Démarrage à froid :
priors marqués, remplacés dès n ≥ 3.

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
