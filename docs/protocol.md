# Protocole et monde live (SDK uniquement)

Seul `rs/crates/sdk/` parle le réseau : `src/connection.rs` (session, trames),
`src/protocol.rs` (opcodes), `src/buffer.rs`, `src/isaac.rs`, `src/rsa.rs`,
`src/words.rs`. Monde live : `src/world.rs` (PNJ/décors/joueurs/objets, chat,
hint, side_flash), `src/snapshot.rs` (+ `src/client/snapshot_view.rs`),
`src/save.rs`, `src/tables.rs`.

Actions via les méthodes `Bot` (`src/client/actions.rs`, `src/client/movement.rs`,
`src/client/approach.rs`, `src/client/doors.rs`) : `op_loc`, `op_npc_t`,
`if_button`, `close`, `walk_to`, `take`, `approach`. Attentes :
`src/waits.rs` (`wait_ticks`, `wait_hitpoints`, `wait_backpack`, `TICK_MS = 20`).
Erreurs : `src/error.rs` (`Error` typée : position, cible, état observé, ticks).

Règles : un script ne manipule jamais `Connection` ni un id d'opcode ; toute
fonctionnalité ajoutée au SDK doit servir n'importe quel script. En doute
protocole : vérifier dans `../lostcity/engine` puis valider en live.
