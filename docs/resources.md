# Ressources (état cible)

Contrainte : 300 bots connectés 24 h/24 sur 8 cœurs, 20 Go RAM, SSD, avec
serveur LostCity, base et dashboard sur la même machine. Grands runs au
tick de prod (600 ms). Tests live ≤ 20 bots ; tenue à 300 démontrée par
coûts par bot × 300 + banc hors ligne à 300 faux workers (E15).

## Enveloppes

RAM 13,5 Go (système 2, serveur 3, navigateur 1, daemon 1,5, workers 6 à
20 Mo/bot), marge 6,5 Go. CPU : système + navigateur 1 cœur, serveur ≤ 2
(≤ 300 ms de travail par tick), daemon ≤ 1 (≤ 0,33 % par bot), workers ≤
2,5 (≤ 0,8 % actif, ≤ 0,2 % et ≤ 2 réveils/s au repos), marge ≥ 1,5.
Disque : écritures continues hors logs ≤ 1 Mo/s à 300 ; logs 100 Mo/bot en
rotation ; base SQLite quelques centaines de Mo/an. Fonctionnement 24 h/24 :
pente de PSS nulle sur 72 h à 20, structures bornées, redémarrage par
vagues, reconnexions étalées.

## Mesure et protection

`res-stats` : PSS/RSS/CPU/threads/réveils par worker et daemon, plus
compteurs par sous-système (pushes/s, octets, plans/s, interrogations/s,
fichiers/s, connexions/s) sur threads nommés. Admission de sessions :
refus typé au-delà de l'enveloppe, jamais de dégradation silencieuse.

## Chantiers

Ordre fixé par la répartition mesurée (E13, threads nommés) : connexion
persistante unique (E2), nav sans interrogation (E3), heartbeat en deux
flux (E4), workers pilotés par événements (E5), allocateur si T2 le justifie
(E6), multi-process seulement si ≥ 20 % CPU (E7), supervision en un thread
(E8), Varrock sans plafond avec coût mesuré (E9), ouvertures à débit borné
(E10), logs bufferisés sans perte (E11), observabilité conservée et
optimisée (E12), sobriété des demandes de plans (E14), banc E15.
Règle permanente : chaque brique déclare et mesure son coût à sa porte V2.

## Coûts déclarés

Guides money making et entraînement (2026-11-17, thread `guides`,
`orchestrator::guides_store`) : aucune requête ne calcule un guide ; un
jeu complet (vues catalogue + une vue money et une vue training par
tranche 1-5 … 96-99, 42 vues, ~94 prix D17) est reconstruit toutes les
300 s dans un seul moteur et servi en `Arc` à l'API et au cerveau (R7).
Mesures sur la copie de la base live du 2026-11-17 (`orchestrator
rates-rebuild <copie> --guides-store <dossier>`, hôte chargé) :
- CPU par cycle : ~4,0 s au premier cycle du processus (mémos des
  données statiques à remplir), ~3,2 s ensuite, soit ~1 % d'un cœur à
  300 s ; ~49 000 lectures de rendement par cycle (une par activité et
  niveau utile), comptées dans `stats.yield_read` ; temps d'attente du
  marché et des quêtes tenus de façon incrémentale (0 ms par cycle sans
  nouveau rapport, 1,1 s pour un recomptage complet toutes les 12
  cycles) ; `cycle_ms`/`cycle_cpu_ms` servis avec chaque guide et ligne
  `guides: built …` dans `run/orchestrator.log` à chaque cycle ;
- RAM : le jeu ~60 Mo (59 Mio sérialisé), pic de construction +125 Mio
  de résident (mémo des sources des chaînes ; les rendements ne sont
  gardés qu'un niveau à la fois), deux jeux coexistent le temps qu'un
  lecteur relâche l'ancien ;
- lecture API : 60-90 ms pour une vue complète (lecture + JSON, 1-1,6
  Mo), quelques ms pour une vue filtrée par skill ; `held` réévalue les
  seules lignes recette (~50 ms).
- D22 (conditions et faisabilité par ligne, 2026-11-17, même copie) :
  cycle chaud 3,3-3,8 s CPU (avant 3,3-3,6 s, +3 % dans le bruit), résident du
  jeu +10 Mio (`MarketNeed` par objet acheté), lecture API 80-85 ms
  pour une vue complète (conditions et `feasibility` sérialisées, ~2 Mo),
  42 vues 81 Mio en JSON (avant 61).
Avant (requête calculée en ligne, mutex tenu pendant le calcul) : money
level=30 11 s, training level=30 21 s, money sans level 450 s, training
sans level 349 s, sur un cœur partagé avec la flotte.
