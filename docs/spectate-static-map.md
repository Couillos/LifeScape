# Spectate : map statique précalculée (remplace `/minimap.png`)

Doc seule, aucun code. Objectif : supprimer le stream de map rendu à la demande,
qui tue le front dès que le dashboard est ouvert (verrou global + re-parse par
requête, `rs/crates/orchestrator/src/minimap.rs:589`, cache 16 frames
`minimap.rs:420` ; 81 s pour 4 frames froides concurrentes, ~15 ms sérialisé,
~60 img/s tiède — `minimap.rs:428-430`). Chaque nouvelle origine de tuile
déclenche un fetch (`orchestration/static/spectator.js:267-277`), le redraw est
à 1/s par défaut (`spectator.js:28`) ou chaque frame en RT, poussé par la WS à
5 Hz (`rs/crates/orchestrator/src/web.rs:24`, `WS_INTERVAL`). Un bot qui marche
= une origine par pas = jusqu'à ~5 renders/s par client derrière le même
verrou, et le cache client (48 entrées, `spectator.js:273`) ne couvre que les
allers-retours.

Décision produit : précalculer la map entière **une fois**, la servir en
fichiers statiques immuables, le client ne fait que crop autour du joueur
(position déjà reçue par WS à 5 Hz). Zéro requête image par seconde.

Contrainte : **parité visuelle exacte** avec le rendu actuel — mêmes pixels à
l'écran que `/minimap.png?size=33` aujourd'hui. Le rendu actuel, c'est
**4 px par tuile**, murs et sprites mapscene inclus (preuve §3). Tous les
chiffres ci-dessous sont mesurés à cette parité depuis le pack
(`/home/josh/dev/lostcity/engine/data/pack/.cache/maps-server.zip`,
14,5 Mo, 483 carrés `m` + 483 `l`), pas d'intuition. Méthode reproductible
au §8.

## 1. Périmètre : ce qu'est « la map entière »

Bbox des carrés : x 1856–3647, z 1280–10367, soit 1792 × 9088 tuiles, mais
3493 des 3976 carrés de la bbox sont vides (océan). Les carrés peints forment
5 grappes (par carré-z) :

| grappe (carré-z) | tuiles (x × z) | L0 | L1 | L2 | L3 |
|---|---|---|---|---|---|
| z20 (îlot isolé, 1 carré) | 64 × 64 | 4096 | 0 | 0 | 0 |
| z44–62 (continent) | 1600 × 1216 | 1 134 505 | 35 425 | 8 327 | 952 |
| z70–77 | 1216 × 512 | 187 840 | 18 724 | 1 937 | 1 603 |
| z144–158 | 1216 × 960 | 371 974 | 53 425 | 14 716 | 4 118 |
| z160–161 | 128 × 128 | 7 643 | 238 | 221 | 0 |

Totaux peintes (underlay **ou** overlay) : L0 1 706 058, L1 107 812,
L2 25 201, L3 6 673. Note : L3 = 100 % overlay, 0 underlay (étages sans sol
peint) — tout compteur basé sur le seul underlay rate L3.

Pas de crop « F2P » : l'appartenance F2P n'existe pas dans le pack, il
faudrait une liste de tuiles maintenue à la main qui casserait le jour où un
bot en sort. La découpe par grappes exclut déjà le vide inter-grappes, ce qui
était l'essentiel du surcoût (88 % des carrés de la bbox sont vides).

## 2. Palette : l'indexé 8 bits est impossible, PNG32 obligatoire

Chaîne couleur (mêmes passes que le client) : sols et blends passent par
`colour_table` (`mapcache.rs:412`), overlays par `flo_overlay_colour`
(`mapcache.rs:608`), murs 2 couleurs (`minimap.rs:20-21`), sprites mapscene
sur palette partagée.

Comptage exact (réplique IEEE754 de la chaîne, flo côté **client**
`pack/client/config` → `flo.dat`, 103 entrées dont 87 colorées — le dump
serveur `pack/server/flo.dat` ne contient que des zéros) :

- underlays utilisés : 35 ids ; overlays utilisés : 57 ids → **47 couleurs**
  (dont 10 moyennes de textures : ids 1, 2, 3, 11, 15, 23, 25, 31, 35, 46) ;
