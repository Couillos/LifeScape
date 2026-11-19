# Économie (lot L5 : market + simulateur)

But : prix issus de l'offre et de la demande entre bots, rendements mesurés
en continu, visibles au dashboard et dans une base interrogeable.

## Market

Double enchère continue, un carnet par objet échangeable F2P : priorité prix
puis ancienneté, exécutions partielles au prix de l'ordre au carnet, modifier
= annuler + replacer (perte d'ancienneté). Carnet jamais croisé (sauf
un compte contre lui-même, et la paire d'un deal échoué pendant son
refroidissement de 3000 ticks) : tout ordre qui (ré)entre — placement,
amend, relist après échec, réinsertion au boot — est TAKER et paie le
prix de l'ordre déjà au repos ; `market_uncross` guérit le reste (boot,
puis chaque tour de convocation). Code pur dans
`rs/crates/market/` : `book` (carnet + appariement), `guards` (prix ≥ 1 +
couverture), `deals` (machine à états), `price` (prix courant = médiane des 5
derniers fills bornée par les prix d'impact du carnet, bougies OHLCV), `trends` (signaux publics), `zip` (cotation),
`concession` (courbe vague 2), `maker` (teneurs de marché vague 3 :
tranches, capital cible, cotation selon le stock), `discovery` (ancrages
publics d'un objet jamais échangé, M7), `sim`, `sim_discovery`,
`sim_lifecycle` et `sim_maker` (simulateurs à graines fixes).

Billets = objet (2026-11-19, décision utilisateur : « les items et les
banknote de ces items sont les mêmes items et doivent être mesurés,
évalués, échangés de la même manière, c'est le même objet même si l'id
diffère ») : `lifescape_gamedata::base_obj_id` / `base_obj_name` /
`obj_forms` replient `cert_<nom>` sur `<nom>` partout où l'on compte,
valorise ou échange. Stocks de la flotte (`holdings` : totaux, liste,
variation 24 h, courbes qui additionnent aussi les anciennes lignes
billet), classement (`hiscores::valuation`, prix sous l'id de base),
stocks du cerveau (`brain_plan::stacks_of` : surplus, gardés, verrous,
équipement, outils, nourriture, souhaits, ventes et leur réserve),
registre de couverture du daemon (`market_audit::ledger_holdings`),
opérations `quote`/`book`/`history`/`trades`/`seller_quote`,
`check-method`. Jamais pour UTILISER (même jour : « les banknotes ne
peuvent pas être utilisés pour les craft ») : un billet transporte un
grand nombre d'unités pour échanger ou vendre ; une recette, un sort ou
un repas ne prend que l'objet non noté (les scripts comptent le sac tel
quel ; un dépôt en banque dénote). Restent sous l'id billet : 7
exécutions antérieures au 2026-11-16 (avant `rebase_note_orders`).

Tick réel (plan marché Q0, 2026-11-16) : le serveur tourne à 60 ms
(`../lostcity/engine/data/config/world.json`, `tickRate` 60 ; la variable
`NODE_TICK_RATE=600` du processus ne sert qu'à migrer un ancien `.env`).
Toute durée de marché s'exprime en ticks : 2000 ticks = 120 s, l'attente
de fill de 6000 ticks = 6 min, un horizon de 36 000 ticks = 36 min
(×10 le jour où le serveur passera à 600 ms).

Lignée et motif (plan marché M0-M2, schéma 6, `decisions.md`
2026-11-16) : un amend (annulation + nouvelle ligne : la priorité est
perdue) reste la MÊME intention. La ligne de remplacement porte
`replaces` (la ligne qu'elle remplace), `root_id` et `root_clock` (t0) de
sa lignée, le `motive` de l'ordre ; la ligne remplacée reste `cancelled`
avec `cancel_reason = 'amended'`. Une vraie annulation est une ligne
terminale annulée (jamais remplacée), sa raison dite (`gear_plan`,
`audit`, `need_met`, `need_gone`, `credibility`). L'âge d'une offre (poids
de la demande, `market_demand`), son rang à l'audit de couverture et sa
durée de vie se lisent sur la lignée. Motifs et horizons
(`market_lineage`) : `need` 6 000 ticks (nœud bloquant, offre D-B),
`gear`/`tool`/`surplus`/`market_make` 36 000, `stock`/`loot` 72 000 ; vide =
non dit (ordres de worker). `QuoteBounds` porte lignée, t0, motif,
horizon et ouverture (la courbe de concession de la vague 2 les lira).
Un besoin garde UNE offre par (compte, objet) : `place_obtain_bid` et
les souhaits reprennent l'offre au repos (sa marche continue) au lieu de
l'annuler ; l'échéance de 6000 ticks termine la jambe marché du nœud,
pas l'offre. `orchestrator economy [heures]` imprime les critères du
plan (§ Section marché ci-dessous).

Marche de 10 % (2026-11-19, décision utilisateur : « la règle des 10 %
vaut pour tout le monde en bid et ask, sinon les prix ne sont pas
réels ») : constat live, bûches d'if sans échange pendant 7 h, carl_miller
mis en vente à 68 (2 × son coût de 34), descendu à 34, retiré à
l'horizon, remis à 68, huit fois de suite ; tim_wolfe 5 h à 96, son coût.
Désormais chaque fenêtre (`concession::step`, `WALK_PCT`) : un ask
invendu baisse de 10 % (arrondi au-dessus, au moins une pièce) jusqu'à
1 pièce, SOUS son coût propre (plus de plancher vendeur, plus de retrait
à l'horizon : un ask à 1 reste au carnet) ; un bid non servi monte de
10 % (au moins une pièce) jusqu'à son plafond (ce que l'acheteur peut
payer). Restent : recul ZIP de 5 % après un fill propre, tirage à
mi-chemin vers un prix public, prise du meilleur prix opposé quand
pressé ou quand la marche suivante l'atteint. La courbe ci-dessous ne
fixe plus que le plafond de l'acheteur (sa limite) ; ouverture, β et
horizon ne pilotent plus le prix. Simulateurs (graines fixes) : plus
d'échanges (1,4 à 2,2 × les paires compatibles), acheteurs servis plus
tôt, prix final 17 à 27 % sous l'équilibre des limites tirées (les
vendeurs vendent sous leur coût) ; un excès de butin descend sous toutes
les valeurs de garde.

Pénurie du marché, jamais une interdiction (2026-11-19) : un achat
resté sans vendeur ou refusé par le marché n'interdit aucune compétence
(`planner::market_shortfall`, `brain_succession::close_goal_history` ;
122 interdictions du matin venaient d'achats non servis).

Jamais plus promis que détenu (2026-11-19, décision utilisateur, point
1) : constat hana_91 (teneur), 144 022 pièces et 148 893 engagées dans
29 offres d'achat, la marche de 10 % les montait sans contrôle (`amend`
ne vérifiait pas la couverture) et chaque passe retentait des offres
refusées (129 refus « buyer holds 0 coins »). Désormais une hausse
d'offre d'achat ne dépasse jamais les pièces libres : `op_amend` la
refuse si elle n'est pas couverte, la marche s'arrête au prix que les
pièces libres permettent (`market_audit::affordable_raise`), et un
teneur ne pose ni ne monte une offre non couverte (`market_maker::
move_covered`, sans ligne de refus) ; côté vente, il n'annonce que son
stock libre (`market_maker::sell_leg_within`, lots entiers, rien si
rien n'est libre ; live rae_stone : 370 seaux annoncés pour 131 libres,
refusés à chaque passe). Biens promis (point 2) : les
objets vendus pas encore livrés (deals ouverts côté vendeur,
`brain_locks::promised_names`) entrent dans le `--keep` du concierge de
banque : jamais remis en vente ni jetés (ida_reed, 84 big bones vendus
à thorn_vera remis en vente pendant l'attente de la remise).

Courbe de concession (vague 2, M3/M4, `lifescape_market::concession`,
`market_reprice`, remplace le pas de 5 % par fenêtre) : un ordre posé
avec un `ceiling` (acheteur) ou un `floor` (vendeur) concède dans le temps
vers sa limite PROPRE, `p(t) = ouverture + (limite − ouverture) ×
(t/H)^(1/β)`, t compté depuis le t0 de la lignée (relancé par un fill
propre), H l'horizon du motif, β = `concession_beta(price_patience,
urgence)` = 2^(1 − 2 × patience) × (×2 `need`/banque pleine, ×0,5
`stock`/`loot`, ×1 sinon), borné [0,25 ; 4] ; `price_patience` est lu
dans la vie du bot (`account_patience`, plus le 0,5 du store). Fenêtre
= H/12 bornée [500 ; 2000] ticks, phase propre à chaque ordre (hash
compte/objet : jamais une marche en bloc après un redémarrage).
Chaque fenêtre (`concession::step`) : fill propre → recul ZIP de 5 %
(≥ 1 pièce : le vendeur remonte, l'acheteur redescend) et courbe
relancée de là ; un prix courant échangé (hors fill propre) du côté où
l'ordre concède le tire de moitié vers lui ; sans fill, le prix ne va
que vers la limite (jamais hors [ouverture, limite]) ; amend seulement
au-delà de 1 % (≥ 1 pièce) ou pour poser la limite. M4 : un ordre pressé
(`need`, ou `urgent` : banque pleine) prend le meilleur prix opposé
dans sa limite, tout ordre prend celui que sa courbe atteindrait à la
fenêtre suivante. Q4 (RETIRÉ le 2026-11-19 par la marche de 10 %) : un vendeur à son plancher après H sans fill est
retiré (`cancel_reason = 'horizon'`) et son stock reste en banque un
horizon (`withdrawn_recently`, sauf banque pleine) ; un acheteur reste
à son plafond tant que son besoin existe. Simulateur
`sim_lifecycle` (graines fixes, tableau contre l'ancien pas dans
`decisions.md` 2026-11-16 vague 2).
Acheteur : ouvre à l'ask s'il le couvre, sinon à son propre dernier
prix d'achat de l'objet, sinon au prix courant (`remembered_bid`),
plafond = ce que son portefeuille paie (plus de plafond « juste »).
Vendeur (ouverture ; la marche de 10 % descend ensuite sous le plancher
jusqu'à 1) : plancher = sa réserve, son PROPRE coût (§ Réserve vendeur),
ouverture à la référence échangée (ou au meilleur bid plus riche), à
`max(meilleur bid, 2 × réserve)` pour un objet jamais échangé, jamais
sous la réserve (`seller_open`). Une vente monte sans plafond propre
(seule la demande l'arrête). AUCUN PLANCHER DE PRIX (décision
utilisateur 2026-11-18, « retire les floor de prix ») : le plancher d'un
vendeur est son seul coût propre, jamais ce qu'une boutique paie, le net
d'une alchimie ni un prix de surstock ; un butin (`found_open`)
REJOINT le meilleur ask d'un autre vendeur sur un objet jamais échangé
au lieu de le sous-coter d'une pièce (fin de la cascade à 1) ; sur un
objet échangé il s'ouvre au prix courant, jamais relevé par l'ask plus
cher d'un autre (2026-11-18) ; seul un vendeur pressé (banque
pleine) frappe un bid ≥ son plancher ou sous-cote d'un pas relatif de
1 % (≥ 1 pièce). Surstock (2026-11-18) : un stock libre au-delà des bids
+ 2 jours de volume (au moins un inventaire, `market_fit::overstocked`)
n'est pas détourné vers la boutique (Q3), sans plancher particulier.
Plancher appris (plan prix §1, 2026-11-18 ; lu sous l'objet de base du
carnet pour un lot noté depuis le 2026-11-19 : `cert_yew_logs` ne lisait
jamais ses retraits) : chaque horizon terminé sans
vente (retrait `horizon` au carnet, 24 h glissantes, moins les ventes
réglées) baisse le plancher de 10 %, jusqu'à 1 pièce, sous le coût de
fabrication s'il le faut (`discovery::learned_floor`, `Store::
seller_learning`) ; un lot retiré est remis en vente aussitôt à ce
plancher plus bas (l'ancienne attente Q4 est retirée). Ouverture (§2) :
jamais sur le plancher, au moins plancher × (1 + marge de profil), une
pièce au-dessus au minimum (`discovery::open_over_floor`) ; un bid hors
besoin bloquant s'ouvre sous son plafond (`bid_under_ceiling`). Le
repricing saute un ordre sorti du carnet (deal en livraison). Cotation
par lot (plan prix lot D, 2026-11-18, RETIRÉE le même jour : tout est
coté à l'unité, `LOT_QUOTATION = false`) : un objet qui s'échange en vrac
(empilable, ou ingrédient d'une recette ; jamais un équipement, un outil
transporté ni les pièces) et vaut ≤ 5 pièces se cote pour 100 unités,
≤ 20 pour 10 ; décidé une fois au premier démarrage sur le prix observé
(`items.lot`, `Store::set_item_lot` remet l'historique à l'échelle,
annule les ordres ouverts qui ne sont pas des lots entiers). Carnet,
deals, exécutions et chandelles en pièces par lot ; quantités en lots
entiers ; pièces d'un deal = quantité × prix / lot (`market_units::
deal_coins`, envoyé aux scripts). Le cerveau, les guides et les scripts
gardent des prix par unité : `market_units` convertit à la frontière du
service (`reference` par unité, `reference_lot` interne). Les méthodes
`alch:<objet>` du guide argent remontent d'elles-mêmes. Production
(point 2) : un lot argent sur stock (`--use`) ne fait jamais plus
d'actions que ce que le marché de son produit absorbe
(`market_fit::absorbable_actions` : bids + 2 jours de volume, moins ce
qui est en vente et ce que le bot détient) ; sans place, pas de ligne. Filtre de ramassage (`brain_loot`) : un combattant
laisse au sol les drops de son spot dont la pile vaut moins de 20 ticks
de son propre taux (`--skip-loot`, valeur publique = prix courant
échangé ou prix boutique, le plus riche ; jamais les pièces ni les os).
Voir § Prix par l'offre
et la demande et § Réserve vendeur. Vente aux PNJ (Q3, vague 2,
`decisions.md`) : un surplus qu'une boutique F2P paie PLUS que le marché
(meilleur bid, prix courant échangé ; rien pour un objet jamais échangé
sans bid) n'est pas listé ; quand le gain dépasse 1500 ticks au taux du
bot, le nœud janitor (`SpaceCleanup`, `market cleanup --sale-shop
--sale-site --shopsell ITEM:QTY:MIN`) va à la boutique qui paie le plus
(gardien le plus proche) et vend tant que la valeur serveur minorée
(`shop_sale::unit_value_floor`, delta 30) reste ≥ MIN = ce que paie le
marché + 1. Par paquets depuis le 2026-11-19 (décision utilisateur : « on
doit pas vendre clic par clic, mais en utilisant les options ») : avant
chaque clic, le stock live de la boutique donne le nombre d'unités qui
paient encore MIN (`kit::shop::units_paying`, la valeur de l'unité i se
lit au stock + i comme dans `shop.rs2` `sell_item`) et le bot clique le
plus gros bouton qui tient dedans (`sell_batch_for` : vendre 10, 5 puis
1 ; une boutique n'a pas de « vendre X » en rev 274) ; 28 bûches d'if =
6 clics au lieu de 28. Les pièces créées sont `shop_sold` dans le
registre de pièces ; `orchestrator economy` imprime « npc shop sales ».
Depuis le 2026-11-15 tout
objet F2P qu'un marchand stocke peut lui être acheté (§ Achats aux
marchands de `planner.md`, `decisions.md`). Les marchands tirés (~10 %,
méthode de revente) sont SUPPRIMÉS en vague 3 : des teneurs de marché
dédiés cotent tout l'univers (§ Teneurs de marché ci-dessous).

Couverture vérifiée à l'ordre (objet possédé, pièces disponibles, registre
en mémoire ; les holdings live des connectés se brancheront sur le même
contrôle). Aucune bande de prix (décision utilisateur 2026-11-15 : l'offre et la
demande seules font les prix) : `guards::check_price` refuse seulement un
prix < 1 ; plus de plafond 5× ni de plancher 0,2× autour de la référence,
ni au placement, ni à l'amend, ni dans la marche (`market_reprice`) ou
l'ouverture vendeur. Deals livrés physiquement place
de Varrock (anneau autour de la fontaine, voir § Place ci-dessous) :
`matched → convened(case d'échange, échéance) → both_arrived →
trading → settled | failed{no_show|mismatch|death|timeout|nav_down}`.
Échéance = 2× trajet ETA max + marge (ticks) ; départs échelonnés par
ETA nav (cache par zone de départ, un plan par zone) pour une arrivée
simultanée ; regroupement 300 ticks (une convocation par bot, paires
regroupées en un seul échange) ; une convocation = case + heure de
départ seulement (jamais la position de l'autre). Les ticks sont des
époques par login : la convocation porte des décalages
(`deliver +<départ> +<échéance>`, `market_convene::deliver_args`), que le
worker ancre sur sa propre horloge à la réception
(`market_handover::anchor_deliver_args`) ; un `+N` non ancré est refusé
(`Usage`). Le balayage des expirés suit l'horloge murale du daemon
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

### Stock futur (plan marché M6, Q5, 2026-11-17)

Chaque bot (non Iron, hors teneur qui cote) pose à l'avance des bids
PATIENTS (motif `stock`, horizon 72 000 ticks, β 0,5) pour les intrants
et consommables de sa tranche en cours et de la suivante
(`planner::stock_target`, `orchestrator::brain_stock`, passe de 600 s
après l'alchimie, `decisions.md` 2026-11-17). Cible d'un objet = Σ
consommation /6000 (flux sortants input, ammo, rune, bait, supply, food
du rendement publié de la méthode au niveau et à l'outil du bot ;
nourriture convertie en PV puis en la nourriture qu'il s'offre) × durée
de la tranche (XP de la tranche ÷ XP dérivée /6000), × part d'affinité
`(affinité − 0,25) / 0,75` (sous 0,25 : rien, le bot farme). Bid quand
le détenu libre + en vol tombe sous la moitié de la cible, pour le
reste de la cible ; gardé au repos sinon (marche continue) ; retiré à
la cible (`need_met`) ou quand l'objet quitte le plan (`need_gone`).
Plafond unitaire : prix courant échangé, sinon meilleur ask, jamais
au-dessus du marchand F2P le moins cher (aucun plancher public) ;
budget 50 % du portefeuille libre, la tranche en cours d'abord. Un
objet déjà enchéri par un autre motif reste à ce bid ; les objets du
stock ne sont jamais listés ; les bids `stock` de l'alchimie
(`brain_alch::claimed`) ne sont jamais retirés par cette passe.
Exemples : pêcheur mouche 20-25 → plumes (≈ une par prise de la
tranche) ; firemaker → bûches ; mage → runes du sort ; combattant →
nourriture adaptée à ses PV, s'il en mange.

### Montées en mode marché : bids `need` au volume de la tranche (2026-11-17)

Levier utilisateur « entraînement marché plus fréquent » (`decisions.md`
2026-11-17) : une montée dont le tirage R7 prend une ligne MARCHÉ du
guide d'entraînement (`brain_train`, `TrainPick::buys_inputs`) marque son
nœud (`Node::buys_inputs`, écrit dans la vie, remis à zéro à chaque
résolution). Quand elle bloque sur ses intrants (barres, bûches, poisson
cru, runes, os, peaux, essence…), l'obtention déposée (`replan`,
`replan::buys_by_market`) n'a que la jambe `market:buy`, dimensionnée sur
la tranche (`need_count_with` : intrants de son niveau live à la fin de
la tranche) : un ask couvert est pris, sinon un bid `need` au repos au
volume de la tranche, borné par ce que le portefeuille paie
(`resolve_obtain::buy_quantity`), le reste au blocage suivant ; sans
pièces, `have_coins` d'abord. Jamais de production en parallèle ni de
fonte/coupe de l'intrant. Les outils (marteau, tinderbox) gardent leurs
jambes. Une ligne autonome garde toutes ses jambes (production d'abord).
Ironman : jamais (aucune ligne marché).

### Découverte du prix d'un objet jamais échangé (plan marché M7, 2026-11-17)

Un objet sans aucun fill s'ouvre sur des ancrages PUBLICS seulement
(`lifescape_market::discovery`, lus par `orchestrator::market_discovery`,
`decisions.md` 2026-11-17) : offre d'une boutique F2P ouverte à tous
(Q3, `calc_shop_value`) ; net d'une alchimie (le vendeur : son propre
sort, haute ou basse, runes au prix courant ; acheteur et teneur : la
basse alchimie) ; coût de production publié par les guides
(`planner::guide_cost` : ligne cotée recette, chaîne ou 100 % market la
moins chère, coût/6000 ÷ produit/6000, mis en cache par jeu publié) ;
prix de vente d'une boutique F2P ouverte à tous. Ce sont des repères
d'OUVERTURE, jamais des planchers (2026-11-18). Règles : plancher
vendeur = son coût propre ; ouverture vendeur = coût publié × (1 + marge
du profil), à défaut 2 × plancher (butin : sa valeur de haute alchimie),
jamais au-dessus du prix boutique ni sous le plancher, un bid plus riche
est pris ; ouverture acheteur = valeur d'usage × (1 − marge) (prix
attendu du gear, réserve dérivée, sinon coût publié, sinon prix
boutique), jamais au-dessus du coût publié ni du prix boutique ni du
plafond, 1 pièce sans valeur connue ; aucune offre d'achat n'est
refusée pour « manque de crédibilité » ; un ask couvert reste pris. Marge =
`0,05 + 0,15 × price_patience` (`Profile::margin`). Sans ancrage :
règles existantes, aucun prix inventé, ligne de guide « non cotée »
inchangée. Premier fill fait : prix courant, concession et ZIP
reprennent la main. Simulateur `sim_discovery` (chiffres dans
`decisions.md`).

### Section marché d'`orchestrator economy` (critères du plan marché §6)

Sur la fenêtre `[maintenant − heures, maintenant)`, staff exclu
(`Store::market_criteria`) : part des objets actifs cotés des deux côtés
(base 12,7 % → ≥ 35 %) ; écart médian meilleur ask / meilleur bid sur
ceux-là (×1,72 → ≤ ×1,25) ; lignées d'achat terminées : vraies
annulations / réglées (6 217 / 6 929 → ≤ 0,2) ; annulations du plan
d'équipement (`gear_plan`, ~5 300/24 h → ≤ 200) ; lignées d'achat
annulées puis rouvertes plus bas par le même compte (46 % → ≤ 5 %) ; vie
médiane d'une lignée d'achat non servie, de t0 à son annulation (~1 080 s
→ ≥ son horizon) ; volume réglé (unités, pièces) ; objets échangés à 1
pièce dont la consommation mesurée (`task_flows` input/ammo/rune/bait/
supply/food, lignes v1 `consumed`) est > 0 (≥ 6 → 0) ; asks à 1 pièce
(objets, ordres, unités) ; objets en asks seuls jamais échangés (65 →
≤ 20) ; amends de la fenêtre (par minute : CPU de la marche).

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

Référence (PRD3, remplacée le 2026-11-14 par le prix courant, § Prix
courant) = VWAP 24 h ; sinon dernier print ; sinon milieu du
carnet ; sinon **1 pièce** (jamais le prix magasin). Ce repli public
reste honnête (« pas de donnée ») mais aucun vendeur n'en part : un
objet non échangé (référence non `traded`) ouvre à `2 × réserve`. Bougies OHLCV en horloge (1 h / 7 jours,
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
seul). Page `/market` (2026-11-13) : liste `list?active=1` = biens avec
au moins un ordre ouvert OU au moins une exécution passée (pas de
plafond ; carnet vide jamais échangé caché) ; `recent` porte le `name`
de chaque deal, le flux et le titre ne dépendent plus de la liste. Les
ETA de convocation passent par le service de navigation (un plan par
zone de départ, `market_eta`).

Fin de remise (2026-11-19, décisions utilisateur : « relancer son
activité depuis la place Varrock », puis « une livraison doit mettre en
pause la tâche en cours, pas l'arrêter ») : après les échanges, la
livraison dépose TOUT à la banque la plus proche de la place, sauf les
outils (tout objet qu'une voie du jeu porte comme outil,
`gamedata::obtain::is_carried_tool` : hache, pioche, filet, ciseau,
marteau, briquet, moules, aiguille…) et les objets que le script en
pause tient (`market_lots::pack_kept_after_trades`, `pack_stays`) ; plus
de retrait de l'ancien sac ni de marche retour. La tâche en pause
REPREND depuis la place (`session_rescue::gate_market`) : le trajet en
cours se replanifie depuis là (`travel_pause`), les scripts retournent à
leur site et à leur banque d'eux-mêmes. La banque de la livraison est
atteinte par un vrai trajet puis ouverte, un second essai
(`market_lots::open_bank_here`, live : 39 « lots not secured » quand le
bot repartait vers son site).

Échecs de remise (revue 2026-11-13, deals 1100–1285) :
- `timeout` dominant : la pause market tourne DANS la tâche en cours
  (souvent une fenêtre d'argent de 1500 ticks) et le crochet de budget
  comptait la pause en vol comme du travail → `interrupted: run stopped`
  au milieu de la remise, souvent sur la case. La pause est facturée dès
  son premier tick (`Bot::begin_pause` / `pause_ticks_now`, `budget_due`).
- Convocation calée sur l'instantané d'un login précédent (worker rouvert
  à t=1, départ 61826) : corrigé par les décalages ci-dessus.
- `mismatch` à tort (1116, 1124) : une relance (marche d'approche ou
  `op_player`) croise la fenêtre que le partenaire vient d'ouvrir et la
  ferme (« Other player declined trade. »), les deux côtés lisaient un
  désaccord. `trade_request` relit le flux avant tout clic ; une fenêtre
  fermée avant les lots est `ExchangeOutcome::WindowClosed` /
  `SellerOffer::Closed` : la paire se retrouve (`MEET_RETRIES` = 3) tant
  que le daemon garde le deal.
- Restent hors marché : « nearest bank did not open » (marche vers la
  banque refusée par une porte/un loc, nav) et paires adjacentes qui ne
  se voient jamais (`cannot see`, liste joueurs SDK ; la ligne journalise
  désormais `debug_players`).
Échecs de remise (revue 2026-11-13 soir, deals 1643–1799, après le
déploiement de 21:19 CET) :
- `short` = lot promis deux fois. Un ordre entièrement apparié ne
  laisse aucune ligne au repos, mais ses biens (ou ses pièces) restent en
  banque jusqu'à la remise ; le registre les ré-miroitait depuis le
  snapshot à chaque tour, le cerveau les relistait et la relance
  s'appariait aussitôt (bct5rxpg 1716 + 1722 pour 12 flèches,
  fiona_park 1698 + 1721, 1692 + 1720) ; même chose côté pièces avec les
  offres permanentes reposées après un balayage complet (bd23dqpq « 98/98 »
  puis « 97/98 », 1643–1652 ; bct5rxpg 1772, 1789). Corrigé : le miroir
  du registre est net des deals en vol (`market_audit::net_of_deals`) ;
  depuis le 2026-11-19 il ne recopie qu'un instantané qui voit le sac ET
  la banque depuis l'ouverture de la session (`witnesses_holdings` :
  `inv_streamed`, `bank_streamed`), sinon le registre connu reste
  (thorn_vera après un redéploiement : banque pas encore vue, registre
  mis à 0 pièce 0 objet, bids « buyer holds 0 coins » et six ventes
  « seller holds 0 » refusés pendant ~20 s),
  `op_place` ne couvre que le non-engagé (registre moins les ordres au
  repos du même côté, `market_audit::uncommitted`) ; côté cerveau, le
  surplus est net des verrous (`brain_sales::free_stacks`), les pièces
  engagées comptent les deals acheteurs en vol (`brain_gear::deal_coins_from`)
  et l'offre permanente attend la remise d'un achat en vol
  (`brain_market::buying_in_flight`). Ni piles ni billets en cause : les
  quantités retirées en banque étaient exactes, la banque avait moins.
- `no_show` will_field (×7) : `spin` bloqué depuis 20:19 UTC. Le
  level-up a cassé la chaîne make-X avec de la laine restante ;
  `produce::drain_chained` (cible `u32::MAX` pour `want` 0) attendait
  sans fin, modale de chat ouverte, donc jamais de point sûr pour la
  pause market. Une chaîne qui ne consomme rien pendant `ITEM_TICKS`
  s'arrête (`produce::chain_stalled`).
- `no_show` t01 (1671/1672/1684) et cascade (1687 `timeout`, 1689
  `no_show` contre bd23dqpq pourtant présent) : un worker inactif pompe
  sur le thread de service (`idle_pump`) et la porte de pause y lançait
  la livraison en ligne, bloquant l'accept du socket pendant tout le
  trajet ; chaque pose du daemon expirait, la liste `unposed` renvoyait
  la convocation périmée (1664, déjà close) à chaque tour, qui écrasait
  la nouvelle et relançait 500 ticks de banque Falador ↔ Varrock.
  Corrigé : la porte ne s'ouvre qu'en présence d'un thread de script
  (`gate_tick`, sinon `drain_idle_market`) ; le daemon ne renvoie que la
  pose la plus récente par compte et seulement si un de ses deals est
  ouvert (`market_daemon::prune_unposed`) ; une livraison sans deal
  ouvert rend la main sans aller en banque.
- Cascade de groupes : un bot à plusieurs partenaires attendait un absent
  sur sa case pendant qu'un autre partenaire l'attendait, immobile, à
  quelques cases (bd23dqpq/nina_rhodes, 1689). Un partenaire hors de vue
  depuis `SERVE_FIRST_AFTER_TICKS` (100) cède le tour au premier groupe
  suivant dont le partenaire attend sur son point (`market_handover::
  serve_first`, au plus 2 détours par groupe, jamais une fois la fenêtre
  ouverte).
- `trade gave no harpoon` (bdr53jee) : la pause market dépose tout sauf
  les outils ; l'achat en magasin interrompu reprenait sans ses pièces
  (« You don't have enough coins »). La livraison rend désormais le sac
  d'avant la pause après les échanges (`market_lots::carried_before`,
  `redeposit`), formes billets comprises, sans jamais retirer deux fois
  ni sortir le verrouillé.
- Trajet coupé par une pause (bct5rxpg, bdr53jee, b5xntyz6 : « step
  refused »/porte jamais ouverte avec « last game: Accepted trade. ») :
  la livraison tourne dans le pump d'un segment en cours ; au retour la
  jambe périmée échouait, avec dossier et arête bannie. Une pause finie
  pendant le plan (`Bot::pauses_done`) explique l'échec : replan depuis
  la position, sans ban ni dossier, échéance recalculée
  (`travel::paused_mid_trip`).
Les holdings live ne sont pas encore branchées au registre.

## Simulateur

Graines fixes, agents ZIP synthétiques (offres/demandes tirées des
catalogues). Mesuré : convergence des prix à ±10 % de l'équilibre en 2
appariements sur le scénario symétrique (12 acheteurs / 12 vendeurs) ;
choc d'offre (+6 vendeurs au rabais) sans oscillation entretenue (16
inversions sur 120 appariements, prix final dans la bande) ; deux
marchands resserrés sur carnet clairsemé : 124 appariements contre 93
sans eux (+33 %).

## Rendements

Une seule mesure (décision 2026-11-16, `docs/decisions.md`, plan
`docs/plan-rendements.txt`) : le RENDEMENT d'une activité, bilan de flux
SIGNÉ par 6000 ticks de travail — tout ce qui entre (+) et tout ce qui
sort (−) d'un run, chaque flux avec sa cause. L'XP et le profit en sont
dérivés (XP = Σ unités porteuses × XP de l'unité du dump ; profit = Σ flux
× prix courant), jamais mesurés à part ; les guides money making et
entraînement en sont générés (lots R2-R8, à venir).

Contrat de rapport 2 (lot R1, `session_report.rs`, `report_contract.rs`),
poussé par la session à chaque fin de tâche :
- contexte : `planner_method` (méthode fine du nœud lancé, transmise par
  `--method` que la session retire avant le script ; `method_source` =
  `node`, ou `none` quand la jambe ne l'a pas portée : rien n'est deviné),
  `site` [x, z, niveau], `target` (ressource, spot ou recette donnée au
  script), `tool` (meilleur palier de la famille d'outil détenu au départ,
  ou arme + munitions portées d'un combat), `combat_style`
  (`melee:<skill entraîné>`, `ranged`, `magic`), `levels_start` (tous les
  skills), `work_ticks` et `pause_ticks` séparés ;
- `flows` : bilan signé [{name, cause, qty}] tiré du registre par action
  du SDK (`item_ledger`, `coin_ledger`), jamais du diff net. Causes
  (`flow_causes.rs`, données du jeu, jamais une liste) : `product` (P),
  `byproduct` (brûlé, gemme), `input` (C : cru cuit, minerai, barre,
  argile, bûche brûlée, os enterré, essence, objet alché/enchanté),
  consommables de méthode `ammo` (flèches tirées −, ramassées +), `rune`,
  `bait` (appât, plumes), `supply` (fil), `food`, `discarded` (jeté),
  `loot` (ramassé en combat), `ground_spawn` (ramassé ailleurs), `fee`,
  `death`, `shop_bought`, `shop_sold`, `traded`, et `unattributed` ;
- `units` : unités porteuses d'XP comptées brutes (fabriquées puis mangées
  dans le même run comprises) — `product`, `input`, `cast` (sorts payés
  par leur XP de magie), `kill` et `damage` par type de PNJ (masques de
  dégâts NPC_INFO sur la cible du bot ; le serveur n'envoie pas
  l'attaquant) ;
- `alerts` : `unattributed` (flux que rien n'explique) et `residual`
  (écart entre le diff net sac + porté + banque et la somme des flux d'un
  objet : contrôle de cohérence D4/D19), loggées par la session
  (`task_report alert …`), objectif zéro, jamais un rendement.

Les champs v1 (`activity`, `items` aux provenances D45, `ticks`,
`market_pause_ticks`) restent, dérivés des causes (`legacy_provenance` :
product → produced, consommables → consumed, discarded → lost…) : ils
servent le store v1 et le carnet marchand, plus aucun rendement (R2).

Registre SDK : sac et équipement porté observés ensemble (porter ne sort
rien, une flèche tirée sort du carquois), à chaque frontière de tick
serveur (avant chaque `PLAYER_INFO`, plus une fois au début et à la fin
d'un rapport) : le serveur écrit sac et porté en deux trames, une
observation en milieu de tick lisait un échange d'équipement comme quatre
flux `unattributed` ; actions ciblées par objet et par signe (manger,
enterrer, jeter, ramasser, lancer) ; contexte boutique, échange,
dialogue ; XP par stat dans les 8 ticks (`skill`) ; banque ouverte =
transfert ; fermée depuis moins de 2 ticks = transfert, sauf action
ciblée envoyée depuis (un jet juste après la fermeture reste `discarded`) ;
pertes dans les 30 ticks d'une mort = `death`. Contrôle de cohérence :
billets repliés sur l'objet de base (la banque ne tient que l'objet de
base, un lot du marché arrive noté) ; banque de fin = banque vivante, ou
telle qu'à son dernier `UPDATE_INV_STOP_TRANSMIT` (`World::inv_closed`).

Store (schéma 5) : `task_reports` + `measure_version` (1 = rapport par
diff net, 2 = contrat 2), méthode fine, source, site, cible, outil, style,
ticks de travail et de pause ; `task_levels`, `task_flows` (cause),
`task_units`, `task_alerts`. Purge des mesures 1 (D8) préparée,
exécutée seulement sur confirmation utilisateur, daemon arrêté :
`orchestrator purge-measures-v1` (simulation) puis `--confirm` (lignes
v1 de `task_reports`/`task_yields` et `run/rates-snapshot.json`).

Ingestion : la session pousse le rapport, le superviseur du daemon le
verse dans le store (`jobs::ingest_report_into` : lignes v1 + mesure du
contrat 2) puis dans l'agrégat persisté (`rates_service`,
`run/rates-snapshot.json` schéma 4, rechargé au redémarrage ; fichier
absent, corrompu ou d'un schéma plus ancien : reconstruit depuis les
rapports contrat 2 du store, rejoués par la même admission). Écriture
différée (`rates_flush.rs`) : au plus une réécriture par minute, plus
l'arrêt propre. Un lecteur prend un `Arc` du snapshot, jamais une copie.

Estimateur (R2, `rates_estimator`, `rates_measure`, `rates_read`) :
- clé d'ACTIVITÉ (R2.1, `planner::activity`) : ce que le run a FAIT,
  jamais l'objet voulu. Un drop farm est le combat qu'il lance
  (`farm:feather` en `melee:attack` sur les poulets → `fight:melee/
  chickens`, style dans la clé ; ranged `fight:ranged/<spot>`, magie
  `fight:magic/<sort lancé>/<spot>`), une pêche son option de spot
  (`farm:raw_salmon`, `gather:fishing/raw_salmon` → `gather:fishing/
  raw_trout`), `farm:<minerai|bûche>` le récolteur, `farm:blankrune`
  `essence:blankrune`, `cook/<cru>` `cook:<plat>` ; le reste nomme déjà
  son activité. Le store garde méthode, style, cible et site : la clé
  se dérive au rejeu (schéma 4 → reconstruction au démarrage). Le
  cerveau lit par la même dérivation (`brain_plan::rate_key`), prior de
  SA méthode (un drop farm garde le prior de son objet) ;
- seules les causes de rendement entrent dans un taux
  (`rates_measure::RATED_CAUSES` : product, byproduct, input, ammo,
  rune, bait, supply, food, discarded, loot, ground_spawn, fee, death) ;
  `traded`, `shop_bought`, `shop_sold`, `unattributed` restent au store ;
- seul le contrat 2 l'alimente ; un rapport sans méthode du
  planificateur (`method_source = none`), un marchand, le tutoriel ou une
  méthode sans voie du jeu (equip, quest, space…) n'est jamais agrégé
  (loggé, rien de deviné) ;
- clé D2 = (méthode fine, style de combat, site, palier d'outil, tranche
  de 5 niveaux du skill de la méthode ; 0 sans skill) ; chaque run
  alimente aussi deux cumuls « tous sites » et « tous sites et outils » ;
- seau = sommes déclinées, jamais d'échantillons (mémoire bornée,
  `MAX_BUCKETS` seaux au plus) : flux signés par (objet, cause), unités
  porteuses, ticks de travail et de pause ; taux = Σ quantités /
  Σ work_ticks × 6000 ; avant chaque run de t ticks, tout est multiplié
  par 0,5^(t / 60 000) (D7) ; run < 300 ticks pondéré t / 300 ;
- `failed` / `blocked` : hors taux, taux d'échec du seau ; échantillon
  au-dessus du plafond physique (ticks d'action du serveur, succès
  certain, une action de marge) : rejeté et loggé ;
- lecture : clé exacte, sinon tous sites, sinon tous sites et outils
  (repli loggé ; les cumuls restent INTERNES, jamais publiés comme
  lignes) ; seuil unique n ≥ 3 (page ET cerveau), puis mélange
  avec le prior (le prior pèse 6000 ticks de travail) ; sous le seuil, le
  prior seul (marqué) ; ni mesure ni prior calculable = `RateError`
  typée, jamais un chiffre par défaut ; le cerveau lit à sa tranche
  réelle (niveau live, meilleur outil possédé) ;
- confiance : n, âge, dispersion du taux d'unités par run, taux
  d'échec, rejets.

Priors (R2, `planner::method_rate`) calculés du jeu pour chaque famille :
6000 / (ticks d'une tentative + (aller-retour banque + ops de banque) /
tentatives par sac) × flux par tentative. Ticks d'action relus dans les
scripts serveur (boucles p_op : roll toutes les N + 1 ticks, `p_delay(n)`
répété toutes les n + 2) ; succès `stat_random` au niveau
(`gamedata::action_odds`, dump `action_odds.json` : roches, pioches,
arbres par hache, poissons, cuisson feu/fourneau, runes par essence,
repousse des roches et abattage des arbres, jet de gemme d'une roche et
`mining_gem_table` relus de `mining.rs2`, `mining_gems`) ;
combat : `melee_math` (mêlée), modèle `sizing` (distance, magie). Familles
sans timing (moulin, combinaisons de préparation) : `PriorError`.
Ressources épuisables (`method_rate_trip`, audit 2026-11-16) : une roche
minée se vide (`rock_respawnrate`, `p_delay(0)` d'une roche normale), un
arbre normal tombe à chaque bûche (120 + random(80) ticks), les autres une
fois sur 8 (`respawnrate`), un objet au sol ou un mouton tondu (100 ticks)
repousse : chaque succès paie la marche vers la ressource voisine du
groupe (même voie à ≤ 10 cases) et un petit groupe attend ses repousses
(au plus `nombre / repousse` par tick) ; sans site demandé, le site de
travail est celui qui minimise marche banque + coût par succès, jamais le
monde entier. Trajets : un site souterrain (z ≥ 6400) remonte par son
échelle (`gamedata::underground`, `transitions.json`) ; la mine
d'essence passe par la téléportation d'Aubury (6 ticks) et le portail
(3) ; une visite de banque = ouvrir, déposer, fermer + 2 ops par retrait
(bouton, quantité) ; runecraft : entrée des ruines (`p_delay(1)`) et
portail par charge. Tailler une gemme reste 1 tick (`[opheldu,chisel]`
sans délai ni make-X). Gardes : `method_rate_server_tests` (un test par
famille épinglé au script serveur, plafond physique sur tout le
catalogue, ×2 d'une mesure de référence).

XP dérivée (R3.1/R3.2, `planner::unit_xp`) : XP/6000 par skill = Σ unités
porteuses × XP de l'unité (dump / 10) : produit (récolte, cuisson,
forge, artisanat, enchantement, superheat Magic + Smithing, telegrab),
intrant (bûche brûlée, os enterré — dans tout run, combat compris —,
essence, objet alché), lancer (téléport, malédiction, bananes, base d'un
sort de combat), dégâts (×4 au skill du style, contrôlé ×1,33 à chacun
des trois, distance ×4, magie ×2, PV ×1,33). Les seaux d'XP mesurée sont
supprimés ; l'XP observée sert au contrôle D4 à l'ingestion (écart > 5 %
loggé `rates: xp coherence`).

XP et temps dérivés PARTOUT (R3.3, 2026-11-17, `planner::work_rate`) :
le planificateur ne cote plus une montée avec ses propres constantes
(presse 5 ticks, `BANK_TRIP_TICKS` 60, `HIT_ODDS` 0,6, ticks et XP écrits
dans les étapes de chaîne) ; il lit un `WorkRate` (actions, XP dérivée
des unités, flux signés par 6000 ticks) : ticks par XP = 6000 / XP,
consommables par XP = |flux| / XP. Lecteurs : `training` (pricer,
`TrainPrices::rate`), `sizing` (XP par action, durée et consommation
d'une tranche, niveau par niveau ; prior du jeu, `*_with` pour une
mesure), durée des tranches `ReachLevel` (`est_ticks`), chaînes D58
(`eval_chain_with`, une étape sans rendement laisse la chaîne non
cotée). Le prior des combats à distance et de magie suit les jets du
serveur (attaque contre `rangedefence` / magie + `magicdefence` du PNJ,
dump `npc_combat.json`) au lieu de `HIT_ODDS`. Détail :
`docs/planner.md` § « XP et temps dérivés des rendements ».

Page `rates` = vue MESURES (R2.1, `web_rates::measure_rows`) : une
ligne par clé exacte mesurée, avec SES sommes (Σ / Σ ticks de travail),
jamais la valeur d'un cumul ni le prior ; `thin` sous n = 3 ; les
ramassages sous `guide_flows::NEGLIGIBLE_DROP_PER_6000` (1 par 6000
ticks : moins d'une fois par fenêtre, une ligne qui ne tient qu'à un ou
deux ramassages) se replient en une ligne « other drops » par clé,
valorisée au prix courant (le profit garde chaque pièce). Copie de la
base live du 2026-11-16 rejouée (`orchestrator rates-rebuild <copie>`) :
735 clés mesurées (637 sous n = 3), 107 activités × style, 2 730 lignes
de flux (contre 9 583 publiées avant, copies et échanges compris).
Page agrégée (2026-11-17, `rates_view`, `docs/orchestrator.md`) : la
liste plate atteignait 18 388 lignes de flux (+ 10 948 unités, 6 904 XP)
sur la copie du 2026-11-17 ; la page sert une ligne par activité mesurée
(188 ; 88 « mesurées seulement », vue par défaut) ou par clé exacte
(3 433), produits et consommations principaux, XP dérivée, confiance
(mesurée n ≥ 3 / estimée thin), filtres et pagination serveur, détail
de chaque activité (tous les flux, other drops, unités, clés en
sous-lignes). Une ligne d'activité plie ses clés en Σ / Σ ticks de
travail : chiffre de vue seulement (les cumuls de l'estimateur restent
internes, le cerveau lit la clé exacte).

Guides (R4, `planner::guide_*`, moteur pur ; `web_guides`,
`/api/guides/money|training?skill&level&family&quests`, pages
`/guides-money`, `/guides-training`) : une ligne pour chaque activité F2P
du planificateur (`guide_methods::catalogue` : jambes de production de
tout objet F2P par activité, combats en 4 styles, sorts D18, feu, os,
essence, tranches d'entraînement, alchimie des objets fabriqués) × site
(le meilleur du prior, puis chaque clé mesurée de la tranche dont la clé
EXACTE porte la mesure, n ≥ 3 : un site mesuré que l'estimateur lit par
un cumul ou le prior ne fait aucune ligne, jamais une copie ; une clé
listée deux fois, une ligne — décision 2026-11-16 §4) × outil (le palier
du niveau) × tranche : une ligne par (méthode, style, site, outil,
tranche, famille ou mode). Lecture : estimation `rates_read` avec
le prior des guides (`method_rate::guide_prior` : un combat y ajoute sa
table de drops ; le cerveau garde `method_prior`). Prix (2026-11-16,
`guide_engine`), toujours pour le VOLUME d'une ligne (unités / 6000
ticks), jamais le prix d'une unité appliqué à une fenêtre :
- vente d'un objet échangé : prix courant (D12), ou le PNJ s'il paie
  plus (Q3) ; un bid qui lève le prix au-dessus de la médiane ne paie que
  ses propres unités (un bid d'1 unité paie 1 unité, le reste à la
  médiane) ;
- PRIX BORNÉ AU VOLUME RÉCENT (décision 2026-11-16 sous 5 fills, étendue
  2026-11-17 à tout nombre de fills) : le prix courant ne vaut que pour
  le volume réellement échangé RÉCEMMENT, c.-à-d. Σ des quantités des
  fills qui forment la médiane (les 5 derniers fills de l'objet,
  `PRICE_FILLS`, quel que soit leur âge : la même fenêtre que D12,
  `Quote::fill_units`, `web_guides::quote_of`) ; au-delà, vente aux bids
  restants pour leur profondeur, puis au PNJ (Q3), sinon 0, comme un
  objet jamais échangé (1 fill d'1 unité à 15 411 : 1 unité à 15 411,
  pas 300 ; Leather cowl, 5 fills d'1 unité vers 462 : 5 unités à 462
  sur 988 par fenêtre, pas 988). Moins de 5 fills marquent le prix
  `thin`, de même qu'un volume qui dépasse le volume échangé quand le
  prix courant en porte encore la majorité ;
- vente d'un objet jamais échangé : pas de prix courant ; bids pour
  leur profondeur, le reste au PNJ (Q3, `web_guides::NPC_SALE_VOLUME`)
  ou 0 sans débouché ;
- ACHAT (décision 2026-11-16) : au moins le meilleur ask disponible :
  asks au repos (chaque unité à max(prix courant, ask) pour un objet
  échangé, à l'ask seul sinon) et fenêtre des boutiques F2P parcourus
  ENSEMBLE du moins cher pour le volume (boutique d'abord à prix égal) ;
  au-delà (D22, 2026-11-17 : plus aucune ligne bloquée) le prix courant
  d'un objet échangé, jamais moins que la dernière unité parcourue (un
  objet jamais échangé : la dernière unité parcourue ; rien parcouru :
  NON COTÉ, `priced` false, aucun profit inventé) ; ce que carnet et
  boutiques tiennent est une CONDITION de la ligne (`MarketNeed`), jugée
  par chaque bot (`guide_feasible::feasible_for`) ; médiane 7 sous un
  ask à 35 : achat à 35. Même
  logique de volume que la vente : le prix courant n'est qu'un plancher,
  jamais une profondeur (5 fills de 5 000 unités ne vendent pas une
  unité que personne n'offre ; test
  `guide_volume_tests::current_price_gives_no_purchase_depth`) ;
- STOCK BOUTIQUE FINI (décision 2026-11-16 §2, `gamedata::shop_buy`) :
  une boutique vend par fenêtre de 6000 ticks son stock de base, unité
  par unité au prix serveur du stock restant (`shop.rs2`
  `adjusted_item_cost_buying` = `calc_shop_value(coût, shop_delta,
  multiplicateur de vente, stock − base)`, au moins 1 pièce : le prix
  monte à chaque unité achetée), puis son réassort de la fenêtre (une
  unité toutes les `rate` ticks sous la base, `World.ts`, achetée au
  stock 1) ; chacune à son ouvreur F2P le moins cher, `shop_delta` relu des configs
  (`gamedata/data/shop_haggle.json` : param des `.npc`, défaut de
  `shopkeeper.param`, argument des `~openshop`) ; au-delà, comme au-delà
  des asks : prix courant, condition « buy X N/lot: asks A, recent R,
  shops S » (D22) ;
- BOUTIQUES À CONDITION (Q7, décision 2026-11-16, achat ET vente
  alignés) : toute boutique F2P qu'un non-membre atteint
  (`gamedata::obtain::shop_reach`) est dans la fenêtre d'achat
  (`shop_buy::purchase_steps`) et parmi les débouchés PNJ
  (`shop_sale::npc_sale_shops_reachable`), sa condition avec elle :
  quête F2P de l'ouvreur (Oziach, `runeplateshop` : Dragon Slayer) ou
  points de quête de la porte (Champions' Guild, `championshop` et
  `runiteshop` : 32). Une ligne qui achète ou vend à une telle boutique
  en porte la condition en prérequis (`Prereqs::quests`,
  `Prereqs::quest_points` ; `guide_prereq::require_shop`, points tenus =
  `quest_points_of(quêtes faites)`) ; une recette valorisant son stock
  au prix de vente (D5) ne commerce avec personne : aucune condition.
  Exclues, inaccessibles en F2P : Magic Guild (membres), Lundail (Mage
  Arena, hors carte F2P). Le cerveau (voyage de vente Q3,
  `npc_sale_shops`/`npc_sale_value`) ne garde que les boutiques ouvertes
  à tous (ni quête ni points) ;
- OBJET ≠ NOM AFFICHÉ (correctif 2026-11-16) : un prix du guide se lit
  sur l'OBJET (`web_guides::quote_of`, id de l'objet dans le store),
  jamais sur le premier objet d'un nom affiché ; les flux d'une activité
  (rapports et prior en noms affichés) se relisent dans les objets de SA
  voie (`rates_names::debug_in` : sortant = intrants et outils de la voie,
  entrant = son produit). Cas réel : « Sapphire amulet » = amulette
  enfilée (que `enchant:strung_sapphire_amulet` prend) ET non enfilée
  (que `string:` prend) ; la ligne market de l'enchantement achetait
  les 150 asks à 540 de l'amulette non enfilée et affichait 1,24 M/6000
  (fixant D17 au niveau 30) quand l'amulette enfilée n'a qu'un ask ;
- confiance prix par flux (`basis` : traded, thin, shop, book,
  book+shop, book+none, none, own ; prix courant si la majorité du
  volume y est, sinon ce que carnet et PNJ en portent) et par ligne
  (`price_confidence` : traded | thin | book/npc).
Recette sur stock (D5) : condition `stock` = un lot de chaque intrant
(paquet de 28 cases) ; guide général (rien détenu) : économie d'une
fenêtre pleine, infaisable pour le bot sans stock (jamais lue par D17) ;
pour un bot (`held=objet:qté,...`, R7) : évaluée sur SON stock (part de
la fenêtre que le stock couvre) et jugée sur lui. Un nœud argent lance
TOUTE recette sur son stock (suite R7, 2026-11-17 : cuire sa pêche,
fondre ses minerais, forger ses barres, filer sa laine, tanner et coudre
ses peaux, tailler ses gemmes, cuire ses poteries, ses runes…, plus
seulement l'alchimie) : lot = actions que son stock libre couvre
(`planner::stock_lot`, l'intrant le plus rare décide), lancé avec
`--use <intrant> N` (fin `goal_reached` à N consommés ou au stock épuisé,
`scripts::kit::stock_use`) ; produits vendus par la vente par défaut.
CONDITIONS ET FAISABILITÉ (D22, 2026-11-17, `planner::guide_feasible`,
décision datée dans `docs/decisions.md`) : aucune ligne n'est bloquée.
Chaque ligne porte sa liste structurée d'exigences (`conditions`) —
niveaux, combat, site (`site` : aucun site à portée de banque,
`danger::NEVER`, IMPOSSIBLE aujourd'hui pour tous, jamais « combat
255 »), quêtes, points de quête, outils (`shop` : une boutique F2P le
vend), stock d'un lot, capital d'un lot, pas d'Ironman, disponibilité de
chaque objet acheté (`market` : unités/6000, unités par lot, asks, flux
récents = unités des derniers fills, affichés jamais comptés (D21),
boutiques, coté ou non), sources manquantes (`source` : impossible
aujourd'hui). Un lot = un paquet de 28 objets non empilables que la
ligne consomme, sinon la fenêtre. Un objet que rien ne cote (jamais
échangé, aucun ask, aucune boutique) laisse la ligne NON COTÉE (`priced`
false, profit `null` à l'API, classée après les lignes cotées).
`feasible_for(ligne, bot)` (pure, unique) rend `feasible` |
`after_prereqs` (niveau, combat, quête, points, outil à acheter ou
obtenir, pièces d'un lot) | `infeasible` (site, source, stock < un lot,
détenu + asks + boutiques < un lot d'un objet acheté, objet non coté non
détenu, Ironman sur le marché). « À portée » (`within_reach`) =
faisable, ou après un outil ou des pièces seulement : ce que lisent D17
(meilleure ligne cotée à portée) et les offres du cerveau. Exemple JSON
d'une condition : `{"kind": "market", "text": "buy naturerune 28/lot:
asks 0, recent 40, shops 0", "impossible": false, "item": "naturerune",
"cause": "rune", "per_6000": 1000, "per_lot": 28, "asks": 0, "recent":
40, "shops": 0, "priced": true}` ; faisabilité : `{"state":
"infeasible", "within_reach": false, "pending": [], "reasons":
["naturerune: holds 0, asks 0, shops 0 of 28 per lot"]}`.
- money making, profit/6000 au prix courant = Σ flux signés × prix,
  pauses comptées dans les ticks ; familles farm, recipe (intrants au
  prix de vente, D5), chain (D16 révisé 2026-11-16 §3 : chaque intrant
  et consommable par la voie la MOINS COÛTEUSE, farmer — ses ticks au
  prix D17 du bot moins les autres flux de sa source — ou acheter — prix
  au volume, asks et boutique finie ; égalité = farmer ; prix D17 lu sur
  la meilleure ligne accessible HORS chaînes, la boucle est coupée là ;
  ticks par unité sommés sur les étages farmés, chaque étage avec son
  profit « vendu ici », la voie de chaque intrant affichée `routes` :
  objet, cause, farm/buy, source, coût farmé et coût acheté par unité ;
  une chaîne qui achète tout n'est plus une chaîne : la ligne market la
  couvre), market
  (D15 : intrants achetés, aller-retour place de Varrock ↔ atelier et
  attente market mesurée par lot, capital d'un lot, Ironman exclu) ;
  XP/6000 en bonus ; conditions (D22 ci-dessus) ; confiance (mesuré/estimé, n, âge, dispersion, échec, clé
  lue) ;
- entraînement : skill × tranche × activité × site × outil × mode
  (autonome, D13 « le bot fait tout lui-même » : chaque intrant de
  recette OBTENU par sa source autonome la plus rapide, récursive comme
  une chaîne D16 (`guide_chain::Sourcing::Farm` : aucun achat d'intrant
  en amont), temps compté, XP/6000 = XP de l'action ÷ (ticks de l'action
  + ticks d'obtention) ; intrant sans source autonome au niveau →
  condition `source` (impossible aujourd'hui), ligne non cotée, jamais
  achetée ; consommables de
  méthode (runes, munitions, appât, fil, nourriture ; décision
  2026-11-16 §1) PRODUITS de la même façon quand une activité les fait
  au niveau (sources du guide : jambes de production, drops et
  sous-produits — runes élémentaires par runecraft, plumes des poulets,
  loi par les drops de hobgobelins), leur temps compté ; tout drop
  compte, même rare (décision 2026-11-16 : aucun seuil de rareté, la
  ligne montre la lenteur, ex. 2 lois par fenêtre = 3000 ticks par rune ;
  test `a_rare_drop_is_farmed_in_autonomous_mode_however_slow`) ; seuls ceux
  qu'aucune activité ne fait restent achetés (péages et pièces jamais
  produits) ; la ligne affiche la voie de chacun (`routes`) ; marché : intrants achetés selon la règle d'achat ci-dessus,
  volume au-delà des asks + boutique → prix courant et condition (D22) ;
  un aller-retour place de Varrock et une attente market mesurée par
  ACHAT, un achat portant les intrants de toute la tranche de 5 niveaux
  (`guide_training::band_slots`, au moins un sac ; levier utilisateur
  2026-11-17 « entraînement marché plus fréquent » : la montée achète
  sa tranche d'un coup par des bids `need` à son volume, jamais un sac
  par visite au marché ; avant, l'attente de 1 224 ticks par sac de 28
  plafonnait toute ligne marché rapide à ~110 intrants/6000 ticks) ;
  conditions inchangées (disponibilité d'un sac, capital d'un sac) ; la
  ligne 100 % market du guide money garde un achat par sac ; une méthode qui ne
  consomme QUE des consommables (téléports, malédictions, munitions,
  appât : aucun intrant de stock) a aussi sa ligne marché (décision
  2026-11-17) : chaque consommable acheté au volume disponible (asks,
  boutiques), au-delà au prix courant, sa disponibilité en condition,
  même règle que les autres lignes marché ; aucune ligne
  marché quand la ligne autonome achète déjà tous ses consommables
  (aucun n'est produit : ce serait sa copie) ; pièces converties
  en ticks au profit de la meilleure ligne money making à portée, D17) ;
  ligne non cotée : aucune XP effective (comme aucun profit en money) ;
  sources d'une chaîne = jambes de production + activités du catalogue
  dont le prior des guides donne l'objet en sous-produit ou drop (gemmes
  brutes : 1 jet de minage sur 256, `mining_gem_table` de `mining.rs2`,
  dump `action_odds.json` `mining_gems` (`gamedata::action_odds::
  mining_gems`, test qui relit le script serveur), prior des guides
  seul ; drops des combats) ; XP/6000 = unités × XP du dump ; pièces/6000,
  pièces/XP, ticks jusqu'à la fin de tranche ; raccourcis de quête (D20,
  XP forfaitaire sur la durée mesurée des runs `quest:<clé>`) ;
- sans `level`, chaque méthode est lue à son propre seuil (vue
  catalogue) ; avec, un bot à ce niveau dans tous les skills, ses
  quêtes faites (`quests`) ; toute ligne reste listée, jugée pour le
  profil de la requête (`level`, `levels=skill:n,...`, `quests`,
  `held`, `coins` : clé `feasibility` de chaque ligne ; front : colonnes
  conditions et faisabilité, filtre « faisable maintenant / après
  prérequis ») ; SANS profil (aucune de ces clés, 2026-11-17) : aucune
  ligne jugée, jamais pour un bot fictif (D22 : chaque bot juge) —
  `feasibility` null sur chaque ligne, `judged` false à la racine ; le
  front n'en grise aucune (« not judged (no profile) »), seules les
  conditions impossibles pour tous restent en rouge ; avec un profil,
  les lignes `infeasible` pour lui sont grisées ;
- MAGASIN PRÉCALCULÉ (2026-11-17, `orchestrator::guides_store`) : aucune
  requête ne construit un guide. Le thread `guides` du daemon construit
  toutes les `REFRESH_PERIOD` (300 s) un jeu complet dans UN moteur
  (rendements, devis, sources et prix D17 partagés) : vues catalogue
  money et training, et une vue money + training par tranche (1-5 …
  96-99) construite au DERNIER niveau de la tranche (5, 10, …, 95, 99),
  pour un bot sans quête ni stock ; l'API `/api/guides/*` et le cerveau
  (R7, `guides_store::current()`, `Arc` partagé, `GuideSet::money|
  training(filtre)`) lisent la dernière version avec son âge (`age_ms`,
  `cycle_ms`, `cycle_cpu_ms`, `refresh_s`). Filtres appliqués en LISANT la
  vue (`planner::guide_read`), jamais en reconstruisant : `skill`,
  `family` (lignes retirées, ordre gardé) ; `level` = vue de sa tranche,
  activités au-dessus du niveau retirées, faisabilité et ticks de fin
  de tranche à ce niveau, rendements du dernier niveau de la tranche
  (exact à 5, 10, …, 99) ; `quests` = faisabilité, raccourcis de quête
  faits retirés, D17 du training relu dans la vue money avec ces quêtes ;
  une ligne chain garde les choix farmer/acheter du bot sans quête ;
  `held` = les seules lignes recette de la vue réévaluées sur ce stock
  (aux devis du cycle ; un objet que le cycle n'a pas coté → erreur
  typée). Ce que la lecture approxime est dit dans `approximate` (vide :
  lecture identique au guide construit avec ce filtre, vérifié sur la
  copie du 2026-11-17 et par `web_guides_tests::the_precomputed_set_
  serves_what_a_direct_build_gives`). `rates-rebuild <copie>
  --guides-store <dossier> [money|training:requête …]` construit le jeu
  hors ligne et en donne le coût (docs/resources.md) ;
- les bots CHOISISSENT dans ces guides (R7, 2026-11-17, `docs/planner.md`
  § « Choix des bots dans les guides ») : argent et toute montée, lignes
  jugées par `feasible_for` sur l'état live du bot, faisables d'abord,
  tirage pondéré parmi les lignes à ≤ 15 % du meilleur score (poids =
  score × profil), 5 % d'exploration des lignes à < 3 mesures ; les asks
  lus sont ceux du cycle. Un profil donné à l'API (`level`, `levels`,
  `quests`, `held`, `coins`) trie les lignes faisables d'abord (D22 Q1),
  le front aussi ; avec `held`, le guide d'entraînement ajoute les lignes
  `stock` (recettes sur le stock du lecteur, R7 (a)). Une ligne de bande
  demande la PORTE de l'activité dans sa compétence, jamais le dernier
  niveau de la bande (constat live 2026-11-17 : « attack 72->80: no
  attack line within reach »). Le trait rapide ↔ économe ne pèse PAS
  sur l'argent (décision 2026-11-17 : pas tant que la mesure live ne
  montre pas de manque) : `merit_money` = profit × affinité marché seule.

### Exécution 100 % market (R5, 2026-11-17)

Le guide money dit QUOI faire, `trade:craft/<recette>` le fait
(`docs/planner.md` § « Exécution 100 % market ») : pour une ligne market
cotée, rentable et à portée du bot (D22 : `feasible_for` sur ses niveaux,
combat, quêtes, avoirs, bourse libre et les asks du cycle — un intrant
qu'il détient compte), le bot achète tous les intrants au marché (jambes
`market:buy` existantes, au prix et au volume des asks du cycle, lot borné
par sa bourse libre, la profondeur du carnet et le volume d'une fenêtre de
la ligne), crafte avec le script de la recette, vend le produit par la
vente par défaut (motif `trade`, horizon d'un surplus, réserve = son coût
propre). Ironman exclu. Le guide reste la seule source du choix (lecture
du jeu précalculé, aucun calcul à la demande).

Suivi « profit prévu vs réalisé » (`trade_report.rs`, section de
`orchestrator economy`, tableau « 100 % market lots » de la page economy,
`/api/economy` clés `trades` et `trades_closed`) : une ligne par lot des
vies (`memory.trades`) : prévu = profit du guide × lot ; achats = deals
d'achat des intrants du compte depuis le verrou, plafonnés aux unités
planifiées ; craft = produits faits et leur rendement /6000 ticks de
travail face à celui du guide ; ventes = deals de vente du produit,
plafonnées aux produits faits ; réalisé = ventes − achats ; statut
`buying` | `crafting` | `selling` | `closed` (les deals d'un lot s'arrêtent
à l'ouverture du lot suivant du compte). Staff exclu (règle 11).
- le cerveau lit le guide d'ENTRAÎNEMENT (R6, 2026-11-17,
  `brain_train` : `guides_store::current()`, aucune construction à la
  demande) pour choisir la méthode d'une tranche cotée à la résolution
  (forge, magie dès confuse, cuisine, artisanat, feu, prière) : score
  d'une ligne pour le bot = ticks propres par XP + poids(thrift) ×
  pièces par XP converties au D17 du bot, une ligne marché ×(1,5 −
  market_affinity) (`planner::train_score`, `docs/planner.md` §
  « Entraînement par le guide et les traits ») ; meilleur score en
  tirage pondéré de R7 (`train_score::pick`, `guide_pick`) ; le guide
  money making est lu par les nœuds argent (R7, `brain_money`).

Une seule source de profits (2026-11-17, `decisions.md` « Une seule
source de profits ») : le guide money précalculé. L'ancienne table
`rates_profit` (rentabilité par activité × tranche tous sites et outils,
calculée avant les guides, avec ses aberrations : `enchant:strung_
sapphire_amulet` à 21,8 M/6000) est SUPPRIMÉE, avec `cached_profit_rows`,
`brain_bids::rate_rows`/`realizable_rows`, `market_demand::chain_profits`,
`brain_market::saturated_profit`, `market_fit::coin_rate` et le champ
`profit` de `/api/rates/table`. Chaque lecteur du cerveau lit
`guides_store::current()` via `brain_d17` (jamais un calcul à la
demande), sémantique R7 :
- D17 d'un bot (`brain_d17::bot_d17`) = meilleur profit/6000 des lignes
  cotées et gagnantes de SES vues (`brain_money::bot_rows` : la vue de
  bande de son niveau dans chaque compétence) à sa portée
  (`feasible_for(..).within_reach()`, jugé comme `guide_read::
  best_profit_in` : sans stock, sans bourse, asks du cycle ; Ironman :
  aucune ligne marché) ; un bot au même niveau partout lit exactement le
  D17 de `brain_train` ;
- taux de pièces d'un bot (`brain_d17::coin_rate`) = D17 / 6000, plancher
  `market_fit::MIN_COIN_RATE` (0,1 ; aucun jeu publié ou aucune ligne :
  le plancher) : réponse aux bids (`brain_bids::bot_rate`), alchimie
  (`brain_alch`), demande dérivée (`brain_market::derived_reserve`) ;
- `ReadyCtx::coins_per_tick` (temps d'un achat) = ce même taux, posé par
  `brain_market::guide_coins` (dans `fill_ctx`) pour tout nœud ; avant le
  premier jeu, le 0,5 du contexte aveugle ;
- profit d'une méthode d'un nœud argent/pièces (`method_profit`) = sa
  meilleure ligne cotée dans les lignes du bot (`brain_d17::
  method_profit_in`, clé d'activité) ; le guide valorise déjà ce que le
  marché absorbe (D12) : plus de saturation par-dessus ;
- poids des ambitions par type (`market_demand::kind_profits`) =
  meilleure ligne du catalogue par type (`brain_d17::kind_profits` :
  lignes cotées sans condition impossible pour tous, type de leur
  compétence de bande, chaînes comprises).

## Tendances

Signaux publics (carnet, exécutions) : pénurie et momentum, score lissé
borné ; bonus de rentabilité pondéré, anti-oscillation par lissage, durées
aléatoires et profils. Les bids des teneurs de marché ne sont jamais de la
demande (`market_demand::signals_from` : liquidité, pas consommation).

## Teneurs de marché (plan marché vague 3, décision Q6, 2026-11-16)

Des comptes DÉDIÉS cotent les deux côtés de TOUS les objets F2P
échangeables, à profit, informations publiques seulement (D22), sans
triche (règle 13) : leur stock vient de leurs deals.
- Univers (`gamedata::f2p::market_universe`) : les 1128 lignes F2P
  échangeables du catalogue du marché moins les 552 billets (`cert_*`,
  échangés dans le carnet de leur objet de base) et les pièces = 575
  objets, un carnet chacun.
- Capacité d'un compte (`lifescape_market::slice_capacity`) : banque F2P
  60 types (`bank.constant ^bank_free_slots`) + sac 28, moins 20 cases
  gardées (pièces 1, trousse permanente 4, nourriture 1, demi-sac 14 pour
  les livraisons), × 80 % de marge = 54 objets. N = ⌈575 / 54⌉ = 11
  comptes. Au plus 49 types de la tranche détenus à la fois
  (`market_maker::MAX_HELD_TYPES` = 55 − pièces − trousse 4 − 1,
  strictement sous la ligne du concierge) : aucun bid n'ouvre un 50e
  type, le concierge ne se déclenche jamais sur le stock du teneur (sa
  tranche est en plus dans la liste gardée du concierge).
- Tranches (`lifescape_market::partition`) ÉQUILIBRÉES PAR CAPITAL : chaque
  objet coûte son lot plafonné (ci-dessous) au capital équilibré
  (`balanced_capital` : point fixe Σ min(lot_i, 10 % × T) / 11, arrondi au
  millier, 100 000) ; d'abord les objets lourds (une unité vaut plus
  qu'un lot), puis les légers, chacun par id croissant vers la tranche de
  plus haut poids (hachage rendez-vous) qui a encore de la place (≤ 54
  objets) sous le plafond de capital de l'étape (capital déjà placé / 11,
  arrondi au millier, + un lot pour les légers), sinon vers la tranche la
  moins chargée qui a de la place. Couverture totale, sans recouvrement,
  déterministe. Stabilité retenue : un objet léger ajouté après les
  autres ne déplace rien ; un objet ajouté n'importe où en déplace peu
  (mesuré sur l'univers réel, 38 ajouts : médiane 3, au plus 26 objets
  déplacés sur 575, < 5 %).
  Besoin par tranche à la cible commune : 98 312 à 102 590 pièces (écart
  4,2 %), 49 à 54 objets. Un compte garde son indice de tranche à vie
  (vie : `profile.maker`).
- Capital cible COMMUN (`market_maker::common_capital_target`,
  `lifescape_market::common_capital_target`) : besoin d'une tranche = un
  lot d'achat (1000 pièces, ≤ 500 unités) sur chaque objet à son ancrage
  public (valeur boutique), chacun plafonné à 10 % du capital ; la cible
  est le plus petit millier T, à partir du capital équilibré, qui couvre
  le besoin de TOUTES les tranches à T : 103 000 pièces, identique pour
  les 11 teneurs (avant : 60 000 à 290 000 selon la tranche).
- Migration (`market_maker::migrate_role`, à chaque chargement de vie,
  idempotente, une ligne de journal) : un rôle écrit avant l'équilibrage
  garde son indice de tranche, sa phase et sa base de P&L, prend la cible
  commune et le nombre de tranches courant ; le contenu de la tranche
  suit la nouvelle partition. Teneur déjà en cotation : réassigné au
  redémarrage, sans perte de stock (ses cotations sur les objets sortis
  de sa tranche sont annulées à la passe suivante, `stale_quotes`, et ce
  stock part par la passe de vente comme tout reste hors tranche ; les
  objets entrés sont cotés à la passe suivante).
- Cycle de vie : le compte naît comme tout bot (`create-user --role
  market_maker`, tutoriel), joue (ambition opérateur `market_maker` :
  échelle de pièces 5 000 → 1 000 000, méthodes d'argent productrices)
  jusqu'au capital cible (pièces live sac + banque + porté,
  `brain_plan::holdings_count`, comparées par `switch_due`), puis bascule
  pour toujours
  (`market_maker::maybe_switch`, `lifescape_planner::become_quoting`).
  `orchestrator economy` et le tableau de bord affichent comme capital
  d'un teneur en phase `earning` CETTE bourse (lue dans
  `run/users/<compte>.json`, « n/a » sans instantané), celui d'un teneur
  en cotation = pièces à la bascule + P&L (2026-11-17 : la phase earning
  lisait 0, bascule + P&L n'existant pas encore). À la bascule :
  achats ouverts et annonces de sa tranche annulés (`role`), nœuds
  abandonnés, but `MakeMarket` = un nœud `market:make` après l'autre (le
  script `market make --post x,z,niveau` se tient à son poste, une case
  de l'anneau de remise de la fontaine de Varrock, mange, revient au
  poste après une livraison). Plus d'ambition, d'équipement, de souhait,
  d'alchimie ; restent la banque, les livraisons, la vente de ses restes
  hors tranche et la vente PNJ (Q3).
- Cotation (`lifescape_market::maker::quote`, passe démon toutes les
  1000 ticks, `market_maker::quote_pass`, motif `market_make`, prix fixe :
  jamais sur la courbe de concession) :
  juste valeur v = prix courant échangé, sinon milieu bid/ask des autres,
  sinon (jamais échangé, M7) le coût de production publié par les guides,
  borné par plafond / (1 + h) où plafond = min(valeur boutique, meilleur
  ask des autres), à défaut plafond / (1 + h) ; AUCUN plancher (décision
  2026-11-18 : « les teneurs doivent s'adapter au marché ») ; lot =
  1000 / v (1..500) ;
  biais de stock r = clamp((stock − lot)/lot, −1, 1), prix de réserve
  p = v (1 − h r) ; bid = ⌊p (1 − h)⌋ ≤ v − 1 ; ask = ⌈p (1 + h)⌉ ≥ v + 1,
  > bid (peut passer sous ce que paie une boutique). h = 4 % (liquide : achats d'autres
  ≥ un lot sur 72 000 ticks), 8 % (mince), 10 % (découverte, quart de
  lot, jamais relevé par un stock bas). Stock haut → bid ET ask baissent ;
  stock bas → les deux montent. Sans demande (échangé, aucun bid d'un
  autre, aucun achat d'un autre depuis 72 000 ticks) : le bid descend de
  10 % par passe jusqu'à 1, le rythme des vendeurs (plus de bid plancher) ;
  un ask du teneur encore invendu au passage suivant baisse aussi de 10 %
  (jusqu'à son bid + 1), quel que soit le dernier prix échangé, le stock au-delà d'un lot part au magasin PNJ
  (Q3, `take_dumps` → plan de vente de `brain_sales`). Plafonds :
  capital engagé par objet (stock à max(valeur, coût) + bid) ≤ 10 % des
  fonds propres, stock ≤ 500 unités, part d'un côté ≤ 25 % de la
  profondeur des autres (seul sur un côté : un lot).
- Simulateur (`sim_maker`, `sim_lifecycle::run_with_maker`, 5 graines,
  40 objets façon constat) : voir `decisions.md` 2026-11-16 vague 3.
- Opérateur : section « market makers » d'`orchestrator economy` et carte
  « Market makers » de la page `economy` (capital, P&L, écart moyen
  coté, part du carnet, stock par objet ; `market_maker_report`).
- Financement (utilisateur 2026-11-17, `orchestrator fund-makers`,
  procédure dans `orchestrator.md`) : les comptes les plus riches de la
  flotte donnent aux teneurs encore en jeu le manque à la cible commune
  (capital = pièces sac + banque, lu comme la bascule), chacun gardant
  une réserve nommée (bids et deals acheteurs engagés, prochain achat
  d'équipement et d'outil, marge 10 % ≥ 5 000) ; répartition gloutonne
  du plus riche vers le plus grand manque (`funding_plan`). Un don est un
  deal de son propre genre (`funding_gifts`) : objet coins, prix 0,
  vendeur = donateur, acheteur = teneur ; il suit toute la remise
  physique (convocation, départs échelonnés, partie absente ouverte,
  `market deliver`, double écran, balayage) mais ne touche ni carnet, ni
  registre, ni exécution, ni bougie, ni fiabilité, ni relist ; son id
  (≥ 10^12) n'a pas de ligne en base, l'état vit dans le daemon et le
  journal `run/funding.jsonl`. Le teneur rapporte les pièces reçues
  (`received`, comparées au montant). Échec typé retenté une fois, puis
  abandonné (section « funding » d'`orchestrator economy`). Ce n'est pas
  de la triche (règles 12-13) : de vrais échanges en jeu entre comptes
  de la flotte, jamais le staff, jamais un ordre au marché.

## Front et base

Page `market` (prix au-dessus des bâtons de volume ; vues 24 h en
bougies de 15 min, 96 créneaux, `res=15m` (demande utilisateur
2026-11-17), 7 j / heure (`1h`), 30 j et 6 mois / jour (`1d`) ; dernier
point temps réel par le WS, replié dans le créneau en cours de la vue
(le quart d'heure en 24 h, `chartRows` de `market.js`) ; carnet +
dernières exécutions ; canvas maison, zéro dépendance). La fenêtre
demandée est recalée sur les bornes des bougies (`web::snap_range` :
900, 3600 ou 86400 s) pour que la bougie en cours reste dedans. Bougies continues, sans trou (règle
utilisateur 2026-11-13) : chaque bougie ouvre à la clôture de la
précédente (haut/bas élargis à cette ouverture), une fenêtre sans
échange est une bougie plate grise à la dernière clôture (volume 0), de
la première exécution connue jusqu'à la fenêtre en cours. Le chaînage est
fait à la lecture côté serveur (`store::candle_chain`, op `history`,
`prev_close` = dernier prix avant la fenêtre) ; la table `candles` garde
les agrégats bruts par fenêtre et par résolution (`15m`, `1h`, `1d` :
`YieldCandleResolution::ALL`), tous repeints ensemble à chaque échange
réglé (`Store::rebuild_all_candles`, une transaction). Les `15m` de
l'historique existant sont reconstruits depuis `executions` par la
migration 7 du store (au démarrage, dans la transaction de migration) ;
une fenêtre sans bougie en cache mais avec des exécutions est
reconstruite à la demande (`candles_or_rebuild`). Le front rechaîne la queue live
(`LifeChart.carryFlat`) et ses créneaux finissent sur la fenêtre en cours
(`LifeChart.liveSlots`). L'anneau des prints (`trades`, `tick`, WS) est
rechargé depuis `executions` au démarrage du daemon. Page rendements (vue mesures, § Rendements) : tableaux activité ×
site × outil × tranche, guides money making et entraînement, nombre de bots engagés
par script (information pour l'opérateur humain uniquement, jamais lue
par les bots). Page `life` (`/api/lives`, `/api/life/<compte>`) : but,
profil, engagement en cours, graphe des tâches (statuts colorés,
méthode choisie et alternatives avec coûts estimés, avancement,
ticks dépensés), historique des issues. Page `economy`
(`/api/economy`) : deals
(réglés, ratés, taux d'échec), top activités par profit/6000 ticks
(guide money précalculé, `web_lives::guide_activities` : vue catalogue
de `guides_store::current()`, mêmes chiffres que la page guides ; les
10 lignes cotées sans condition impossible pour tous, avec famille,
méthode, tranche, confiance ; jamais l'ancienne table `rates_profit`),
teneurs de marché
(tranche, phase, capital cible, capital, P&L, écart moyen coté, part du
carnet, stock), lots 100 % marché (masse monétaire, top holdings et
tendances retirés de la page le 2026-11-17 ; les tendances restent
calculées dans le store et lues par le cerveau). Les bots ne lisent
aucune de ces pages.

Base `lifescape-store` (SQLite, daemon seul) : ordres, deals,
exécutions, bougies, rapports, rendements, fiabilité, tendances.

## PRD3 : économie centrale

- Ventes par défaut : le cerveau liste le surplus échangeable des bots
  inactifs (`brain_sales`, toutes les 600 s) par LOTS (§ Réserve
  vendeur), prix qui marche ensuite avec la demande sans passer sous la
  réserve ; protégés : pièces, nourriture ≤ 100, keep-list du graphe,
  intrants de tout ce que le plan du bot travaillera
  (`brain_sales::planned_methods` : méthodes et alternatives des nœuds
  niveau/argent ouverts, tranche d'un nœud niveau pas encore résolu,
  tranche de chaque jalon niveau et production de chaque jalon produire
  non finis de l'ambition ; bûches d'un futur allumeur, poisson cru
  d'un futur cuisinier, minerai/barres d'un fondeur/forgeron, runes
  d'un sort de combat) et un produit qu'une autre méthode ouverte
  consomme, outils, porté, pièces de l'échelle d'équipement et palier
  suivant, envies. Les produits fabriqués (barres, forgé, artisanat,
  cuisine) sont vendus. Les annonces sans bornes (posées avant un
  redémarrage) sont réadoptées à chaque passe, marchands compris
  (`market_seller::adoption`) : sous la réserve → relevée à la réserve,
  au-delà du lot de demande ou sans coût connu → retirée (stock rendu à
  la banque, la passe suivante liste un lot), au-dessus de l'ouverture
  du jour → descendue à elle, sinon marche jusqu'à la réserve (voir
  § Butin).
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
  27 intrants, 13+13 pour le bronze ; un outil = 1 ; une obtention de N
  barres demande N × l'intrant de la recette ÷ les chances de succès :
  2 minerais de fer par barre de fer, `Way::success` 0,5 lu dans
  `smelting.rs2`), résolue marché
  d'abord ; la progression de niveau remet le compteur de blocages à zéro
  (une charge par blocage est le rythme normal).
- Demande (2026-11-13) : `market_demand::signals()` (offres d'achat au
  repos par objet : nombre, acheteurs, quantité non servie, meilleure
  offre, âge, poids) et `kind_profits()` (guide money, `brain_d17`)
  nourrissent les ambitions (voir `docs/planner.md` § Ambitions) ; lecture seule.
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

## Équipement obtenable maintenant (2026-11-18)

Un bot marché ne vise que les pièces obtenables MAINTENANT
(`gear_plan::obtainable_now`) : un ask au carnet, une boutique que ses
points de quête ouvrent, ou une voie de fabrication dont un bot de la
flotte a le niveau (`brain_gear_fleet`, niveaux relevés à chaque passe
d'équipement, staff exclu ; le plan attend 75 % de la flotte relevée
après un démarrage du daemon). Live : felix_rook (défense 76) enchérissait
sur un rune kiteshield que personne ne vend ni ne forge pendant que 529
mithril kiteshields restaient invendus.

## Équipement par paliers (2026-11-13)

Remplacé le 2026-11-14 pour le CHOIX des pièces par § Équipement : le
meilleur abordable (tous les emplacements à la fois) ; les mécaniques de
pas (`insert_gear_step`, retrait des pas morts, `next_ready`) restent.

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

1 pièce n'est que le prix d'amorçage public. Règles
(`lifescape_market::zip`, `market_reprice`) : ouverture à la référence
échangée (VWAP 24 h → dernier print), `2 × réserve` sinon, puis un pas
de 5 % (≥ 1) par fenêtre de 2000 ticks selon les exécutions de la
fenêtre (voir § Market). Un vendeur qui ne vend pas descend jusqu'à sa
réserve (son propre coût, § Réserve vendeur) et y reste ; un vendeur
qui vend monte ; un acheteur qui n'est pas servi monte jusqu'à ce que
son portefeuille paie. Le prix suit donc la masse monétaire : plus de
pièces → plafonds plus hauts → prix plus hauts.

## Réserve vendeur, mémoire des fills, anti-dumping (2026-11-14)

Constat 2026-11-14 : 104 carnets actifs sur 117 à 1 pièce, 84 % des
deals à 1, masse monétaire ×25 en 11 h. Causes : plancher vendeur =
référence/2 avec référence = VWAP 24 h pondéré en quantité (un gros dump
abaisse le plancher : saule 13 → 6 → 2 → 1) ; objets jamais échangés
ouverts à 1 ; stock de revente des marchands (rôle supprimé en vague 3)
relisté à la référence (ryan_park : 81 % du côté vente à 1) ; lots
entiers jetés d'un coup.

Toutes les entrées sont les données PROPRES du bot (ses rapports, ses
fills) ou publiques (carnet, prints) ; jamais l'état privé d'un autre.

- Réserve (`brain_reserve`) = ticks qu'une unité a coûté AU BOT × ses
  pièces par tick, arrondi au supérieur (≥ 1). Ticks par unité mesurés
  sur ses 200 derniers rapports : les ticks d'un rapport sont partagés
  entre ses lignes de farm (`produced`, `loot`, `ground_spawn`, pièces
  ramassées) au prorata `quantité × valeur de jeu` (pièces à 1) ; un
  rapport de récolte ou de production donne tous ses ticks à son
  produit, un rapport de combat sa part de butin à chaque drop. Une unité
  fabriquée ajoute les intrants qu'elle a mangés (lignes `consumed` de
  ses rapports, même partage, `measured_inputs_per_unit`), chacun à SON
  coût propre (ticks mesurés × pièces par tick + ses propres intrants,
  3 niveaux, sinon a priori, jamais sous son dernier prix d'achat ;
  `OwnCosts::unit_cost`) : casque de fer = ses ticks + 2 barres, une
  barre = ses ticks de fonte + son minerai. Non
  mesuré : l'estimation du planificateur (`est_ticks` / quantité d'une
  alternative de farm d'un nœud d'obtention du graphe du bot), sinon le
  taux de démarrage à froid du script (`rates_service::prior_for`).
  Pièces par tick : la meilleure méthode du bot, (pièces ramassées ou
  vendues au magasin + unités farmées, nettes de la vente magasin, × son
  propre dernier prix de vente) / ticks, jamais sous
  `market_fit::MIN_COIN_RATE` (0,1, le plancher du coût unitaire de
  `bid_response` : le temps n'est jamais gratuit ; saule à 43,6 ticks →
  5 pièces). Un objet ACHETÉ coûte au moins son propre dernier prix
  d'achat. Plafond partagé avec la réponse aux offres : la réserve ne
  dépasse jamais le coût de FABRICATION que `bid_response` donne au bot
  (`bid_response::production_cost` = chemin « make » de `unit_cost`,
  pièces + ticks × taux du bot (son D17, `brain_d17::coin_rate`) ; calculé une fois par passe
  par `brain_bids::production_costs_for`, mêmes niveaux, bans, quêtes,
  rendements et carnet que le tirage), arrondi bas, ≥ 1 : un quota compte
  les bids au-dessus de ce coût, le producteur ne garde donc jamais le
  stock que ces bids achèteraient. Butin (`loot`, `ground_spawn`,
  `drop`) : coût nul, réserve `Found` à 1 (§ Butin) ; aucune trace
  propre : coût de fabrication si le bot sait le faire, sinon trouvé.
- Ouverture (`zip::seller_open`) : référence échangée (ou meilleur bid
  plus riche) ; objet jamais échangé : `max(meilleur bid, 2 × réserve)` ;
  jamais sous la réserve ; ramenée à 5× d'une référence échangée (bande).
- Mémoire des fills (`market_seller`, `Engine::own_sells` + `deals`) :
  après une vente RAPIDE (servie ≤ `FAST_FILL_TICKS` = `PATIENCE_TICKS`
  = 2000 ticks après la mise en vente, soit dans la première fenêtre,
  avant tout pas de descente, durée mesurée
  depuis `QuoteBounds::listed_wall` ; un ask qui croise un bid à l'arrivée
  = 0), le lot suivant ouvre à ≥ 105 % de ce prix. Après redémarrage le
  prix revient du store (deals non échoués du bot), la vitesse non (pas
  de hausse). Une offre d'achat ouvre à son propre dernier prix d'achat
  (`remembered_bid`), l'ask couvert restant pris d'abord.
- Anti-dumping : un lot = `market_fit::sale_lot` (bids des AUTRES
  comptes + volume 24 h, au moins 28, plafonné au stock libre ; une
  seule règle avec la réponse aux offres) ; le reste reste en banque, le
  lot suivant part quand le précédent n'a plus de ligne au repos
  (vendu). Jamais sous la réserve, jamais d'expiration qui brade. Ce que
  le plan du bot consommera n'est pas du surplus : jalons ouverts de
  l'ambition par `market_fit::own_use_items` (`cleanup_keep`), nœuds
  ouverts du graphe par `brain_sales::planned_methods` (tranche d'un nœud
  niveau non résolu, runes d'un sort de combat). Mêmes règles pour le
  `market sell` et le `market cleanup` des workers (op `seller_quote`,
  niveaux live lus au superviseur pour le plafond de fabrication).
- (Marchands : cotation de revente supprimée en vague 3, remplacée par
  les teneurs de marché, § Teneurs de marché. La tranche d'un teneur qui
  cote n'est jamais relistée par `maybe_list_surplus`.)
- Coût CPU du repricing (2026-11-14, ~55 amends/s et ~10 000 lignes de
  log en 3 min après le redémarrage) : patience 2000 ticks (était 200) ;
  un ordre dans sa fenêtre est sauté avant toute lecture du store ; un
  ordre parqué à une borne (réserve, plafond) ferme sa fenêtre sans
  amend ni log ; une référence lue une fois par objet et par passe ; les
  amends d'une passe partagent une transaction (`Store::begin_batch` /
  `commit_batch`) ; une ligne de synthèse par passe (`market-reprice: N
  moved, P at a bound, R refused`). Adoption idempotente : un ordre
  adopté (ou relevé, retiré) une fois n'est jamais repris
  (`Engine::adopted`, `market_reprice::handled`), une ligne de synthèse
  par compte et par passe.
- Plafond acheteur (décision utilisateur) : le plafond « juste »
  `min(pièces/qté, max(3 × réf, 20))` est retiré
  (`resolve_obtain::bid_ceiling` = pièces / quantité) ; aucun plafond lié
  à la richesse.

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

## Producteurs qui répondent aux offres (point 7, 2026-11-14)

À CHAQUE tirage de but (`brain_succession::draw_next`, avant le jalon
de l'ambition, pas seulement quand les compétences sont bannies), le
cerveau lit le carnet PUBLIC (`brain_bids` : ordres au repos du moteur,
ses propres offres exclues, volume 24 h des exécutions) et demande au
planner (`planner/src/bid_response.rs`, pur) s'il existe une offre
d'achat que ce bot sait fabriquer avec profit :
- Coût unitaire (`unit_cost`, récursif sur la recette, 3 niveaux) : le
  moins cher de ACHETER (asks au repos qui couvrent la quantité) et
  FABRIQUER (jambe de production praticable MAINTENANT : niveau atteint
  et compétence non bannie, quête faite, outils détenus ou achetables —
  ask au repos, ou magasin PNJ si D-A l'autorise —, site hors guilde ;
  ses ticks = 6000 / unités par 6000, mesurées sinon a priori, sinon 600
  fabrication / 200 récolte ; chaque intrant à SON coût unitaire). Un
  stock détenu coûte ce que sa vente rapporterait (meilleur bid public)
  et aucun temps. Total = pièces + ticks × taux du bot.
- Taux du bot (`brain_bids::bot_rate` → `brain_d17::coin_rate`) : son
  D17 / 6000, le meilleur profit du guide money à sa portée (niveaux,
  combat, quêtes, Ironman), plancher 0,1 pièce/tick (le temps n'est
  jamais gratuit).
- Profondeur : bids qui paient plus que le coût total ; moins les asks au
  repos à ≤ 125 % du meilleur bid (offre déjà en route : ces vendeurs
  descendent vers les bids) ; moins le stock libre du bot (il le vend
  sans fabriquer) ; le bot en prend la MOITIÉ (`PRODUCER_SPLIT`, arrondi
  haut, ≤ 100). Revendre l'objet acheté n'est pas produire (marchands).
- Règle de score : taux brut = (revenu des bids pris − pièces dépensées)
  / (ticks de production + trajet réel du lot, § Rentabilité de tout le
  marché ; l'ancien forfait de 800 ticks par réponse est retiré). L'offre
  passe DEVANT l'ambition si ce taux ≥ le taux propre du bot (aucune
  marge : `BID_PREEMPT` retiré le 2026-11-18) et si le net après le temps ≥ 50 pièces
  (`BID_MIN_PROFIT`). Entre plusieurs, le meilleur taux, incliné de
  ±15 % par un mot propre au bot et à l'objet.
- Étalement (sans état d'un autre bot) : un bot ne répond à un objet que
  dans une fenêtre de 6000 ticks sur deux choisie par son mot
  (`answers_now`, rotation par fenêtre) ; avec la moitié de la
  profondeur par producteur, les bots capables ne se ruent pas tous sur
  le même carnet.
- But : `GoalKind::Produce{objet, N}` (jambes de production seules,
  intrants par les sous-tâches, marché d'abord), origine `bid:<objet>` :
  aucun jalon, ambition intacte, le tirage suivant la repèse. Log
  `brain <compte>: answers bids: produce N <objet> (best bid …, unit …
  coins + … ticks, … coins/tick vs own …, net …; plan)`. Le produit fini
  sort de la keep-list et part par la vente par défaut (`brain_sales`,
  ouverture au meilleur bid, pas de 5 % ensuite).
- Iron : jamais.

## Saturation et usage propre (« les bots jouent comme des joueurs », 2026-11-14)

`planner/src/market_fit.rs` (pur, carnet public + état du bot) :
- `saturation_factor(vue, lot)` = (profondeur des bids + volume 24 h) /
  (asks au repos + lot), borné [0, 1] ; 1 sans ask (le premier vendeur
  fixe le prix) ou sans donnée. Lu par `bid_response` (ce qu'un lot
  ajoute à un carnet) ; les méthodes d'argent n'appliquent plus ce
  facteur (2026-11-17) : leur profit est leur ligne du guide money, qui
  valorise déjà le volume absorbé (D12 : le prix courant ne vaut que pour
  le volume de ses 5 derniers fills, puis bids, PNJ, sinon 0).
- `own_use_items(ambition)` : ce que les jalons ouverts du bot
  consommeront (chaque tranche d'un jalon de niveau, de son niveau live à
  la cible, `sizing::tranche_consumption` : bûches de l'allumeur, poisson
  cru du cuisinier, minerais de la fonte, os du prêtre, runes, flèches ;
  intrants d'un quota et les leurs : barres puis minerais). Ajouté à
  `brain::cleanup_keep` ⇒ ni vendu (`brain_sales::working_keep`) ni
  jeté au rangement : un allumeur coupe ses bûches pour les brûler.
- `sale_lot(stock, vue)` = ce que les acheteurs prennent (bids + volume
  24 h), au moins un inventaire (28), jamais plus que le stock : taille
  d'une annonce de joueur. Câblé (2026-11-14) dans la vente par défaut,
  l'adoption des annonces et `seller_quote` (`market_seller`, § Réserve
  vendeur).
- Origine des volumes absurdes (revue 2026-11-14, `task_yields`) :
  plumes ≈ 115 000 en montées de combat sur les poulets (butin, jamais
  lâché) ; saule ≈ 8 900 en tranches de bûcheronnage, 6 500 en `make_money`
  et ≈ 11 500 en obtentions : b8jxrdd2 n93/n109 = `obtain willow_logs
  2359` d'une tranche d'allumage 30→45 (`tranche_consumption`), chaîne
  abandonnée puis 2 035 bûches listées à 1 (corrigé par l'usage propre) ;
  paul_rhodes n60 `obtain oak_logs 308` a coupé 3 187 saules en 106 839
  ticks (le script coupe un autre arbre que la ressource du nœud : bug
  script hors de ce lot). Les quotas des échelles (100–200) sont sains.

## Prix courant : les 5 derniers fills (2026-11-14)

Décision utilisateur : plus aucune décision de bot ne lit la VWAP 24 h.
Un bot price comme un joueur qui regarde les dernières ventes :
`lifescape_market::current_price` = médiane des 5 DERNIERS fills de
l'objet (quel que soit leur âge ; nombre pair = moyenne des deux du
milieu), bornée par les PRIX D'IMPACT du carnet vivant (§ Borne
d'impact : un bid d'impact plus haut est le prix du moment, un ask
d'impact plus bas aussi). Jamais échangé : milieu du carnet, sinon son
seul côté, sinon 1 pièce, `traded: false` (les règles d'ouverture
s'appliquent : `seller_open` ouvre à `max(meilleur bid, 2 × réserve)`).
Branché partout où la référence décidait : `MarketService::reference`
(ouvertures `opening_ask`/`opening_bid`/`remembered_bid`, repricing,
vente, réserve, gear, résolution des nœuds), `rates_price::price_estimate`
(profits publiés, marges de demande `market_demand`, chaînes
`planner::profit`), la cotation `quote` et le prix de la page marché.
Le « % 24 h » de la page reste un affichage (VWAP du jour contre la
veille), aucun bot ne le lit.

CPU (« attention au cpu ») : jamais de scan de l'historique d'un objet.
Le service garde un anneau en mémoire par objet (`Engine::last_fills`,
5 fills (prix, quantité)), chargé une fois par une lecture indexée
`LIMIT 5` (`Store::last_fills`, index `executions(item_id, clock)`), puis
poussé à chaque fill réglé (`settle_print` → `Engine::note_fill`). Avant,
chaque `reference()` lisait TOUTES les exécutions de l'objet depuis 0.
Le volume 24 h du `seller_quote` et des bids est un `SUM` indexé
(`Store::volume_between`), plus les lignes. `price_estimate` (rates) : 3
lectures indexées (5 fills, puis les meilleurs bids et les meilleurs
asks jusqu'à couvrir la taille d'impact, `Store::book_levels_covering`,
`LIMIT 64` ordres par côté, arrêt dès la taille couverte).

### Borne d'impact (2026-11-14)

Défaut corrigé : borner par le MEILLEUR bid/ask laissait une poussière
fixer le prix — une unité listée à 1 pièce ramenait à 1 le prix d'un
objet échangé par lots de 100, pour tous les bots (le cliquet à 1 pièce
historique). Comme le « mark price » des dérivés (prix d'impact), la
borne lit maintenant le prix qu'il faut pour remplir un lot réel :

- taille d'impact N = médiane des QUANTITÉS des 5 mêmes derniers fills
  (`lifescape_market::impact_size`, au moins 1, plafond `IMPACT_CAP` =
  1000) ; jamais échangé → N = 1 ;
- ask d'impact = prix de la N-ième unité en remontant les asks au repos
  depuis le meilleur ; bid d'impact pareil sur les bids
  (`impact_price`) ; un côté dont la profondeur totale est < N ne borne
  PAS (un côté mince ne fait pas le prix) ;
- prix courant = médiane des 5 derniers prix bornée dans
  [bid d'impact, ask d'impact] (bid d'abord, puis ask, l'ask gagne si
  le carnet est croisé) ;
- jamais échangé (aucun fill) : règles inchangées (milieu des deux
  côtés, sinon le seul côté, sinon 1), avec N = 1 donc les meilleurs
  prix.

Exemples (tests `price.rs`, `market_impact_tests.rs`) : plumes en lots
de 100 à 4, une unité à 1 → prix 4 ; 150 unités à 2 → prix 2 ; 10 unités
bidées à 9 → prix 4 ; 90 de plus à 7 → prix 7 (la 100e unité). Les
bornes de la bande anti-bug ×5/×0,2 lisent toujours `guard_anchor` =
cette référence, `traded` garde son sens (vrai dès un fill).
Coût : `MarketService::reference` parcourt dans le carnet mémoire les
quelques meilleurs ordres de chaque côté jusqu'à N unités
(`Book::levels_covering`), une passe, jamais le côté entier.

## Équipement : le meilleur abordable, tous les emplacements (2026-11-14)

Constat live : 25 combattants niveau 40-58 en heaume moyen de bronze et
dague de fer ; seuls des cimeterres de BRONZE montaient au carnet (18
offres, 29 → 7321), un emplacement à la fois (`gear_ladder` : palier
suivant de l'arme d'abord), jamais un plastron, des bottes ou une
amulette ; les obtentions d'anciens buts `equip_best` (`iron_platebody`,
`amulet_of_power`, `black_cape`, `leather_boots`…) ouvraient à 1 et
étaient annulées après 6000 ticks à 1-20 pièces (« unfilled after 6000
ticks », ~1 000 à 1 600 fois chacune dans les lignes « graph stuck »).

Règle (`planner/src/gear_plan.rs`, pur ; câblage `brain_gear.rs`, une
passe par compte toutes les 60 s, tout style de but, jamais Iron) :
- Emplacements : arme, corps, jambes, tête, bouclier, pieds, mains, cou,
  cape. Candidats par style (calculés une fois) : objets F2P échangeables,
  obtenables sans quête, score de style > 0 (`gear::score` : mêlée =
  bonus + 2 × force, distance = arc et cuir, magie = bâton et robes) ;
  mêlée et magie à une main (bouclier), distance à l'arc (pas de
  bouclier) ; munitions et runes restent des consommables des tranches.
- Le bot veut, par emplacement, la meilleure pièce que ses niveaux
  ouvrent et qui bat ce qu'il possède (sac, banque, porté). Le choix de
  l'arme suit le score (hache de bataille, cimeterre ou épée longue).
- Budget = 50 % (`GEAR_SHARE_PCT`) du portefeuille libéré de ses achats
  NON équipement ; prix attendu = médiane des 5 derniers fills, sinon la
  valeur de haute alchimie (personne ne vend dessous) ; tant que la somme
  dépasse le budget, l'emplacement le plus cher descend à sa pièce
  suivante moins chère (ou sort). Plafond de chaque envie = sa part du
  budget (attendu × budget / total) ; l'offre ouvre AU PRIX ATTENDU
  (`market_reprice::keep_bid_from`, jamais à 1) et monte avec la patience.
- TOUS les emplacements enchérissent à la fois. Un ask au repos sous le
  plafond, ou une pièce que seul un magasin vend (D-A), devient un nœud
  `obtain_items` + `equip_best` (un pas ouvert à la fois, `shop:buy`
  ajouté pour le seul cas D-A). Une pièce possédée meilleure que la
  portée → `equip_best`. Les offres sur des pièces que le plan ne veut
  plus sont annulées.
- Gardé (jamais vendu) : par style et emplacement, la meilleure pièce
  portable et toute meilleure pas encore portable (`kept_gear`) ; les
  pièces remplacées sont du surplus (vente par défaut, ou alchimie d'un
  lanceur). La liste d'envies (`brain_wishlist`) ne garde que les outils.

## Trousse permanente : hache, pioche, marteau, tinderbox (2026-11-14)

Constat live : personne au-delà d'une pioche de bronze ; 185 offres
« wish … pickaxe » jamais remplies (aucune recette F2P ne forge de pioche
en rev 274, aucun monstre F2P n'en lâche : seul Nurmof, `pickaxeshop`,
Dwarven Mine, vend fer → rune) ; puis décision utilisateur : chaque bot
(Iron compris) possède en permanence une hache, une pioche, un marteau et
une tinderbox, et ne les revend plus en changeant de but.
Règle (`planner::tool_kit`, passe `brain_tool_shop.rs` de 60 s, banque
diffusée, avant le plan d'équipement) : trousse de base d'abord (marteau,
tinderbox, pioche, hache manquants : meilleur palier ≤ 25 % du portefeuille
libre, sinon le moins cher ≤ 50 %), puis améliorations (meilleur palier que
le niveau ouvre au-dessus du palier qui travaille, ≤ 25 % `TOOL_SHARE_PCT`).
Chaque achat devient un pas d'équipement (`file_step` : un pas à la fois,
cooldown `WITHDRAWN_RETRY_SECS`, jamais de bannissement) : `shop:buy`
(Nurmof, magasins généraux) ou, pour une hache, ask couvert puis Bob's
Brilliant Axes (exception D-A `kit_shop_route`, bronze → acier). Aucune
offre marché sur un outil vendu en boutique (`brain_wishlist::biddable`),
les anciennes sont annulées ; les haches mithril+ (forgées seulement)
gardent l'offre de la liste d'envies. Garde (`brain_tools::kept_tools`) :
meilleure hache/pioche détenue + meilleure qui travaille, 1 marteau,
1 tinderbox ; le reste se vend. Les outils inutilisés restent en banque :
chaque script ne garde au sac que son outil (`ToolSpec::keep_ids`), le
voyage en banque dépose le reste une fois, rien ne les retire hors besoin.
La Dwarven Mine est un étage de boutiques routé
(`access::shop_floor_routed`, trappe de Falador) : la cueillette et les
fermes de drop y restent refusées.

Depuis le 2026-11-17 (décision utilisateur, cas jim_ellis, decisions.md) :
la cible d'une famille est le meilleur palier OBTENABLE MAINTENANT
(boutique, meilleur ask du carnet, production propre au niveau ;
`planner::tool_upgrade`), jamais le palier absolu ; au-dessus du quart du
portefeuille, l'outil s'achète quand les ticks qu'il économise sur le
travail planifié dépassent son prix en ticks du bot, financé par un pas
`have_coins` du prix si les pièces manquent (`tool_kit::kit_purchases_in`,
`brain_tool_shop`). Le bid patient sur les paliers introuvables reste (Q8).

## Producteurs : entraînement pour une offre (2026-11-14)

Constat live : personne ne forgeait de cimeterre à 2 812 pièces parce
que TOUTE la flotte avait forge 1 (sara_miller 18, harry_page 10,
bh99tw2f 7) et que `bid_response` n'ouvrait que les jambes praticables
au niveau ACTUEL (cimeterre de bronze : forge 5). Les tirages, eux, sont
fréquents (~120 par heure pour ~25 bots) : la fenêtre n'était pas le
frein. Depuis : une jambe jusqu'à 15 niveaux au-dessus
(`MAX_TRAIN_LEVELS`, RETIRÉ le 2026-11-19 : voir § Rentabilité de tout
le marché) d'une compétence de production (forge, artisanat,
cuisine, minage, pêche, bûcheronnage, allumage, runes) compte pour une
offre (`unit_cost_training`) ; l'entraînement (méthode de tranche
`reach_method`, actions par la table d'XP serveur, rythme du bot,
intrants à leur coût unitaire) est un coût unique du lot, ajouté aux
pièces et ticks de l'opportunité (`training_cost`). Le but `Produce`
bloque sur le niveau (`missing:level:…`) et s'entraîne d'abord. Le
plafond de la réserve vendeur (`production_cost`) reste sans
entraînement.

## Alchimie (2026-11-14)

`planner/src/alchemy.rs` (pur) + `orchestrator/src/brain_alch.rs` :
- Sorts de la table serveur : haute alchimie (magie 55, 1 nature + 5
  feu, 65 XP), basse (21, 1 nature + 3 feu) ; un bâton de feu paie le
  feu. Valeur = `alch_high`/`alch_low` des configs (coût × 0,6 / 0,4).
  Net d'un lancer = valeur − runes au prix courant ; 1 lancer par 5 ticks.
- Méthode d'argent : la meilleure alchimie du stock propre du bot (hors
  keep-list, gear gardé, verrous d'annonces), seulement si le net bat la
  vente (meilleur bid, sinon prix courant), est publiée `alch:<objet>`
  dans `method_profit` des nœuds argent/pièces (`brain_alch::publish`) ;
  `resolve_money` la classe avec les autres ; `required_tool` exige une
  rune de nature et du feu (bâton accepté).
- Tirage : un stock à alcher qui vaut le temps du bot comme une offre
  (taux ≥ 2 × son taux, net ≥ 50, runes de nature en main pour ≥ 10
  lancers) devient un but `MakeMoney` d'origine `alch:<objet>`
  (ambition intacte).
- Ventes : un lanceur ne liste pas ce qui s'alche mieux que ça ne se
  vend (`alch_keep`) et ne vend jamais sous le net du lancer (plancher
  `alch_floor`) : la valeur d'alchimie devient le plancher naturel du
  marché.
- Marché (`market_pass`, toutes les 600 s) : runes de nature pour le
  stock à alcher (offre au prix courant + la moitié de la marge ; le
  marché seulement : un spot de combat les lâche, D-A), et l'ask d'un
  autre compte le plus loin sous `net − 5 ticks × taux` est acheté pour
  l'alcher.
- 2026-11-15 : les billets (`cert_*`) comptent comme leur objet dans le
  stock à alcher (`stacks_of` ; avant, un lot acheté arrivé noté lisait
  « missing » et le nœud abandonnait) ; un lanceur garde ses runes de
  nature, de feu et son bâton (`caster_means`, ventes et `cleanup`) ; il
  enchérit sur les runes de nature pour la meilleure occasion, son stock
  ou à défaut le meilleur ask à alcher (plafond = prix courant + moitié
  de la marge, `nature_ceiling`) ; les achats à alcher descendent la
  liste par profit par lancer DÉCROISSANT (`rank_by_profit`) tant que
  runes, pièces et lot suivent.
- Entraînement, classement (décision utilisateur 2026-11-15 : « la
  méthode la plus rentable », remplace le moindre coût par XP) : toute
  méthode qui gagne des pièces passe avant toute perte, la meilleure en
  pièces par tick (temps non compté) ; si toutes perdent, la plus petite
  perte en pièces par XP (`training::sort_best_first`, `rank_key`).
- Entraînement (2026-11-14, `training.rs`, `brain_train.rs`, voir
  docs/planner.md « Entraînement au prix ») : dès magie 21, chaque
  tranche compare au lancement le sort de combat et l'alchimie (stock
  propre ou asks d'autres comptes au plus à la valeur d'alchimie) en
  pièces par XP, perte comprise : l'alchimie entraîne même à petite perte
  quand elle coûte moins par XP que les sorts. Le lot (lancers que stock,
  runes et bourse couvrent) part en `alchemy <objet> <lot> --level magic
  N` ; runes de nature au marché seulement (drops F2P, aucune boutique
  F2P hors Wilderness), bâton de feu acheté quand il coûte moins que les
  runes de feu de la montée. Aucun bot n'a magie 21 aujourd'hui (max 10) :
  vérifié par simulation (magie 25 et 60, `brain_train_tests.rs`).

## Rentabilité de tout le marché (2026-11-14)

Demande utilisateur : « le brain doit être capable d'évaluer la
rentabilité en fonction de toutes les dépendances et des rates (si
existants), pour tout le marché » ; une forte demande d'un produit doit
faire produire et vendre, et tirer le prix de ses composants vers le
haut. Constat live (11:40) : seuls les bids `pot_flour` trouvaient
preneur ; bst29bh9 (forge 22, minerai de fer en banque) perdait 559 sur
3 heaumes de fer à 123 à cause du forfait de 800 ticks ; le bouclier de
fer (forge 27) portait toute la montée 22→27 (−12 431) ; personne
n'enchérissait sur les barres ni les minerais au prix que les produits
justifient.

Évaluation (`planner/src/bid_response.rs`, `market_value.rs`, câblage
`brain_bids.rs`) — à chaque tirage, pour CHAQUE objet de valeur
(offres d'autres comptes, prix courant échangé avec volume 24 h, plancher
d'alchimie du bot lanceur) qu'une jambe de production fabrique :
- Coût unitaire récursif (profondeur 4 : amulette enfilée, amulette,
  barre, minerai) : acheter (asks qui couvrent la quantité) ou fabriquer
  (niveau, bans, quête, outils, rendements appris sinon a priori), chaque
  intrant à son propre coût ; stock détenu au prix de sa vente.
- Frais fixes du LOT, jamais par réponse : site choisi comme le brain le
  ferait (`market_value::leg_site` = `combat::pick_site_gated` au niveau
  de combat du bot, sites sûrs et ouverts seulement) ; marche (Manhattan,
  1 tick/case) de chaque étape vers l'étape suivante (minerai → fourneau
  → enclume → fontaine), péage Al Kharid aller-retour (sauf Prince Ali)
  et bateau de Karamja, outils achetés une fois, trajet fontaine → site
  pour les intrants achetés (une remise : le plus long), trajet de la
  position du bot au premier site, remise à la fontaine (`HANDOVER_TICKS`
  = 100). Le coût par unité étale ces frais sur le lot (`total_for`).
- Revenu = ce que le marché absorbe (`market_value::absorbed_lot`) :
  bids au-dessus du coût, moins les asks à ≤ 125 % du meilleur bid (offre
  en route) ; plus les ventes au prix courant sur la journée (50 % du
  volume 24 h moins les asks déjà au repos à ce prix ou dessous) ; moins
  le stock libre du bot et les unités RÉCLAMÉES par d'autres producteurs
  (tableau public, ci-dessous) ; le bot en prend la moitié
  (`PRODUCER_SPLIT`, ≤ 100) ; un lanceur ajoute son plancher d'alchimie
  (net des runes et de 5 ticks de lancer au taux du bot) jusqu'à un
  inventaire (28).
- Entraînement = investissement (`training_charge`) : coût brut de la
  montée (temps au taux du bot + intrants) moins la valeur des produits
  faits en montant (prix courant ou meilleur bid × saturation), puis une
  PART seulement : rien quand l'ambition du bot monte déjà cette
  compétence (échelle du catalogue ou jalon niveau ouvert,
  `brain_bids::planned_levels`), un lot sur `TRAINING_AMORTIZE_LOTS` (4)
  sinon. Plus de plafond de niveaux (2026-11-19, décision utilisateur
  « tout le temps ») : n'importe quel écart se chiffre, le coût de
  l'entraînement décide ; la magie s'entraîne pour une offre (`TRAINABLE`
  de `bid_response`). Constat live : 51 offres d'achat d'amulettes de
  puissance (3 557 à 1 042 900), intrants à quelques pièces, personne ne
  répondait (artisanat 53 au mieux, 70 requis ; magie jamais entraînée ;
  lot de 26 pour 6 diamants bruts en vente). Le coût d'entraînement prend
  la méthode la moins chère ouverte au niveau du bot (méthode de tranche
  et toutes les recettes de `training_production::candidates`, net de
  la valeur des produits ; jamais 7 071 bagues en diamant introuvables) ;
  une méthode sans produit (combat magique) se chiffre au temps par XP
  (`sizing::tranche_ticks_with`). `bid-why` affiche la méthode retenue.
- Lot réduit aux intrants (2026-11-19, `bid_response::feasible_lot`) :
  quand les intrants en vente ou détenus ne couvrent pas toute la
  demande, le lot descend au plus grand nombre qu'ils couvrent (bissection,
  la faisabilité ne fait que baisser avec la quantité), jamais à rien ;
  la demande reste lue entière (`market_value::absorbed_lot_within`).
- Règle de choix inchangée : taux ≥ 2 × taux du bot et net ≥ 50 ;
  meilleur taux incliné ±15 % ; étalement une fenêtre sur deux. Log :
  `brain <compte>: answers the market: produce N <objet> (k to bids, best
  …; k at the current price; k alched; revenue …, unit …, lot … ticks,
  … coins/tick vs own …, net …, train …; plan)`.
- CPU : un choix par compte et par fenêtre d'étalement mis en cache 60 s
  (`CHOICE_TTL_SECS` ; un tirage réessayé à chaque passe le réutilise) ;
  prix courants et volumes lus à la demande et mémorisés par lecteur ;
  objets sans jambe de production écartés avant toute lecture du store ;
  volume lu seulement pour un objet avec bids, prix échangé ou alchimie.

Taux du bot (2026-11-17) : son D17 dans le guide money précalculé
(`brain_bids::bot_rate`, `brain_d17`), plus la table `rates_profit`
« réalisable » (`rate_rows`/`realizable_rows`, supprimées) : le guide
valorise déjà ce que le marché absorbe (D12) ; le cas live de 2026-11-14
(`smith` tranche 1 à 65 929 / 6000 sur quelques fills de cimeterres,
11 pièces/tick pour tous) ne peut plus se produire, chaque ligne étant
cotée au volume de ses fills. `orchestrator bid-why` affiche la ligne D17
du bot (famille, méthode, tranche, profit).

Tableau des commandes (`market_claims.rs`) : un bot qui prend un lot
publie « N unités de X réclamées » ; tout bot lit le total réclamé par
les autres (jamais qui, jamais autre chose) et le retire de ce que le
marché absorbe. Public comme le tableau des commandes d'artisanat d'un
vrai jeu : D22 tient (voir `decisions.md`). En mémoire, expire après 2 ×
la durée estimée du lot (10 min à 6 h), libéré au tirage suivant du bot.

Demande dérivée (`planner/src/derived_demand.rs`,
`brain_market::derived_reserve`) : quand un nœud d'obtention nourrit un
produit du graphe (barres d'un heaume, minerai d'une barre), son prix de
réserve = (valeur du produit × 0,8 − temps de l'étape au taux du bot −
autres intrants au meilleur ask / prix courant) / unités par produit,
récursif (le minerai vaut la valeur DÉRIVÉE de la barre). Valeur du
produit = meilleur bid d'un autre compte, sinon prix courant. L'offre
permanente posée à côté de la production (D-B) a pour plafond
`min(portefeuille, réserve)` et ouvre à au moins la moitié de la réserve
(`derived_bounds`) : une barre à 2 pièces que personne ne remplit ne
fait bouger aucun mineur, une barre à 23 si. Fondeurs et mineurs y
répondent par le même moteur (les barres et minerais ont des jambes de
production). Une réserve ≤ 0 (produit qui vaut moins que ses intrants)
plafonne l'offre à 1. Log `standing bid … (ceiling C, derived reserve R)`.
Toutes les recettes F2P passent par ces chemins (forge, fonte, cuisine,
artisanat, poterie, cuir, bijoux, filage, runes, récoltes).

Ambitions (`ambition_catalog::kind_weights`, `serves`) : une offre
compte pour un type quand son échelle liste l'objet OU que sa
compétence primaire le fabrique à n'importe quel palier (adamant hors de
portée de toute la flotte → forgerons), et pour moitié par étape de
recette quand sa compétence en fabrique un intrant (minerai, charbon →
mineurs) ; une marge connue négative ne tire personne. L'échelle du
forgeron va jusqu'à l'adamant (minage 70, forge 78 dague/heaume/
cimeterre, forge 88 plastron/jambières).

Vendeur : un ask ancré sur un fill isolé descend (test
`market/src/outlier_tests.rs`, cas du cimeterre de bronze) : le prix
courant est borné par l'ask d'impact (1 unité → 1149, pas 2747), un
nouveau lot ouvre là, et les asks au repos marchent de 5 % par fenêtre
jusqu'à leur réserve (≤ 140 en 60 fenêtres). Bande ×5 inchangée.

## Butin : coût nul, lots entiers, prix qui écoule (2026-11-14)

Constat live (15:40, frank_lane, mêlée aux guerriers d'Al Kharid) :
banque 52/60 types dont 88 heaumes moyens de bronze, 41 dagues de fer,
838 flèches de bronze ; une seule annonce de dague (1 @628 pour ~30),
aucune pour les heaumes ni les flèches ; bâton 2 @1644, flèche d'acier
1 @729, rune de loi 2 @330 ; « holding 1 bronze_dagger (no own cost
known) ». Causes vérifiées :
- la réserve partageait les ticks du combat entre les drops au prorata
  de leur valeur : une flèche d'acier « coûtait » 3 541, une flèche de
  bronze 12 (hors bande 5× d'un prix de 2 → gardée), un drop absent des
  200 derniers rapports n'avait aucun coût → gardé ;
- le heaume PORTÉ protégeait par son nom toute la pile de la banque
  (idem pour la meilleure pièce gardée par `kept_gear`) ;
- une annonce de 1 unité excluait l'objet des passes suivantes : la pile
  grossissait derrière un éclat jamais vendu ;
- une annonce réadoptée gardait son prix ancré sur l'ancienne réserve et
  ne descendait que de 5 % par fenêtre.

Règles :
- Coût propre (`brain_reserve`) : seules les unités `produced` portent
  des ticks (récolte, fabrication) ; le butin, les drops et les
  apparitions au sol sont TROUVÉS : le combat se paie en XP ou en
  pièces, ils ne coûtent rien (`own_origin`, réserve `Found` à 1). Un
  objet à la fois fabriqué et trouvé garde le coût de sa part fabriquée
  (ticks et intrants × part fabriquée). Sans trace propre : estimation
  du plan ou a priori d'une récolte (pêche, bûcheronnage, minage ; plus
  jamais l'a priori de combat), sinon coût de fabrication `bid_response`
  si le bot sait le faire, sinon trouvé. Un achat garde son prix payé
  comme plancher. Un intrant trouvé ne coûte rien non plus
  (`unit_cost`).
- Ouverture d'un objet trouvé (`lifescape_market::found_open`) : objet
  échangé → prix courant (médiane des 5 derniers fills bornée par
  l'impact) ou meilleur bid plus riche ; jamais échangé → juste sous
  l'ask le moins cher (ou dans un bid plus riche), carnet vide → valeur
  de haute alchimie (donnée publique, le prix attendu de `gear_plan`),
  sinon 2 × plancher ; jamais au-dessus de la bande 5× d'un objet
  échangé ; mémoire d'un fill rapide inchangée. Plancher 1, ou le net
  d'alchimie d'un lanceur (`alch_floor`) ; les lanceurs achètent sous ce
  net (`brain_alch`), c'est le plancher naturel du marché. La patience
  (5 % par fenêtre de 2000 ticks) descend l'ask jusqu'au plancher.
- Lots entiers : `sale_lot` inchangé (bids + volume 24 h, au moins un
  inventaire, jamais plus que la pile). Une annonce intacte qui n'offre
  qu'un éclat d'une grosse pile (≤ moitié du lot de la pile entière,
  `market_seller::undersized`) est retirée et la pile part en un lot
  dans la même passe (`brain_sales::regroup_slivers` ; objets non
  gardés, lignes `open` seulement, jamais la tranche d'un teneur de
  marché).
- Réadoption (après redémarrage) : un ask au-dessus de l'ouverture du
  jour descend à elle (`Adoption::Lower`), jamais sous la réserve.
- Équipement porté ou gardé : UNE copie de rechange reste (une mort fait
  perdre le porté), le reste est du surplus
  (`gear_plan::spare_gear_units`) ; une pièce que le plan garde entière
  (obtention, quête) n'est pas touchée.
- Gardé (inchangé) : nourriture jusqu'à 100 unités, outils de travail,
  intrants des méthodes ouvertes, gear gardé, objets d'ambition et de
  quête ; un plan à distance garde ses munitions (`fight:ranged/` →
  flèches de la tranche), un combattant de mêlée vend les siennes.

Vérification hors ligne (`loot_check_tests.rs`, ignoré, mêmes variables
que `live_check` + `LIFESCAPE_CHECK_ACCOUNT` optionnel) : sur une copie
de la base et un `/api/status`, les annonces regroupées, abaissées et
les lots que chaque bot posterait.

## Entraînement en achetant les composants (2026-11-15)

Cooking, crafting, firemaking et prayer rejoignent la forge et la magie
dans l'entraînement au prix (`planner/src/training_production.rs`, voir
`docs/planner.md` § Magie, ranged, demande servie) : chaque recette
ouverte est chiffrée à ce que coûtent ses intrants au bot (stock détenu
à sa valeur de vente, sinon achat au marché ou fabrication, temps du bot
compris à son taux de gain) moins ce que le marché paie le produit. La
plus rentable gagne, sinon la moins chère par XP : un bot qui gagne
mieux en farmant de l'argent achète les composants bon marché au lieu de
les récolter.


## Registre de pièces et mesure de l'économie (2026-11-15)

Le classement des pièces par écart de stock avant/après (heuristique de
`session_report`) attribuait 1,35 M de « ventes en boutique » en 24 h alors
qu'aucun bot ne vend aux PNJ. Le SDK tient désormais un registre
(`sdk/src/client/coin_ledger.rs`) : chaque variation des pièces du sac est
un flux, attribué au contexte que l'action a nommé pour quelques ticks
(`Bot::coin_context`) : `shop` (boutons de la grille ou du côté sac d'une
boutique), `trade` (écrans d'échange), `fee` (péage, passeur, tanneur),
`loot` (objet au sol ramassé), `alch` (sort sur un objet porté),
`dialogue` (achat par dialogue). Une variation banque ouverte est un
transfert, jamais un flux ; ce qu'aucun contexte n'explique reste
`unattributed`. Le rapport de tâche lit ces flux (provenances `shop_sold`,
`shop_bought`, `traded`, `fee` (nouvelle), `loot`, `produced`, reste
`unattributed`/`lost`).

`orchestrator economy [heures]` : masse monétaire (instantané
`holdings_hourly`, staff et exclusions hors champ) et sa variation par jour,
flux de pièces de la fenêtre par provenance (`traded` = transfert entre
joueurs), indice de prix de Laspeyres de la fenêtre contre la précédente
sur les exécutions réelles, nombre d'objets échangés à 1 pièce.

## Remises : paiement Offer-X, banque pleine, péage (2026-11-15)

Diagnostic sur 24 h (3 818 `timeout`, 1 027 `short`) :
- 64 % des timeouts : l'acheteur payait en Offer-10, un clic par aller-
  retour serveur, avec une pile de pièces plus grosse que le dû (le retrait
  prend toute la pile) ; au-delà d'environ 2 100 pièces, les 300 ticks de
  l'offre ne suffisaient jamais (aucun deal réglé au-dessus de 2 261
  pièces). `trade_offer` paie désormais une partie de pile en un seul
  Offer-X (`[inv_button5,tradeside:inv]` puis `p_countdialog`, réponse
  `RESUME_P_COUNTDIALOG` au montant exact, séquence de Client-TS) ; une
  pile exacte reste Offer-All. Les objets passent toujours en billets quand
  ils en ont (`withdraw_lots`) : un gros volume = une case, un clic.
- 96 % des shorts : banque F2P pleine (60 types). Le nettoyage D17
  annulait bien les annonces mais ne pouvait rien retirer avec un sac plein
  des objets refusés au dépôt (« still locked » trompeur) : il jette
  maintenant le moins cher non protégé du sac avant chaque retrait.
- 6 % : derrière le péage d'Al Kharid, la remise déposait toutes les pièces
  puis ne pouvait plus passer la porte vers Varrock ouest : le péage reste
  dans le sac (retiré au besoin), chemins payants ouverts pour ce trajet.
Restent à traiter : étiqueter `short`/`no window` de l'échange autrement
que `timeout` (sans les pénalités du `short` vendeur), le refus « I can't
reach that! » à la banque la plus proche (7 %), l'arrêt d'un run pendant
la remise (2 %).

`orchestrator bid-why <compte> <objet>` (daemon : carnet en mémoire) rejoue
pour un bot la réponse aux offres d'achat étape par étape
(`bid_response::explain`) : tour de l'objet (répartition), recette, demande
(meilleure offre, profondeur, part des ventes, alchimie), coût unitaire et
plan, lot absorbé, charge d'entraînement, taux contre le taux propre (sans
marge) et profit contre le minimum. Une offre ignorée dit pourquoi.
L'objet se nomme par nom de debug, billet, nom affiché ou id
(`bid_why::resolve_item`) ; la première ligne dit lequel est lu
(`stringstar = "Unblessed symbol" (id 1716, banknote 1717)`) et, sans offre
sur lui, les offres posées sur ce qu'on en fait ou sur un homonyme (C13,
2026-11-16 : les dix offres « Holy symbol » sont sur `blessedstar` 1718,
béni par Brother Jered à partir de `stringstar` ; aucun id mal mappé).

`orchestrator rares` (daemon, lecture seule, info opérateur : aucun bot ne
le lit, Q8 : les offres restent) classe chaque objet à offres sans aucun
ask (staff exclu) par `planner::rare_items` : productible maintenant (une
jambe du planner que les niveaux et quêtes de la flotte font, chaque
intrant compris), boutique F2P (porte et nombre de bots qui la passent :
Champions' Guild 32 points de quête), hors de portée (niveau, quête ou
intrant manquant, l'écart le plus petit affiché), introuvable (aucune jambe
ni marchand ; voies serveur sans jambe citées, ex. bénédiction de Jered).

## Taux propre d'un bot : profit par run (2026-11-15)

`bid-why` a montré un taux propre de 70 pièces par tick (420 000 de
l'heure) : la ligne de profit d'une méthode additionnait le rendement moyen
de CHAQUE objet jamais vu dans son seau (cuisine niveau 16 : crevettes,
viande, truite, sardines, anchois, une barre d'acier mal attribuée), comme
si un run les produisait tous : 422 083 pièces par 6 000 ticks, et aucun
lot ne battait jamais ce taux. `orchestrator/src/rates_runs.rs` garde les
flux de chaque run (produits positifs, intrants négatifs, par 6 000 ticks,
50 derniers par méthode et palier, persistés avec l'instantané des taux et
reconstruits depuis les rapports en base au démarrage) ; le profit est la
valeur nette du run médian aux prix du jour. Taux propres après correction :
4 à 6,5 pièces par tick. Une récolte sans site F2P n'est plus une jambe
(`obtain_legs` : pas d'érable F2P).

## Registre d'objets : d'où vient et où part chaque objet (2026-11-15)

Le registre de pièces s'étend à tout objet porté
(`sdk/src/client/item_ledger.rs`) : chaque variation du sac est un flux
attribué au contexte de l'action (boutique, échange, frais, ramassage,
alchimie, achat par dialogue, objet jeté par `Bot::drop`), sinon à la
compétence quand l'XP a bougé dans les 8 ticks (produit ou consommé par le
travail), sinon à une mort ; banque ouverte = transfert. Le rapport de
tâche prend ces provenances (`produced`, `consumed`, `shop_bought`,
`shop_sold`, `traded`, `loot`/`ground_spawn`, `lost`, `death`) ; seul un
reste que le registre n'explique pas garde la règle par différence
(2026-11-16 : remplacé, le diff net n'est plus qu'un contrôle, § Rendements).
`orchestrator economy [heures] item <nom>` liste les entrées (+) et sorties
(−) d'un objet par provenance ; sans nom, les principaux flux du monde.
