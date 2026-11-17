# Protocole et monde live (SDK uniquement)

Seul `rs/crates/sdk/` parle le réseau : `src/connection.rs` (session, trames),
`src/protocol.rs` (opcodes), `src/buffer.rs`, `src/isaac.rs`, `src/rsa.rs`,
`src/words.rs`. Monde live : `src/world.rs` (PNJ/décors/joueurs/objets, chat,
hint, side_flash), `src/snapshot.rs` (+ `src/client/snapshot_view.rs`),
`src/save.rs`, `src/tables.rs`. Les dumps typés viennent de
`lifescape-gamedata` (statiques compilées au build, voir `docs/gamedata.md`
si présent, sinon `rs/crates/gamedata/build.rs`) ; le SDK ne parse aucun
dump au runtime.

Inventaires : `world.inv` = inventaires transmis (interface ouverte) ;
`UPDATE_INV_STOP_TRANSMIT` (banque à chaque fermeture,
`bank.rs2 [label,closebank]`) déplace le contenu dans `world.inv_closed`
(dernier contenu connu, `World::inv_known`), conservé au relogin et amorcé
au login depuis la sauvegarde du compte (`save.rs`). Le heartbeat publie
la banque connue (`bank_streamed` = connue) : pièces et stock bancaires
restent visibles au cerveau et au registre du market entre deux visites.
`bot.bank_items()` ne lit que l'interface ouverte ; un contrôle « détenu »
de script (runes de la magie, arc/flèches du tir, outil en banque) lit la
banque connue (`kit::pack::banked_usable`/`banked_ids`, verrous market
déduits) comme le cerveau (2026-11-13 : bloqués en boucle alors que le
cerveau clôturait `already_held`). Un outil en banque lointaine : la
restauration marche jusqu'à la banque (`kit::tools::restore_banked_tool`).

Combat des PNJ (`src/engagement.rs`) : `UPDATE_PID` donne `world.self_slot` ;
les masques NPC_INFO FACE_ENTITY (0x4, `>= 32768` = joueur + 32768) et
dégâts (0x1/0x10) remplissent `NpcInfo.face_entity` / `hit_tick`.
`Bot::npc_engaged_by_other(slot)` = le PNJ cible un autre joueur, ou a pris
un coup depuis < 8 ticks sans cibler de joueur (miroir de « Someone else is
fighting that. », `player_combat.rs2`) : la chasse l'écarte avant la marche.
Le masque SPOTANIM des PNJ (0x40 : graphique u16 + hauteur/délai u32,
Client-TS `NpcUpdate.SPOTANIM`) est lu, plus sauté : `NpcInfo.spotanim` =
(graphique, tick vu) ; `World::npc_showed_graphic(slot, graphiques,
fenêtre)`. Une malédiction posée montre son impact à tous les clients
(`player_magic.rs2`, `gamedata::spells::impact_graphic` : confuse 104,
weaken 107, curse 110, bind 181) : le script `curse` écarte une cible
maudite par n'importe qui ou engagée par un autre joueur (tour 3, P7c),
et rend `contended:<site>` quand seules de telles cibles restent.

Joueurs visibles (`PLAYER_INFO`, `World::player_info`) : le serveur ne
suit un autre joueur qu'à 15 cases au plus (Chebyshev, `withinDistanceSw`,
`rsbuf/info.ts`) et n'envoie son bloc d'apparence (donc son nom) qu'une
fois par observateur et version d'apparence (`BuildArea.appearances`) ;
`world.player_names` garde le nom par slot pour les ré-entrées. Un
partenaire « invisible » alors que les cases convoquées sont adjacentes
n'est pas un défaut de décodage (2026-11-13, deals 1201/1248/1267/1268,
positions finales relevées) : l'un des deux avait frappé (`op_player4`,
le serveur suit la cible) un partenaire de passage vers sa banque et
s'était retrouvé à 16-28 cases, hors vue. Règle de rendez-vous
(`market_handover::meet_step`) : frapper seulement un partenaire immobile
depuis 3 ticks à 3 cases au plus de sa propre case, sinon attendre sur
(ou revenir à) sa case. La ligne d'attente journalise `debug_players()`.
Plusieurs groupes : un partenaire hors de vue depuis 100 ticks cède le
tour à un groupe suivant dont le partenaire attend déjà, immobile, sur
son point (`market_handover::serve_first`, voir `economy.md`).

Actions via les méthodes `Bot` (`src/client/actions.rs`, `src/client/movement.rs`,
`src/client/approach.rs`, `src/client/doors.rs`) : `op_loc`, `op_npc_t`,
`if_button`, `close`, `walk_to`, `take`, `approach`. Attentes :
`src/waits.rs` (`wait_ticks`, `wait_hitpoints`, `wait_backpack`,
`tick_ms()` : 60 ms en dev, 600 ms en prod via `NODE_TICK_RATE`). Erreurs : `src/error.rs` (`Error` typée : position, cible,
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

Modales : `IF_OPEN*` posent `world.modal`, `IF_CLOSE` l'efface
(`World::close_interfaces`). `Bot::close_modal` envoie `CLOSE_MODAL` puis
efface aussi localement, comme `Client.ts closeModal` : le serveur ne
renvoie aucun `IF_CLOSE` (2026-11-16 : la banque fermée avant un mélange
d'argile restait « ouverte » et le script lisait une interface fantôme).

Trames serveur (P11, 2026-11-17) : la table des tailles du SDK est testée
contre `../Client-TS` (branche 274, `src/io/ServerProt.ts`). `read_exact`
est tout-ou-rien (octets remis en tête du tampon sur timeout) et `recv`
garde la trame dont l'opcode est lu (ISAAC consommé) jusqu'à son corps
complet : un timeout au milieu d'une trame ne décale plus le flux.
« unknown server opcode » nomme les 8 dernières trames décodées.

Désynchronisation (tour 2, P2-9) : comme le client 274 (`tcpIn` →
`logout()` sur un type de paquet inconnu ou une erreur de décodage), un
opcode inconnu ou une erreur `Protocol` de décodage d'une trame ferme la
connexion aussitôt (`Connection::drop_desynced`) et rend `Io("protocol
desync, connection dropped: …")` : la session se reconnecte et reprend son
script depuis la position où le serveur a posé le bot (le trajet est
replanifié, arêtes custom comprises). Aucune lecture après l'erreur.
Cause racine (désynchronisation sur téléportation scriptée) non établie :
capture pktlog live nécessaire.
