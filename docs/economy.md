# Économie (lot L5 : market + simulateur)

But : prix issus de l'offre et de la demande entre bots, rendements mesurés
en continu, visibles au dashboard et dans une base interrogeable.

## Market

Double enchère continue, un carnet par objet échangeable F2P : priorité prix
puis ancienneté, exécutions partielles au prix de l'ordre au carnet, modifier
= annuler + replacer (perte d'ancienneté). Code pur dans
`rs/crates/market/` : `book` (carnet + appariement), `guards` (bandes +
couverture), `deals` (machine à états), `price` (VWAP → mid → magasin,
bougies OHLCV), `trends` (signaux publics), `zip` (cotation + plafonds
marchands), `sim` (simulateur à graines fixes).

Stratégie de cotation inspirée de ZIP (PRD3, demande 2026-11-13) : un
ordre posé avec un `ceiling` (acheteur) ou un `floor` (vendeur) fait un
pas de 5 % (≥ 1 pièce) par fenêtre de patience (200 ticks) tant qu'il
reste au carnet (`market_reprice`, `lifescape_market::demand_step`), sa
direction lue dans la fenêtre : vente servie (même en partie) → monte,
vente au repos → descend jusqu'à son plancher, achat au repos → monte
jusqu'à son plafond, achat servi → redescend vers son prix d'ouverture.
Acheteur : ouvre à l'ask s'il le couvre, sinon à la référence
(`opening_bid`). Vendeur : ouvre à la référence, ou au meilleur bid s'il
est plus riche (`opening_ask`, plus de marge de 10 %), plancher moitié
de la référence échangée, 1 sinon (jamais un coût d'intrant). Une vente
monte sans plafond propre (la bande 5× d'une référence échangée la
borne). Voir § Prix par l'offre et la demande. Les PNJ n'ancrent plus rien : aucun bot ne
vend à un marchand, et l'achat marchand est réservé aux objets non
productibles en F2P (D-A, voir `decisions.md`). Marchands (`market:flip`, ~10 %) :
sélection sur écart/volume, plafonds `max_capital_share` (10 %),
`max_stock_items` (500), `max_book_share` (25 % du côté).

Couverture vérifiée à l'ordre (objet possédé, pièces disponibles, registre
en mémoire ; les holdings live des connectés se brancheront sur le même
contrôle). Bande anti-bug : rejet au-dessus de 5× la référence, achat
rejeté sous 0,2×, ventes jusqu'à 1 pièce. Deals livrés physiquement place
de Varrock (anneau autour de la fontaine, voir § Place ci-dessous) :
`matched → convened(case d'échange, échéance) → both_arrived →
trading → settled | failed{no_show|mismatch|death|timeout|nav_down}`.
Échéance = 2× trajet ETA max + marge (ticks) ; départs échelonnés par
ETA nav (cache par zone de départ, un plan par zone) pour une arrivée
simultanée ; regroupement 300 ticks (une convocation par bot, paires
regroupées en un seul échange) ; une convocation = case + heure de
départ seulement (jamais la position de l'autre). Les ticks sont des
époques par login : départs et échéances sont exprimés dans l'horloge
de chaque bot, le balayage des expirés sur l'horloge murale du daemon
(mesure live du tick, sans interrupteur dev/prod). Le registre règle à
l'appariement ; un `failed` ultérieur rembourse le transfert, replace
la partie présente **avec sa priorité** (seq d'origine conservée) et ne
touche la fiabilité qu'en cas de faute (absent, refuseur ; mort,
timeout, nav_down : neutre). Sans nav : aucune convocation,
appariements conservés, expirations en `nav_down` sans pénalité.

Tout deal se termine (2026-11-13) : `settled` seulement sur le message
serveur « Accepted trade. » (fenêtre fermée sans lui = refus/départ du
partenaire, `ExchangeOutcome::Declined`) ; un worker qui meurt après
`trading` rapporte quand même (acceptation vue → settled, sinon
`timeout`) et le balayage expire aussi `trading` ; un compte reste
convoqué tant qu'un deal de sa convocation est en vol ; chaque côté
interroge le daemon toutes les 100 ticks (un partenaire `short` libère
l'autre aussitôt). Partie absente d'un deal `matched` (aucune session) :
le daemon l'ouvre (`open_job`, admission et cadence respectées, une à la
fois, `market_absent.rs`) ; compte staff/privé, ouverture refusée ou
absence > 3000 ticks → `no_show`, ordres de l'absent annulés, côté
honnête replacé avec sa priorité.

## Place de Varrock (échange physique)

Centre : fontaine (3213, 3429, niveau 0), bassin x 3211-3215 / z 3427-3431.
Anneau calculé au démarrage du market service via l'op `tiles_flags`
(cases marchables autour du bassin, hors bustes (3210,3429), (3213,3426),
(3213,3432), (3216,3429) et étals (3220,3425), (3220,3431)) ; banques
`lifescape_gamedata::banks` (Varrock ouest x 3186 / z 3436-3446, Varrock
est z 3419 / x 3252-3256). Chaque deal reçoit une paire de cases
adjacentes libre, répartie autour de la fontaine ; 2e anneau si plein,
puis file d'attente. Rendez-vous : attente jusqu'à l'échéance, le plus
petit id de compte envoie la demande. Échange SDK `trade_exchange` :
billets (`bank_main:com_93` = notes, `com_94` = items) pour les gros
lots, redépôt après l'échange, les DEUX écrans vérifiés avant accept.

Référence (PRD3) = VWAP 24 h ; sinon dernier print ; sinon milieu du
carnet ; sinon **1 pièce** (jamais le prix magasin). La bande anti-bug ne
s'applique qu'à une référence issue de vraies transactions. Bougies OHLCV en horloge (1 h / 7 jours,
1 jour / 6 mois et plus), recalculables depuis `executions` (source de
vérité), tick serveur stocké avec chaque exécution. Tendances calculées
uniquement sur données publiques (profondeur du carnet + exécutions :
pénurie, momentum, score lissé borné) ; aucun état d'un autre bot n'y
entre jamais.

Service daemon `orchestrator/src/market_service.rs` (+ `market_seed.rs` :
catalogue F2P + historique de démonstration) : socket `run/market.sock`
(JSON-lines, une requête une réponse), API `quote/book/history/trades/
list/place/amend/cancel/my_orders/my_deals/deal_arrived/deal_report/fund/
tick/stats` ; persistance via `lifescape-store` (ouvert par le daemon
seul). Reliquat connu : les ETA de convocation passent par une constante
d'horloge, pas par le service de navigation ; les holdings live ne sont
pas encore branchées au registre.

## Simulateur

Graines fixes, agents ZIP synthétiques (offres/demandes tirées des
catalogues). Mesuré : convergence des prix à ±10 % de l'équilibre en 2
appariements sur le scénario symétrique (12 acheteurs / 12 vendeurs) ;
choc d'offre (+6 vendeurs au rabais) sans oscillation entretenue (16
inversions sur 120 appariements, prix final dans la bande) ; deux
marchands resserrés sur carnet clairsemé : 124 appariements contre 93
sans eux (+33 %).

## Rendements

Chaque fin de tâche produit une ligne par objet dans `task_reports` /
`task_yields` : `{script, méthode, objet, provenance, quantité, ticks,
contexte}`, où le contexte est le niveau du skill principal (bande de
5 niveaux), le tout horodaté en ticks. Seuls les flux de farm comptent
dans un rendement : produits de skill, drops, spawns fixes au sol et
objets ramassés, provenance étiquetée (`produced`, `loot`,
`ground_spawn`, `picked`). Les flux de boutique et d'échange
(`shop_bought`, `traded`) sont stockés bruts mais exclus des taux ; les
`consumed` (appâts, nourriture) nourrissent le coût des intrants.

Ingestion : la session pousse le rapport, le superviseur du daemon le
verse dans le store (`jobs::ingest_report_into`) puis dans l'agrégat
persisté (`rates_service`, `run/rates-snapshot.json`, rechargé au
redémarrage : aucune mesure ne se perd avec le process). Les lignes
brutes restent requêtables pour recouper plus tard.

Agrégation : rendement normalisé par 6000 ticks, par triplet (méthode,
objet, tranche de 5 niveaux), avec n, moyenne, écart-type (population)
et moyenne mobile exponentielle (alpha 0,3, ordre d'arrivée). Mêmes
mesures pour l'XP par 6000 ticks, qui dimensionne les durées de montée
de niveau. Sous 3 rapports, la page sert l'estimation de démarrage à
froid (tables de skills du jeu et pratiques de l'époque, marquée
`prior`) ; à 3 rapports et plus, la mesure remplace l'estimation.

Rentabilité d'une activité et d'une tranche : Σ rendement × prix de
vente estimé (VWAP 24 h, sinon dernier print, sinon meilleur bid au
repos, sinon 1 pièce ; `price_prior` tant qu'aucune transaction ne le
porte) − intrants consommés −
friction des pauses market (pauses mesurées par rapport, valorisées au
taux de l'activité : une pause ne rapporte rien) + bonus de tendance
(score public × chiffre × 0,1). Page `rates` : tableaux méthode × objet
× tranche, rentabilité courante, nombre de bots engagés par script
(information pour l'opérateur humain uniquement, jamais lue par les
bots).

## Tendances et marchands

Signaux publics (carnet, exécutions) : pénurie et momentum, score lissé
borné ; bonus de rentabilité pondéré, anti-oscillation par lissage, durées
aléatoires et profils. Marchands (`market:flip`, ~10 %) : poste fixe place
de Varrock, plafonds de risque, garde-fou 25 % du carnet.

## Front et base

Page `market` (prix au-dessus des bâtons de volume, 6 mois / jour et
24 h / heure, dernier point temps réel par le WS, carnet + dernières
exécutions ; canvas maison, zéro dépendance). La fenêtre demandée est
recalée sur les bornes des bougies (`web::snap_range`) pour que la
bougie en cours reste dedans. Page rendements : tableaux méthode ×
objet × tranche de niveau, rentabilité courante, nombre de bots engagés
par script (information pour l'opérateur humain uniquement, jamais lue
par les bots). Page `life` (`/api/lives`, `/api/life/<compte>`) : but,
profil, engagement en cours, graphe des tâches (statuts colorés,
méthode choisie et alternatives avec coûts estimés, avancement,
ticks dépensés), historique des issues. Page `economy`
(`/api/economy`) : masse monétaire (holdings, hors Iron), deals
(réglés, ratés, taux d'échec), top activités par profit/6000 ticks,
tendances publiques 24 h (pénurie, momentum, score), marchands
(capital engagé en achats ouverts, stock en ventes ouvertes, deals
réglés/ratés, ordres et deals ouverts, part maximale du carnet, profit
de référence `market:flip`). Les bots ne lisent aucune de ces pages.

Base `lifescape-store` (SQLite, daemon seul) : ordres, deals,
exécutions, bougies, rapports, rendements, fiabilité, tendances.

## PRD3 : économie centrale

- Ventes par défaut : le cerveau liste le surplus échangeable des bots
  inactifs (`brain_sales`, toutes les 600 s) à la référence, prix qui
  marche ensuite avec la demande ; protégés : pièces, nourriture ≤ 100,
  keep-list du graphe, intrants du skill entraîné (et un produit qu'une
  autre méthode ouverte consomme), outils, porté, pièces de l'échelle
  d'équipement et palier suivant, envies. Les produits fabriqués
  (barres, forgé, artisanat, cuisine) sont vendus. Les annonces sans
  bornes (posées avant un redémarrage) sont réadoptées à chaque passe
  (hors marchands, qui fixent leurs prix).
- Achats : `market:buy` posé par le cerveau (offre avec plafond) puis
  PARQUÉ (`park_for_fill`, D-SIDE) : l'offre attend au carnet pendant que
  le worker lance le nœud prêt suivant, sinon une fenêtre d'argent de
  1500 ticks (`brain_filler`) ; jamais un worker inactif ; à côté d'une production, offre permanente (D-B)
  annulée quand le nœud aboutit.
- Équipement : échelle par paliers (§ Équipement par paliers) ; les
  envies (`brain_wishlist`) ne portent plus que les outils pour les buts
  mêlée/non combattants (plafond 25 % du portefeuille par outil), et
  l'équipement des buts distance/magie (50 % partagés) ; une offre au
  repos garde sa marche (`market_reprice::keep_bid`), seul son plafond
  suit le portefeuille ; jamais vendues.
- Pièces comme objectif : un achat non couvert crée `have_coins`, résolu
  d'abord par les PNJ qui lâchent des pièces (hommes ~8,2/kill, gobelins
  ~2,2, tables de drop F2P : seule entrée de pièces sans acheteur, les
  magasins n'achetant jamais), puis la méthode d'argent la plus rentable
  (rates par script) ; le choix de site redescend la liste si le niveau
  de combat manque.
- Intrants des skills de transformation : un `blocked{missing}` (cuisinier
  sans poisson cru, allumeur sans bûches, fondeur sans minerai) crée une
  obtention dimensionnée à la charge d'inventaire (`planner::need_count` :
  27 intrants, 13+13 pour le bronze ; un outil = 1), résolue marché
  d'abord ; la progression de niveau remet le compteur de blocages à zéro
  (une charge par blocage est le rythme normal).
- Demande (2026-11-13) : `market_demand::signals()` (offres d'achat au
  repos par objet : nombre, acheteurs, quantité non servie, meilleure
  offre, âge, poids) et `chain_profits()`/`kind_profits()` nourrissent les
  ambitions (voir `docs/planner.md` § Ambitions) ; lecture seule.
- Couverture : pièces et stock en banque comptent (banque connue gardée
  par le SDK après fermeture, voir `docs/protocol.md`) côté cerveau
  (`holdings_count`) et registre (`market_service::ledger_holdings`).
  Côté cerveau, le stock verrouillé par les propres ventes/deals vendeur
  du bot (`locks_for`, les verrous poussés aux workers) n'est PAS détenu :
  `holdings_count`, outils portés (`carried_tools`), base des quotas
  (`brain_locks`, 2026-11-13 : b3gybcev, marteau en vente lu « détenu »,
  forge bannie). Les outils de travail (haches, pioches, filets, cannes,
  appât, tinderbox, marteau, burin, aiguille, cisailles, couteau) ne sont
  jamais mis en vente et leurs annonces existantes sont annulées par la
  passe de ventes (jamais réadoptées).
- Demande vue par un bot (jalons demande, buts d'appoint, tirages) : les
  offres de TOUS les autres comptes, jamais les siennes
  (`market_demand::signals_for`) ; l'affectation de flotte lit la demande
  globale.

## Équipement par paliers (2026-11-13)

Règles pures dans `planner/src/gear_ladder.rs`, câblage `brain_gear.rs`
(une passe par compte toutes les 60 s, bots non Iron, but mêlée ou non
combattant).
- Paliers bronze → fer → acier → mithril → adamant → rune (F2P,
  forgeables) ; le noir compte comme l'acier s'il est possédé mais n'est
  jamais un palier (pas de recette : objet marchand D-A).
- Emplacements dans l'ordre : arme (cimeterre, sinon épée longue), corps
  (plastron, sinon cotte), jambes (jambières, sinon jupe), casque
  (heaume, sinon casque moyen), bouclier (kite, sinon carré).
- Le palier visé est le SUIVANT de ce que le bot possède (sac, banque,
  porté) dans le premier emplacement que ses niveaux ouvrent (attaque
  pour l'arme, défense pour l'armure, verrou de quête → famille
  suivante) ; jamais la meilleure pièce absolue.
- Plafond = `GEAR_SHARE_PCT` = 50 % du portefeuille LIBRE (sac + banque
  moins ce qu'engagent les autres achats au repos du bot, prix × reste,
  ouverts ou partiellement appariés : `brain_gear::free_coins`) ; même
  règle pour les envies (25 % par outil) et les achats du graphe (le
  `have_coins` qu'ils déposent rajoute l'engagé à la cible) : chaque plafond
  se prend sur ce que les autres laissent, plus jamais sur tout le
  portefeuille à chaque fois (revue 2026-11-13) ;
  un seul palier à la fois, il monte avec les pièces ; l'autre moitié
  reste aux intrants, outils et achats du graphe.
- Décision (`gear_move`) : pièce possédée non portée → nœud
  `equip_best` seul (une fois par heure et par pièce) ; vendeur au
  carnet ou portefeuille trop mince pour enchérir → nœuds
  `obtain_items` (marché seul, jamais `shop:buy`) + `equip_best`
  (marqueur `gear:ladder`), lancés avant le nœud suivant du but
  (`next_ready`) : un ask ≤ plafond est pris, un ask plus cher bloque en
  `missing:coins:<2×ask>` → `have_coins` (hommes, gobelins, puis la liste
  de sites) ; aucun vendeur → offre permanente D-B (`keep_bid`) à la
  référence, qui monte jusqu'au plafond pendant que le bot continue son
  but.
- Un palier mort (un nœud de sa chaîne abandonné) est retiré à CHAQUE
  passe de lancement, avant `graph_state`, quel que soit le style du but
  (`gear_ladder::withdraw_dead_steps`) : jamais un but clos `stuck` ni une
  compétence bannie pour une cause d'équipement. Un but qui quitte la
  mêlée annule ses offres d'échelle (ses envies distance/magie restent).
- Un palier dont le vendeur a disparu au moment de résoudre (et
  portefeuille suffisant), ou abandonné, est retiré du graphe
  (`lifecycle::withdraw_gear_step`, historique `gear_withdrawn`, ids
  jamais réutilisés) : un palier ne bloque jamais le but.
- Les offres sur d'autres pièces portables (ancien palier, anciennes
  envies « meilleur équipement ») sont annulées ; les pièces gardées
  (meilleure portable par emplacement et paliers supérieurs, palier
  visé) ne sont jamais vendues, les paliers inférieurs le sont.
- Limites connues : distance/magie gardent les envies ; les Iron n'ont
  pas d'échelle (achat au marché uniquement).

## Prix par l'offre et la demande (2026-11-13)

1 pièce n'est que le prix d'amorçage. Règles (`lifescape_market::zip`,
`market_reprice`) : ouverture à la référence (VWAP 24 h → dernier print
→ milieu → 1), puis un pas de 5 % (≥ 1) par fenêtre de 200 ticks selon
les exécutions de la fenêtre (voir § Market). Aucun plancher lié au coût
des intrants : un vendeur qui ne vend pas descend, un vendeur qui vend
monte ; un acheteur qui n'est pas servi monte jusqu'à sa part de
portefeuille. Le prix suit donc la masse monétaire : plus de pièces →
plafonds plus hauts → prix plus hauts.

Simulations hors ligne (`market/src/sim_demand.rs`,
`orchestrator/src/economy_sim_tests.rs`) : un acheteur (plafond 60)
fait monter son offre depuis 1 ; le producteur (intrants 20) démarre
dès que l'offre couvre ses intrants et vend avec marge ; un ask servi à
chaque fenêtre monte de 1 à ~10 (ce que paient les acheteurs), servi une
fenêtre sur trois il reste plus bas ; une surabondance reste à ~1. Bout
en bout : cimeterre de fer (2 barres à 15), combattant à 200 pièces
(plafond 100) → le forgeron vend à 31, marge positive ; à 40 pièces
(plafond 20 < 30) aucune production tant que `have_coins` ne porte pas
le portefeuille à 62.

Crochet de production (`planner/src/profit.rs`, pour les ambitions et
les producteurs) : `chain_profit(objet, prix, unités_par_6000)` →
`ChainProfit { way, sale_price, inputs, input_cost, margin,
units_per_6000, profit_per_6000 }` ou une erreur typée
(`NotProduced`, `MissingSalePrice`, `MissingInputPrice`, `BadYield`).
`prix` = prix marché par nom debug : celui de l'objet (meilleur bid au
repos si un acheteur attend, sinon la référence) et de chaque intrant
(référence, ou meilleur ask s'il faut l'acheter) ; jamais un prix de
magasin. Rendement = rates mesurés (`task_yields`) ou estimation de
démarrage. La meilleure recette dont tous les intrants ont un prix est
retenue ; les intrants récoltés soi-même sont comptés au prix de marché
(coût d'opportunité). `profit_per_6000 > 0` avec une offre au-dessus de
`input_cost` = signal de produire (entraîner le skill, acheter les
intrants).
