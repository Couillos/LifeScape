# Navigation — état actuel et cible (phases 3-7)

État actuel : `Bot::walk_to` ne marche que sur un seul niveau (voir
`rs/crates/sdk/src/client/movement.rs`). La marche vit dans le SDK :
`rs/crates/sdk/src/collision.rs`, `bfs.rs`, `reach.rs`, `route.rs`,
`nav.rs`. Les transitions escaliers/échelles sont dans
`gamedata/data/transitions.json`, exploitées au cas par cas par les scripts.
Diagnostic sans marcher : script `debug_walk` (`run <compte> debug_walk <x> <z> [niveau]`).

Cible phases 3-7 (rien n'est implémenté) :
- phase 3 : crate `lifescape-nav` pure (collision statique, BFS/reach serveur,
  graphe), JSON unique, overrides versionnés dans `gamedata/data/` (nouveau fichier, à créer).
- phase 4 : `nav::plan` multi-niveau (marche + portes + transitions),
  routes en or, commande `orchestrator route [--ascii]`.
- phase 5 : `Bot::travel_to` qui exécute un plan segment par segment avec
  postcondition ; `walk_to` devient une enveloppe.
- phase 6 : dossier d'échec sous `run/` (nouveau, à créer) + `orchestrator replay-nav`.
- phase 7 : prober `probe_transitions` écrivant le fichier de vérification (nouveau, à créer).
