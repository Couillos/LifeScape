# Décisions d'architecture

- 2026-11-06 — Registre unique via `lifescape_scripts::names()` : une seule source scripts. Rejeté : double registre orchestrateur (chemins `.py` périmés).
- 2026-11-06 — `dump-transitions` porté en sous-commande Rust fidèle, test de comparaison sémantique. Rejeté : garder le générateur Python (deux codebases).
- 2026-11-06 — Données rapatriées dans `gamedata/data/`, chemins lus via `paths::root()`. Rejeté : laisser le Rust lire le paquet Python.
- 2026-11-06 — Python supprimé, gardés `orchestration/static/` (servi par `web.rs`) et `context/`. Rejeté : suppression totale (casse le dashboard et les notes).
- 2026-11-06 — Std only + `#![forbid(unsafe_code)]` par crate, JSON/HTTP/WS maison. Rejeté : tokio/serde (poids et GC/process inutiles ici).
- 2026-11-06 — Attentes jeu en ticks (`TICK_MS = 20`), garde-fous process en secondes. Rejeté : secondes partout (comportement différent à 20 ms vs 600 ms).