- blends moyennés 10×10 (`minimap.rs:232-273`) : chaque moyenne produit une
  entrée couleur quasi-distincte → **4 690 couleurs distinctes** au total
  (sol + overlay + murs), dont ~4 000 pour le seul L0 du continent
  (3 966 rendues, murs et sprites inclus) ;
- sprites : 3 861 locs placés → 37 sprites mapscene utilisés, palette
  partagée de 32 entrées (`mapcache.rs:792-803`, index 0 = transparent),
  **31 index réellement référencés**.

4 690 > 256 : pas d'image indexée sans quantification destructive.
Recommandation : PNG32 truecolor tel quel — le poids reste faible (§4), la
quantification est un travail pour rien.

## 3. Résolution : le rendu actuel est 4 px/tuile, upscalé 4× — parité imposée

Preuves code, de bout en bout :

- servi : `border = (size * 4)` (`minimap.rs:598`), doc « 4 px per tile »
  (`minimap.rs:581`) ; `size` vaut 33 par défaut (`web.rs:493`) → image
  **132 × 132 px** ;
- demandé : `size = R * 2 + 1 = 33` (`spectator.js:277`, `R = 16`,
  `spectator.js:38`) ;
- affiché : `mapSize = 33 * TILE = 528` px (`spectator.js:259-261`,
  `TILE = 16`, `spectator.js:37`), `drawImage(mapImg, ..., 528, 528)`
  (`spectator.js:306`) avec `imageSmoothingEnabled = false`
  (`spectator.js:305`) → **upscale 4× net**, pas de lissage ;
- contenu : le PNG inclut murs (`draw_wall`, `minimap.rs:611-626`) et
  sprites mapscene tracés côté serveur (`plot_sprite`, `minimap.rs:542-579`,
  palette partagée 32 entrées) — le client ne redessine ni murs ni bâtiments.

Donc parité = **4 px/tuile, murs + sprites inclus**, crop 132 × 132 px affiché
528 × 528. Pas de 1 px/tuile : l'utilisateur n'a jamais demandé à changer le
visuel, et à 4× d'upscale la différence 1 px → 4 px se voit (murs 1 px,
sprites pixel-art). Le `image-rendering: pixelated` du CSS (`index.html:48`)
reste tel quel.

## 4. Poids mesurés à parité : ~4,4 Mo au total, une image par grappe et par niveau

Rendu exact à parité (sols + overlays avec masques de forme, murs 1 px,
37 sprites mapscene tracés comme `plot_sprite`, niveaux supérieurs
VIS_BELOW) encodé PNG32 compression zlib standard, filtre 0 :

| fichier (4 px/tuile) | dimensions px | poids |
|---|---|---|
| continent L0 | 6400 × 4864 | 2 594 Ko |
| z144–158 L0 | 4864 × 3840 | 616 Ko |
| z70–77 L0 | 4864 × 2048 | 259 Ko |
| continent L1 / z144 L1 | — | 220 / 230 Ko |
| autres niveaux (L1–L3, îlots) | — | 1–124 Ko chacun |
| **total 16 fichiers** | — | **~4 477 Ko (4,37 Mo)** |

Référence 1 px/tuile (mêmes contenus, sans murs/sprites) : 690 Ko — donnée
pour mémoire seulement, le visuel n'est pas le même.

Découpage tranché par les chiffres :

- **une image par niveau monde entier : rejeté.** Bbox 1792 × 9088 tuiles à
  4 px = 7 168 × 36 352 px = 260 Mpx à décoder (~1 Go RAM navigateur).
- **tuiles 256 px : rejeté.** ~200 fichiers, ~200 requêtes au premier
  chargement, logique de recollement côté client, pour un total qui resterait
  ~4,4 Mo (le PNG compresse déjà le vide à rien).
- **retenu : 16 images (5 grappes × niveaux non vides), 4 px/tuile, PNG32**,
  servies immuables (nom avec hash du pack, `Cache-Control: public,
  max-age=31536000, immutable`, sur le modèle des sprites
  `web.rs:507-514` qui ont déjà `max-age=86400`). Premier chargement
  ~4,4 Mo, ensuite 0 octet (cache navigateur).

4,37 Mo < 5 Mo : le seuil d'explosion n'est pas atteint, **aucune
alternative dégradée n'est proposée** — la parité visuelle passe telle quelle.

