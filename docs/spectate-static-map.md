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

Fil rouge : poids faible. Tous les chiffres ci-dessous sont mesurés depuis le
pack (`/home/josh/dev/lostcity/engine/data/pack/.cache/maps-server.zip`,
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
  (sol + overlay + murs), dont 4 086 pour le seul L0 du continent ;
- sprites : 3 861 locs placés → 37 sprites mapscene utilisés, palette
  partagée de 32 entrées (`mapcache.rs:792-803`, index 0 = transparent),
  **31 index réellement référencés**.

4 690 > 256 : pas d'image indexée sans quantification destructive.
Recommandation : PNG32 truecolor tel quel — le poids reste faible (§4), la
quantification est un travail pour rien.

## 3. Résolution : 1 px = 1 tuile suffit

Affichage actuel : `TILE = 16`, `R = 16` (`spectator.js:37-38`), canvas 560 px
(`index.html:94`) → 33 tuiles sur 560 px ≈ **17 px écran par tuile**.
L'image serveur actuelle fait 4 px/tuile (132 px, `minimap.rs:597`) déjà
upscalée ~4× par le navigateur ; le CSS impose déjà
`image-rendering: pixelated` (`index.html:48`).

À 17× d'upscale pixelisé, 4 px/tuile n'apporte rien de visible sur des aplats
(aucun texte dans la map, les labels sont dessinés par-dessus par le canvas).
Palier : **1 px/tuile partout**, pas d'intermédiaire (2 px/tuile doublerait
le poids pour un gain nul à cet upscale).

## 4. Poids mesurés : ~784 Ko au total, une image par grappe et par niveau

Rendu au sol exact (blend + overlay, murs et sprites exclus — quelques Ko,
même jeu de couleurs) encodé PNG32 :

| fichier (1 px/tuile) | dimensions | poids |
|---|---|---|
| continent L0 | 1600 × 1216 | 515 Ko |
| z144–158 L0 | 1216 × 960 | 112 Ko |
| z70–77 L0 | 1216 × 512 | 53 Ko |
| continent L1 / z144 L1 | — | 23 / 29 Ko |
| autres niveaux (L1–L3, îlots) | — | ≤ 10 Ko chacun |
| **total 16 fichiers** | — | **~784 Ko** |

À 4 px/tuile, le seul L0 pèse 1 734 Ko (continent 1 229 Ko) pour un rendu
identique à l'écran → rejeté.

Découpage tranché par les chiffres :

- **une image par niveau monde entier : rejeté.** Bbox 1792 × 9088 px =
  16 Mpx à décoder (~64 Mo RAM navigateur) pour 88 % de vide.
- **tuiles 256 px : rejeté.** ~200 fichiers, ~200 requêtes au premier
  chargement, logique de cache et de recollement côté client pour économiser
  ~0 Ko (le PNG32 compresse déjà le vide à rien).
- **retenu : 16 images (5 grappes × niveaux non vides), 1 px/tuile, PNG32**,
  servies immuables (nom avec hash du pack, `Cache-Control: public,
  max-age=31536000, immutable`, sur le modèle des sprites
  `web.rs:507-514` qui ont déjà `max-age=86400`). Premier chargement ~784 Ko,
  ensuite 0 octet (cache navigateur).

## 5. Précalcul : au démarrage du daemon, jamais à la demande

- **Quand : démarrage du daemon** (pas au `cargo build`, pas au premier
  accès). Au build, le pack n'est pas versionné avec le binaire → stale
  silencieux ; au premier accès, on réintroduit de la latence sur le chemin
  utilisateur. Au démarrage : le pack est résolu au même endroit que le
  renderer actuel (`pack_dir`, `mapcache.rs:15-26`, `LIFESCAPE_ENGINE_DIR`
  sinon `../lostcity`), on compare le hash du zip à un fichier tampon et on
  régénère si différent.
- **Où : `orchestration/static/map/`** (à côté des assets servis par
  `asset()`, `web.rs:410-412`), ex. `map-<hash>/c44-l0.png`. Fichiers
  servis tels quels, jamais régénérés hors changement de pack.
- **Coût : une fois.** Référence : l'index loc monde entier prend ~1 s
  (`minimap.rs:654`, commentaire de test) ; le précalcul complet (parse des
  483 carrés ~3 s mesurés + blends + encodage PNG) est en dizaines de secondes
  au pire, à mesurer et noter au premier run. Coût actuel par requête : ~15 ms
  tiède derrière le verrou global, catastrophe en froid (81 s / 4 concurrents)
  — supprimé dans tous les cas, la route `/minimap.png` (`web.rs:488-500`,
  servie `no-store`, `web.rs:497`) est **supprimée**, pas mise en cache.

## 6. Client : crop local, flux restant, fallback

- Crop : au `draw()`, remplacer le fetch `minimap.png?...` (`spectator.js:277`)
  par `drawImage(img, sx, sy, 33, 33, ...)` depuis l'image statique du niveau
  courant (origine = `pos - R`, mêmes maths que `originX/originZ`,
  `spectator.js:256-257`). La position vient déjà de la WS à 5 Hz, aucun
  nouveau flux. Le décalage de secours (`spectator.js:282-297`) et
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

1. **Générateur + 16 PNG.** Offline (même passes que `minimap.rs:286-336`
   + `draw_details`), écrit dans `orchestration/static/map-<hash>/`.
   Test : dimensions et poids par fichier conformes au §4. Revert : supprimer
   le dossier. Mesurable : temps de génération noté, total ≤ plafond §8.
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

- **Poids total servi : ≤ 1 Mo** (mesuré : ~784 Ko ; marge pour les sprites
  dessinés par-dessus et l'évolution du pack). Méthode : somme des
  `Content-Length` des 16 PNG (ou `ls -l orchestration/static/map-*/`).
- **0 requête `/minimap.png` en 2 min de dashboard ouvert** avec un bot qui
  marche (la route n'existe plus → 404 si un vieux client la demande).
- **Premier chargement carte : ≤ 1 Mo réseau**, puis 0 octet (304/immutable).
- **Génération : temps noté au premier run**, une fois par changement de
  pack (pas de plafond strict, ordre de grandeur attendu : dizaines de
  secondes).

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
- **Mémoire navigateur** : plus grosse image 1600 × 1216 px ≈ 7,8 Mo
  décodée ; total décodé ~8 Mo. Négligeable.
- **Vide = noir.** Les tuiles non peintes valent 0 (`minimap.rs:278`,
  `render_ground` ne peint rien quand la couleur est 0) : fond noir identique
  au client actuel, pas de transparence (le canvas peint déjà `#111`,
  `spectator.js:299`).
- **Niveaux supérieurs clairsemés** (L3 = 952 tuiles sur le continent) :
  images minuscules en poids (2–6 Ko), on garde le même pipeline, pas de cas
  spécial.
