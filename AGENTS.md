# LifeScape

Framework Rust (stdlib uniquement, zéro dépendance) pour écrire des scripts
de botting sur notre instance LostCityRS (révision 274).
Toolchain : Rust stable local, `cargo` seul. Tout passe par le workspace `rs/`.
Référence canonique : `../lostcity/AGENTS.md`. Le serveur est la source de vérité.
Ticks serveur : voir `../lostcity/engine/data/config/world.json` ; attentes en ticks.

## 1. Projet

LifeScape pilote des bots sur notre serveur LostCityRS (rev 274) via des
scripts Rust supervisés par un orchestrateur (un worker de session par compte).
Le SDK parle le protocole réseau, gamedata expose les dumps du jeu, les scripts
restent minces et stateless. Ticks serveur : voir `../lostcity/engine/data/config/world.json`.

## 2. Carte des crates

- `lifescape-sdk` (`rs/crates/sdk/`) : protocole 274, monde live, actions, pathfinding ; ne supervise aucun bot.
- `lifescape-gamedata` (`rs/crates/gamedata/`, données `gamedata/data/`) : accès typé aux dumps ; jamais de réseau ni de script.
- `lifescape-orchestrator` (`rs/crates/orchestrator/`, binaire `orchestrator`) : daemon, sessions, CLI, dashboard ; jamais de protocole jeu.
- `lifescape-scripts` (`rs/crates/scripts/`) : registre compilé des scripts minces ; jamais de paquet ni d'opcode.
- `lifescape-json` (`rs/crates/json/`) : parseur/écrivain JSON unique, std only ; aucune logique métier.
- `lifescape-nav` (`rs/crates/nav/`) : collision statique, recherche locale, planification pure ; aucun réseau ni état live.
- `lifescape-planner` (`rs/crates/planner/`) : buts → graphe, cycle de vie des nœuds (`lifecycle.rs`, seul à changer un statut) ; pur.
- `lifescape-market` / `lifescape-store` : carnet d'ordres pur / persistance SQLite (exception D3).

## 3. Commandes

```sh
cargo build --manifest-path rs/Cargo.toml
cargo test --manifest-path rs/Cargo.toml --workspace
./rs/target/debug/orchestrator open <compte> | run <compte> <script> [args] | call <compte> <méthode> [args] | shell <compte>
./rs/target/debug/orchestrator status | show <compte> | see <compte> | logs <compte> [n] | unload <compte> | close <compte> | shutdown
./rs/target/debug/orchestrator route <x> <z> <niv> <x> <z> <niv> [--paid] [--ascii]
./rs/target/debug/orchestrator replay-nav <dossier>
```

## 4. Règles dures

1. Std only : aucun `[dependencies]`, JSON/HTTP/WS maison, pas de tokio/serde. Exception D3 : `lifescape-store` seul dépend de `rusqlite` (`bundled`), voir `docs/decisions.md`.
2. Pas d'`unsafe` : `#![forbid(unsafe_code)]` sur chaque crate, pas de code généré.
3. Noms explicites : jamais de variable, fonction ou module en 1-2 caractères.
4. Moins de 1000 lignes par fichier : découper par rôle avant la limite, API stable.
5. Attentes en ticks (`bot.world.ticks`, `waits::wait_ticks`, `waits::tick_ms()` : 60 ms en dev, 600 ms en prod via `NODE_TICK_RATE`), jamais en secondes ni `thread::sleep` d'attente jeu.
6. Erreurs typées et contextualisées (`sdk::error::Error`), remontées via `Result` ; jamais d'`unwrap` sur un chemin réseau/jeu.
7. Aucun fallback non demandé : en échec, erreur typée puis question à l'utilisateur.
8. Scripts minces et stateless : logique skill uniquement, réutilisable → SDK, données → gamedata, aucun état persisté.
9. Protocole dans le SDK seulement : aucun script ne touche `Connection`, trames ni opcodes.
10. Serveur = vérité : `../lostcity/engine` + `../lostcity/content` (rev 274) ; lecture/dump autorisés, modification seulement sur demande explicite. Client JS de référence (arbitrage protocole/contenu, lecture seule) : `../Client-TS` (branche 274, `src/client/Client.ts`, opcodes `src/io/ClientProt.ts|ServerProt.ts`) — le consulter en cas de doute paquet/modale/séquence, avant tout fix SDK.
11. Comptes : `admin` pour les tests de dev, jamais `bite` (personnel) ; `t01`… et nommés = runs légitimes.
12. Aucun cheat en code commité (`bot.cheat`, `::...`) hors outils dev autorisés (`give`, `goto`, `probe_*`, `debug_walk`, `check_*`).
13. Économie propre : `admin` ne transmet jamais à un autre compte un objet ou de l'argent obtenu en trichant (`give`, `giveother`, échange, dépôt au sol, ordre au marché) ; ce qu'il triche reste chez lui, pour ses tests.

## 5. Protocole debug

1. Reproduire : obtenir un dossier d'échec ou une route qui échoue.
2. Classer : `replay-nav` → A plan faux (bug nav) | B monde ≠ carte (donnée manquante) | C exécution (bug SDK) ; ne lire que la couche indiquée.
3. Écrire une seule hypothèse avant de toucher au code, avec l'observation qui la confirmerait ou l'infirmerait.
4. Vérifier l'hypothèse (code serveur, `see`/`call`, route hors ligne) avant toute modification.
5. Écrire le test qui échoue d'abord (route en or ou test unitaire), puis corriger un seul crate.
6. Deux essais ratés → arrêt, rapport (sorties exactes + hypothèse suivante), attendre l'utilisateur.

## 6. Index docs

- Nav/planification → `docs/navigation.md` ; paquets/monde live → `docs/protocol.md`.
- Daemon/sessions/CLI/dashboard → `docs/orchestrator.md` ; dumps/accès typés → `docs/gamedata.md`.
- Tests live et unitaires → `docs/testing.md` ; choix d'archi → `docs/decisions.md`.
- `docs/journal/` = archives : ne jamais lire sauf demande explicite. `../lostcity/AGENTS.md` = canonique.