Point d'implémentation bloquant : le writer PNG actuel ne compresse pas
(blocs deflate *stored*, `png.rs:51-72`). À parité non compressée, les 16
fichiers feraient ~720 Mo (15 M tuiles × 16 px × 3 o). Le lot 1 doit donc
inclure un writer deflate qui compresse vraiment (au minimum Huffman fixe,
std only) : les 4,37 Mo ci-dessus supposent une compression zlib standard
(niveau 6, filtre 0 — même layout que `encode_rgb`, `png.rs:100-109`).

## 5. Précalcul : au démarrage du daemon, SKIP si à jour, jamais écrasé en aveugle

- **Quand : démarrage du daemon** (pas au `cargo build`, pas au premier
  accès). Au build, le pack n'est pas versionné avec le binaire → stale
  silencieux ; au premier accès, on réintroduit de la latence sur le chemin
  utilisateur. Au démarrage : le pack est résolu au même endroit que le
  renderer actuel (`pack_dir`, `mapcache.rs:15-26`, `LIFESCAPE_ENGINE_DIR`
  sinon `../lostcity`), on compare le hash du zip à un fichier tampon et on
  régénère si différent.
- **SKIP : si le tampon égale le hash du zip actuel, on ne touche à rien**
  (pas de re-parse, pas de réécriture — démarrage instantané).
- **Où : `orchestration/static/map-<hash>/`** (dossier cache dédié, à côté
  des assets servis par `asset()`, `web.rs:410-412`), ex.
  `map-<hash>/c44-l0.png`, plus `map-<hash>.stamp`. Génération dans
  `map-<hash>.tmp/` puis rename atomique ; **jamais d'écrasement en aveugle**
  d'un dossier en cours de service. Après bascule, suppression des vieux
  `map-*` (garder l'avant-dernier le temps d'un redémarrage, pas plus).
- **Coût : une fois par changement de pack.** Référence : l'index loc monde
  entier prend ~1 s (`minimap.rs:654`, commentaire de test) ; le précalcul
  complet (parse des 483 carrés ~3 s mesurés + blends + encodage PNG) est en
  dizaines de secondes au pire, à mesurer et noter au premier run. Coût
  actuel par requête : ~15 ms tiède derrière le verrou global, catastrophe
  en froid (81 s / 4 concurrents) — supprimé dans tous les cas, la route
  `/minimap.png` (`web.rs:488-500`, servie `no-store`, `web.rs:497`) est
  **supprimée**, pas mise en cache.

## 6. Client : crop local, flux restant, fallback

- Crop : au `draw()`, remplacer le fetch `minimap.png?...` (`spectator.js:277`)
  par `drawImage(img, sx, sy, 132, 132, dx, dy, 528, 528)` depuis l'image
  statique du niveau courant : `sx = (originX - X0) * 4`, `sy` idem
  (origines en tuiles comme `originX/originZ`, `spectator.js:256-257`, × 4
  px/tuile). La position vient déjà de la WS à 5 Hz, aucun nouveau flux. Le
  décalage de secours (`spectator.js:282-297`) et
  `mapCache`/`mapPending` disparaissent avec les fetches.
- Zoom/pan : il n'y en a pas aujourd'hui (R fixe) → on n'en ajoute pas.
- Fallback : si l'image du niveau est absente (404) ou vide, **message texte
  dans le canvas** (« static map missing, run daemon with pack »), jamais
  d'icône image cassée, jamais de fond noir avec overlays orphelins (le
  `return` garde-fou actuel `spectator.js:290-294` devient inutile).
- Poids restant par seconde : uniquement la WS JSON existante (état compacté
  `loc_fields`, `web.rs:116-148`, signature anti-renvoi `web.rs:92-113`),
  inchangée. **0 requête image**, contre jusqu'à ~5/s aujourd'hui pour un bot
  qui marche.

## 7. Lots ordonnés (chacun testable + revertable)

