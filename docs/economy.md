# Économie (état cible)

But : prix issus de l'offre et de la demande entre bots, rendements mesurés
en continu, visibles au dashboard et dans une base interrogeable.

## Market

Double enchère continue, un carnet par objet échangeable F2P : priorité prix
puis ancienneté, exécutions partielles au prix du carnet, modifier =
annuler + replacer. Stratégie de prix inspirée de ZIP : marge ajustée sur
les dernières cotations, plancher = réserve (rachat magasin, alchimie,
valeur d'usage), plafond = magasin PNJ ou temps économisé. Couverture
vérifiée (objet possédé, pièces disponibles), bande anti-bug 0,2×–5× la
référence, ventes jusqu'à 1 pièce. Deals livrés physiquement place de
Varrock : `matched → convened → both_arrived → trading → settled |
failed`, convocations échelonnées et regroupées, échéance = 2× trajet +
marge. Référence = VWAP glissant ; bougies OHLCV en horloge (1 h / 7 jours,
1 jour / 6 mois et plus), tick serveur stocké avec chaque exécution.

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

Pages market (prix au-dessus des bâtons de volume, 6 mois / 24 h, dernier
point temps réel), rendements, vie, économie. Base `lifescape-store`
(SQLite, daemon seul) : ordres, deals, exécutions, bougies, rapports,
rendements, fiabilité, tendances.
