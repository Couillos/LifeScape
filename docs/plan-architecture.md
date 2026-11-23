# Plan d'architecture et de CPU — version arrêtée (2026-11-24)

Source : `docs/analyse-architecture-2026-11-24.md` (diagnostic mesuré, options, raisons). Ce document fixe **ce qu'on fait**, dans quel ordre, et comment on le prouve. Les numéros de lots renvoient à l'analyse (sections 3.3, 4 et 5.2).

## 1. Décisions de l'utilisateur

| # | Question | Décision |
|---|---|---|
| 1 | Durabilité SQLite | **On garde `synchronous=FULL`.** On regroupe seulement les écritures (transactions groupées, écrivain unique). |
| 2 | Rafraîchissement du tableau de bord | **On garde 1 seconde.** On rend la trame moins chère (lot 1.2), pas plus rare. |
| 3 | Accumulations | **Oui** : arrêter `run/lifescape-staging.jsonl` et le supprimer ; purger `run/failures/` au-delà de **7 jours** ; purger les ordres **annulés** au-delà de **14 jours** (et leurs `order_amends`). Les ordres ouverts, appariés et réglés, les échanges et les exécutions sont gardés. La rétention des ordres doit rester plus longue que la plus longue fenêtre de lecture qui les utilise (apprentissage vendeur `horizon`, critères marché) : la vérifier avant de purger. |
| 4 | Réactivité (lots 4.3, 4.4) | **On mesure en vrai** : ce ne sont pas des lots neutres, ils sont jugés sur la mesure (XP/h, ticks de travail, erreurs par type). |
| 5 | Taille de flotte | **74 bots.** Pas d'optimisation spécifique « 300 bots ». |
| 6 | Connexion directe | **Oui** : faite le 24/11 (correctif serveur `TcpClientSocket.send` copie le paquet ; 0 désynchro en direct). |
| 7 | Itinéraires identiques à l'octet près | **Règle dure.** Pas d'ALT, pas de hiérarchique. |
| 8 | Guides incrémentaux | **Non** (rang 16 retiré). |
| 9 | Un processus par bot (E7) | **Confirmé.** |

Retirés du périmètre : rang 16 (guides incrémentaux), rang 18 (fait), tout changement de `synchronous`, toute modification de l'horizon `walk_costs` ou du plafond des poches.

## 2. Règles communes à tous les lots

- **Un lot = un commit** sur la branche `plan-architecture`, message en français. La branche n'est fusionnée dans `main` qu'après la vérification d'une heure en vrai.
- **Neutre sauf mention** : mêmes décisions, mêmes lignes de journal, mêmes libellés `cpu_profile`. Exceptions déclarées : 4.3, 4.4, et la purge (décision 3).
- **Preuve hors ligne par les chemins de production** (analyse 5.3) : les bancs partent du monde brut (`Job` avec état vivant JSON, base de test, marché de test) et appellent les mêmes fonctions de lecture que la production. Aucun champ d'entrée rempli seulement par un test.
- **Compilation** : jamais `cargo build --release` pendant que la flotte tourne (le binaire `rs/target/release/orchestrator` est celui que les ouvriers relancent). Itérer avec `--profile release-test` (incrémental) : `RUST_TEST_THREADS=6 nice -n 19 cargo test --profile release-test --manifest-path rs/Cargo.toml …`. Le build release se fait au déploiement.
- **Données vivantes** : `run/` en lecture seule pendant le développement (`sqlite3 "file:run/lifescape.db?mode=ro"`) ; tests isolés de `run/` (feature `test-isolation`, ne pas la défaire).
- **Repowise d'abord** pour toute question sur le code ; `get_risk` avant d'éditer un fichier.
- **Docs** : une ligne datée par décision dans `docs/decisions.md` ; doc du module concerné à jour.

## 3. Pistes et ordre

Deux pistes en parallèle, une seule fusion.

### Piste N — navigation (copie de travail séparée, crate `nav`)

