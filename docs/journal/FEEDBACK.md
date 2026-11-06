# Archive. Ne pas utiliser comme référence.

# LifeScape — retour d'expérience : tutoriel de bout en bout + SDK

Le tutoriel de l'île des tutoriels (rev 274) passe **de bout en bout en live** :
comptes t46 et t48 téléportés à Lumbridge (`done (0, 3222, 3222)`),
pilotés uniquement par ce que le serveur envoie au client.

## Détection d'étape (ne plus jamais deviner)

- Le varp de progression (`tutorial` = 281, `transmit=false`) n'est JAMAIS
  transmis : `world.varps` ne le verra jamais.
- Signal solide : le **texte d'instruction** (coms 6180-6184, réécrit à
  chaque étape), croisé avec la **flèche** (`HINT_ARROW` → `world.hint`,
  PNJ ou tuile) et l'**onglet qui clignote** (`TUT_FLASH` → `world.side_flash`).
- L'**inventaire ne pilote rien** : il prouve seulement ce que le joueur
  possède. Chaque étape vérifie son propre matériel avant d'agir
  (ex. : refondre s'il manque le minerai, re-parler au PNJ s'il manque
  les runes) et l'étape en cours est rejouée jusqu'à changement d'instruction.
- Reprise à froid : l'instruction peut manquer juste après un login ; le
  run attend son arrivée (~10 s), retente un clic d'onglet (qui rejoue le
  proc et ré-affiche l'étape), et en dernier recours rejoue la dernière
  étape connue (cache local `/tmp/opencode/stage_<compte>.json`, alimenté
  par les instructions déjà vues, jamais par l'inventaire).

## Ce qui a été ajouté au SDK pendant le travail

- `Bot.op_held_u(...)` + test byte-exact. Prouvé en live : feu (tinderbox
  sur bûches), pâte (pot de farine sur seau d'eau).
- `Bot.relogin()` avec retries (le serveur purge la session en ~15-30 s,
  sinon `login refused: 5`).
- `World.hint` : parse `HINT_ARROW` (1 = PNJ, 2-6 = tuile, 10 = joueur,
  -1 = effacer), validé sur trames live.
- `World.side_flash` : parse `TUT_FLASH` (opcode 90, id d'onglet).
- Parsing NPC : `CHANGE_TYPE` (0x20) appliqué, tailles `DAMAGE2` (0x1) =
  4 octets et `SPOT_ANIM` (0x40) = 6 octets, ordre des masques aligné sur
  `npcInfoProtIndex` / `playerInfoProtIndex`, boucles calquées sur le vrai
  client (`> 21 bits` NPC, `> 10 bits` joueurs).
- `REBUILD_NORMAL` ne vide plus les listes d'entités (le serveur ne vide
  son build que sur téléportation > 15 tuiles ou changement de niveau) ;
  vidage symétrique ajouté sur téléportation du joueur.
- Patterns côté script (pas encore remontés au SDK) : `cross_door`
  (clic puis dash sans re-cliquer, cooldown 30 s par porte),
  `op_loc_near` / `use_held_on_loc_near` (essai multi-tuiles avec
  vérification, indispensable : fourneau 3x3, enclume, rochers, feux),
  `_talk_from_best_tile` (dialogue exigé depuis une tuile adjacente
  précise), détection "can't reach that".

## Ce qui manque encore (par ordre d'importance)

1. **Verbes "utiliser sur" à monter dans `Bot`** : `OPLOCU`, `OPNPCT`
   (layout validés live, ex. com `magic:wind_strike` = 1152) avec tests
   byte-exact. Aujourd'hui construits à la main dans `common.py`.
2. **Ids de composants codés en dur** (`player_kit:accept` = 3651,
   `controls:com_5` = 153, `magic:wind_strike` = 1152,
   `smithing:column1` = 1119) : les ajouter au dump `gametypes.json`
   ou offrir `Bot.com("if:comp")`.
3. **Désynchro PNJ non élucidée** : après ~5-15 min de session avec
   téléportations, certains PNJ disparaissent du suivi client alors que
   le serveur les a (prouvé par double-bot côte à côte). Parsing vérifié
   bit à bit contre vrai client + encodeur serveur, sans trouver l'écart.
   Contournement : `talk()` re-logue en dernier recours (resync garanti).
4. **Pas de logout propre** : fermer le socket suffit, reconnexion refusée
   ~30 s (`logoutRequests`). `relogin()` gère l'attente.
5. **`MAP_BUILD_COMPLETE` (214) jamais envoyé** : aucun effet observé, à
   ajouter par fidélité.
6. **Portes à fermeture auto (~1-2 s) et portes-téléporteurs à ping-pong** :
   le pattern `cross_door` devrait entrer dans le SDK.
7. **`find_loc` par substring** : pas de "porte praticable la plus proche"
   ni d'empreinte exposée pour les locs non-muraux.
8. **Bug contenu côté serveur (pas SDK)** : le switch `set_tutorial_progress`
   n'a pas de case `tutorial_opened_magic_tab` (630) → affiche "Oops!
   Unhandled Tutorial Step". Transient dans le flux correct (clic onglet
   puis dialogue runes), mais visible tel quel par un vrai joueur qui
   clique l'onglet magie.
9. **Wind Strike silencieux par rafales (suspect moteur, non prouvé)** :
   `OPNPCT` accepté (pas de `UNSET_MAP_FLAG`, mauvais slot rejeté),
   mais de longues séries sans conso de runes, sans XP, sans message,
   puis la progression arrive d'un coup (splash 650, kill 660 validés
   live). Candidat : portée / ligne de vue à travers la clôture du
   poulailler dans la validation d'interaction. Le contournement actuel
   (re-cast en boucle jusqu'à mort du poulet) finit par passer.
10. **Quirk contenu** : à l'étape 195, `tutorial_step_enable_run` fait
    `return` précoce via `p_finduid`, donc le texte "Running." ne s'affiche
    jamais (l'instruction précédente reste). Le bot appuie sur run quand
    même et l'étape 200 suit.

## Pathfinding : état des lieux

- BFS (`route.py`) différentiellement identique au serveur (30/30 chemins
  + tous les flags) ; collisions (`collision.py`) du même build.
- Le vrai coût est ailleurs : portes à fermeture auto, tuiles adjacentes
  refusées par le serveur, PNJ "invisibles". Le routeur ne peut rien y
  faire seul.
- `walk_to` gagnerait à re-planifier après chaque jambe au lieu d'exécuter
  un plan figé face à un monde qui change (portes qui se referment).

## Script tutoriel (un rôle par module)

- `scripts/tutorial/common.py` — ids, lookups, attentes, dialogue,
  portes, détection d'étape (`instruction`, `detect_stage`).
- `scripts/tutorial/north.py` — design, guide, survie, chef, course.
- `scripts/tutorial/mine.py` — quête, mine, fonte, forge.
- `scripts/tutorial/combat.py` — équipement, mêlée, distance.
- `scripts/tutorial/south.py` — banque, chapelle, magie, continent.
- `scripts/tutorial/run.py` — boucle détection → action : `run.py [username]`.

## Trouvé via la boucle gamedata (objets au sol)

- `World.ground_items` fusionnait le template statique du pack (jamais
  retiré) avec l'état live : après un ramassage (`OBJ_DEL` appliqué en
  `None`), l'entrée statique masquait la suppression et le bot voyait
  un objet fantôme. Corrigé : l'état live prime sur le template
  (test `test_live_ground_shadows_static_template`).
- `Bot.take()` (et les boucles `wait_inv`) concluent au succès sur
  simple delta d'inventaire : un sync d'inventaire au login ou un
  refus serveur (`UNSET_MAP_FLAG`, "can't reach that", "don't have
  enough inventory space") passent inaperçus, et 28 slots pleins
  ressemblent à un bug de respawn. Recommandation : fail fast sur les
  rejets explicites au lieu du timeout, et toujours stabiliser la
  baseline avant d'affirmer un changement d'état.