1. **Générateur + 16 PNG.** Offline (mêmes passes que `minimap.rs:286-336`
   + `draw_details`, halo d'une tuile + marge sprites aux bords de grappes),
   écrit dans `orchestration/static/map-<hash>.tmp/` puis rename.
   Inclut le writer deflate compressant (le writer actuel `png.rs:51-72`
   fait du stored : ~720 Mo sinon). Test : dimensions et poids par fichier
   conformes au §4. Revert : supprimer le dossier. Mesurable : temps de
   génération noté, total ≤ plafond §8.
2. **Routeur statique + suppression `/minimap.png`.** Sert les PNG avec
   cache immuable ; la route `web.rs:488-500` et `shared_maps().png`
   disparaissent du chemin requête (le renderer reste pour le debug hors
   ligne, pas pour servir). Test : 2 min de dashboard ouvert → 0 requête
   `/minimap.png` (logs d'accès), page complète ≤ plafond. Revert : revert du
   commit (un seul).
3. **Client crop.** `spectator.js` : `drawImage` depuis l'image du niveau,
   suppression `mapCache`/`mapPending`/fetch, message fallback.
   Test : suivre un bot qui marche 2 min, 0 fetch image (onglet réseau),
   la carte reste alignée sur le joueur. Revert : revert du fichier.
4. **Invalidation.** Hash du zip dans le nom + tampon ; si le pack change,
   régénération au redémarrage, l'ancien dossier reste servi tant que le
   nouveau n'est pas complet (pas de 404 transitoire).

Non-objectifs (on ne le fait pas) : re-render serveur à la demande hors
debug, WebSocket binaire, tuilage 256 px, quantification de palette, zoom/pan,
crop F2P, changer le format WS ou la forme compacte des locs.

## 8. Critères d'acceptation (plafonds chiffrés)

- **Poids total servi : ≤ 5 Mo** (mesuré : 4,37 Mo à parité ; marge pour
  l'évolution du pack et l'écart Huffman fixe vs zlib dynamique).
  Méthode : somme des `Content-Length` des 16 PNG (ou
  `ls -l orchestration/static/map-*/`). Seuil d'explosion non atteint :
  aucune alternative dégradée requise.
- **0 requête `/minimap.png` en 2 min de dashboard ouvert** avec un bot qui
  marche (la route n'existe plus → 404 si un vieux client la demande).
- **Premier chargement carte : ≤ 5 Mo réseau**, puis 0 octet (304/immutable).
- **Génération : temps noté au premier run**, une fois par changement de
  pack (pas de plafond strict, ordre de grandeur attendu : dizaines de
  secondes) ; **redémarrage sans changement de pack : SKIP** (0 s, tampon
  hash égal, aucun fichier touché).

Reproduction des chiffres : pack
`/home/josh/dev/lostcity/engine/data/pack/` (zip `.cache/maps-server.zip`,
config `client/config`, textures `client/textures`, sprites
`client/media`) ; opcode sols `minimap.rs:125-159`, blend
`minimap.rs:232-273`, chaîne couleur `mapcache.rs:373-459`,
overlays `mapcache.rs:606-631`, sprites `mapcache.rs:806-839` ;
flo serveur vide (`server/flo.dat` = 103 zéros) → couleurs prises du jag
client. Approximation : réplique flottante Python des `f64` Rust (±1 ulp
possible sur les moyennes gamma — comptes de couleurs à quelques unités
près, poids à ~1 % près).

## 9. Risques

- **Pack qui change** : invalidation par hash du zip dans le nom de dossier
  (§5, lot 4). Un pack sans les carrés attendus → grappe absente → fallback
  texte, pas de carte partielle silencieuse.
- **Mémoire navigateur** : plus grosse image 6400 × 4864 px ≈ 124 Mo
  décodée (31 Mpx, continent L0 à 4 px/tuile) ; total décodé ~140 Mo si tous
  les niveaux du continent sont chargés. OK sur desktop (outil dev), à
  revalider si le spectate part un jour sur mobile : le découpage par
  grappes/niveaux permet déjà de ne charger que le niveau suivi.
- **Vide = noir.** Les tuiles non peintes valent 0 (`minimap.rs:278`,
  `render_ground` ne peint rien quand la couleur est 0) : fond noir identique
  au client actuel, pas de transparence (le canvas peint déjà `#111`,
  `spectator.js:299`).
- **Niveaux supérieurs clairsemés** (L3 = 952 tuiles sur le continent) :
  images minuscules en poids (2–6 Ko), on garde le même pipeline, pas de cas
  spécial.
