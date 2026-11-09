# Protocole et monde live (SDK uniquement)

Seul `rs/crates/sdk/` parle le réseau : `src/connection.rs` (session, trames),
`src/protocol.rs` (opcodes), `src/buffer.rs`, `src/isaac.rs`, `src/rsa.rs`,
`src/words.rs`. Monde live : `src/world.rs` (PNJ/décors/joueurs/objets, chat,
hint, side_flash), `src/snapshot.rs` (+ `src/client/snapshot_view.rs`),
`src/save.rs`, `src/tables.rs`. Les dumps typés viennent de
`lifescape-gamedata` (statiques compilées au build, voir `docs/gamedata.md`
si présent, sinon `rs/crates/gamedata/build.rs`) ; le SDK ne parse aucun
dump au runtime.

Actions via les méthodes `Bot` (`src/client/actions.rs`, `src/client/movement.rs`,
`src/client/approach.rs`, `src/client/doors.rs`) : `op_loc`, `op_npc_t`,
`if_button`, `close`, `walk_to`, `take`, `approach`. Attentes :
`src/waits.rs` (`wait_ticks`, `wait_hitpoints`, `wait_backpack`,
`TICK_MS = 20`). Erreurs : `src/error.rs` (`Error` typée : position, cible,
état observé, ticks ; `Travel("nav-down: …")` = service de navigation
injoignable ou refusant, jamais reconnect ni rescue).

Voyage (`src/client/travel.rs`) : `travel_to` planifie via le service de
navigation puis exécute segment par segment avec postcondition ;
`walk_to` est l'enveloppe à budget en ticks. Plan distant uniquement
(`submit` non-bloquant + `poll` avec keepalive entre les polls, 1 resubmit
anti-skew) ; `NoRoute` devient `Error` avec le diagnostic du planificateur.
Exécution : portes d'après le plan (op, péage, traversée), rides d'après
le verdict servi, replan via le service avec l'arête bannie, dossier
d'échec sur échec (`src/client/travel_dossier.rs`). Coûts de marche par
batch distant (`walk_costs`, `None` = ordre Manhattan conservé).

Règles : un script ne manipule jamais `Connection` ni un id d'opcode ; toute
fonctionnalité ajoutée au SDK doit servir n'importe quel script. En doute
protocole : vérifier dans `../lostcity/engine` puis valider en live.
