# Décisions d'architecture

- 2026-11-06 — Registre unique via `lifescape_scripts::names()` : une seule source scripts. Rejeté : double registre orchestrateur (chemins `.py` périmés).
- 2026-11-06 — `dump-transitions` porté en sous-commande Rust fidèle, test de comparaison sémantique. Rejeté : garder le générateur Python (deux codebases).
- 2026-11-06 — Données rapatriées dans `gamedata/data/`, chemins lus via `paths::root()`. Rejeté : laisser le Rust lire le paquet Python.
- 2026-11-06 — Python supprimé, gardés `orchestration/static/` (servi par `web.rs`) et `context/`. Rejeté : suppression totale (casse le dashboard et les notes).
- 2026-11-06 — Std only + `#![forbid(unsafe_code)]` par crate, JSON/HTTP/WS maison. Rejeté : tokio/serde (poids et GC/process inutiles ici).
- 2026-11-06 — Attentes jeu en ticks (`TICK_MS = 20`), garde-fous process en secondes. Rejeté : secondes partout (comportement différent à 20 ms vs 600 ms).
- 2026-11-06 — Parseur JSON unique (`lifescape-json`), gamedata et `devtools_check` branchés dessus. Rejeté : trois parseurs ad hoc (divergence silencieuse).
- 2026-11-06 — `buffer.rs` (Reader/Writer) déménagé dans `lifescape-nav`, le SDK le réexporte : un seul lecteur d'octets. Rejeté : dépendance nav→sdk (cycle) et duplication du lecteur.
- 2026-11-06 — Override 1530 supprimé : live prouve portes 1530 normales (reach suffit), aucune entrée requise ; §7.15 couvert par route `rempart_1530` + ce motif. Rejeté : surcharger sans donnée live.
- 2026-11-06 — Péage PaidOnly synthétique validé : Al-Kharid contournable à pied (détour 220 tuiles via `route`), branche couverte par `paid_only.rs`. Rejeté : or PaidOnly réel (aucun péage isolé en F2P).
- 2026-11-06 — Exemption budget 300 ms pour NoRoute-par-épuisement : `chateau_scell` met 3669 ms à prouver Disconnected (release) ; budget = plans réussis (max 140 ms). Rejeté : borner la preuve d'échec.
- 2026-11-06 — Coûts 10/20/50 gardés : DOOR 10, TRANSITION 20, UNVERIFIED 50 ; calibration live reportée phase 7 (prober). Rejeté : calibrer sans mesures d'arrivée.
- 2026-11-06 — Bornes d'exécution mesurées live : DOOR_SWING_WAIT 23 (swing vu à 17 ticks), RIDE_WAIT 41 (atterrissages 2-9, 26 max en live), CROSS_WAIT 7 (pas 1-4), PRE_OPEN_WAIT 17 (couvercle souvent déjà ouvert, calibration prober phase 7). Rejeté : bornes rondes sans source.
- 2026-11-06 — STALL 13 : 19 jambes live à 0,6-6 ticks/tuile (médiane 2), 13 = silence > 2x pire écart + marge pompe. Rejeté : seuil rond (10/15 sans source).
- 2026-11-06 — Budget temporel cost*4+347 ticks : 4 = 2x médiane marche, 347 = file portes + stalls ; aucun Timeout sur les runs live phase 5. Rejeté : budget fixe (ordres de grandeur différents).