N0 mesurer → N1 un seul lecteur du monde → N2 grille dense → N3 `dist`/`prev` denses → N4 caches gardés entre redémarrages → N5 composantes connexes (seulement si N0 le justifie après N2/N3) → N6 découpage de `Service` (+ cache `walk_costs` selon N0) ; test de caractérisation de `reach_wall`.
Preuve : goldens, rejeu des 8 497 requêtes réelles (`service_replay_tests.rs`) identique, CPU du rejeu avant/après.

### Piste D — démon, cerveau, ouvriers (dépôt principal, lots en série)

1. **Phase 0** : 0.1 script de référence et relevé CPU par groupe de fils dans le dépôt ; 0.2 profil complet des fils du démon, étiquettes manquantes du cerveau, durée d'un tour du superviseur, délai rapport → prise en compte.
2. **Phase 1** : 1.1 `brain_trade` via `Holdings::of` ; 1.2 trame du tableau de bord sans copie, partagée entre onglets (toujours 1 s) ; 1.3 comptage d'octets, `holdings-sampler`, `clear_event_if_gone` ; 1.4 fin de `lifescape-staging.jsonl` ; 1.5 état vivant en `Arc` ; **+ rétention** (décision 3) : purge `run/failures/` > 7 jours et ordres annulés > 14 jours, par une tâche périodique du démon et une commande opérateur (`--check` d'abord).
3. **Bancs** (analyse 5.3) : retirer `..LaunchInputs::default()` et `Default` côté production (`LaunchInputs`, `ReadyCtx`, horloge d'entretien) ; bancs de lancement et d'entretien refaits pour partir du monde brut.
4. **Phase 3** : 3.1 vue de passe ; 3.2 vue du marché partagée par génération ; 3.3 tirage retenté seulement si ses entrées changent (preuve : double exécution journalisée) ; 3.4 mémos purs ; 3.5 `BrainDeps`.
5. **Phase 4** (non neutre, jugée en vrai) : 4.1 un fil par connexion de battement ; 4.2 phase/statut lus dans les trames (comparaison muette avec `status`) ; 4.3 le rapport réveille le cerveau ; 4.4 tick de sécurité cadencé par le cerveau ; 4.5 `status` seulement sur silence.
6. **Phase 5** : 5.1 à 5.10 (frontières typées, étape C `Autopilot` en 5 commits, étape D coupe d'`orch-stats`, opérations typées, `dijkstra`/`handle_get`/`handle_report` découpées, **écrivain unique de la base avec écritures groupées** (décision 1), canal ouvrier durable, configuration typée). Le 5.6 (navigation typée, `market_eta` en direct) attend la fusion de la piste N.
7. **Phase 6** : ouvriers (trace sans copie par pompage, cache de banque sur changement, lecture de socket par blocs).

## 4. Vérification, déploiement, mesure

1. **Face au plan** : chaque lot est relu contre ce document (contenu, preuve, neutralité, docs). Écart → correction avant le lot suivant.
2. **Avant déploiement** : fusion des pistes, suite complète `--profile release-test --workspace --no-fail-fast`, contrôle des couches, `git status` propre, puis `cargo build --release`.
3. **Déploiement** (procédure `docs/orchestrator.md`) : arrêt propre du démon, relance avec `LIFESCAPE_SERVER_HOST=<IP du conteneur lostcity>`, ouverture/`redeploy` des 74 comptes un par un (les ouvriers changent : `sdk`, `orch-session`).
4. **Mesure après 1 heure** contre la référence lue dans la base (fenêtre de 1 h avant le déploiement) : rapports/h et parts `goal_reached`/`blocked`/`failed`, ticks de travail, XP/h, erreurs par type, désynchros, délai rapport → prise en compte, CPU par groupe de fils (démon, ouvriers, navigation). Régression → correction et nouveau déploiement.
5. Fusion de `plan-architecture` dans `main` après une mesure positive.
