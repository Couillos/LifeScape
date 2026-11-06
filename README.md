# LifeScape

Framework Rust (stdlib uniquement, zéro dépendance) pour écrire des scripts
de botting sur notre instance LostCityRS (révision 274).

Toolchain : Rust stable, `cargo` seul. Layout `rs/` (workspace) :

- `crates/sdk/` (`lifescape-sdk`) : protocole 274, monde live, pathfinding,
  actions, erreurs typées.
- `crates/gamedata/` (`lifescape-gamedata`) : dumps typés du jeu
  (`gamedata/data/*.json`).
- `crates/orchestrator/` (`lifescape-orchestrator`, binaire `orchestrator`) :
  daemon, workers de session, CLI, dashboard HTTP/WS.
- `crates/scripts/` (`lifescape-scripts`) : trait `Script` + registre compilé
  des scripts (`woodcut`, `combat`, `trade`, `bank`...).

Instructions canoniques : `AGENTS.md` (+ `../lostcity/AGENTS.md`).

```sh
cargo build --manifest-path rs/Cargo.toml
cargo test --manifest-path rs/Cargo.toml
./rs/target/debug/orchestrator open <compte>
./rs/target/debug/orchestrator run <compte> woodcut 1
./rs/target/debug/orchestrator status | show <compte> | logs <compte> 40
./rs/target/debug/orchestrator unload <compte> | close <compte>
./rs/target/debug/orchestrator shutdown
```

Dashboard : http://5.83.152.180:8765/ (état live + spectate).
Tout passe par l'orchestrateur : un worker par compte, un job par compte,
relogin + reprise auto. Ne jamais lancer de bot en direct.

## Règles à connaître

- Attentes en ticks serveur, jamais en secondes ni `thread::sleep`
  (le serveur tourne en ticks de 60 ms en dev).
- Scripts stateless : adaptés à toute position, inventaire et banque.
- Aucun cheat dans le code commité ; compte `admin` réservé aux tests live.
- Serveur de jeu sur ports localhost uniquement (`127.0.0.1:43594`).

## Diagnostic réseau

`LIFESCAPE_PKTLOG=1` (exporté avant `open`, le daemon redémarre avec)
trace chaque paquet `C>S` / `S>C` (nom, opcode, taille, horodatage) dans
le log de session. `LIFESCAPE_SERVER_HOST`/`LIFESCAPE_SERVER_PORT`
choisissent le serveur (défaut `127.0.0.1:43594`).

## Données

Régénérer après mise à jour du serveur/jeu :

```sh
cd ../lostcity/engine && npx tsx tools/lifescape-dump.mts ../../LifeScape/lifescape/data/gametypes.json
```
