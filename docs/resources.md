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
