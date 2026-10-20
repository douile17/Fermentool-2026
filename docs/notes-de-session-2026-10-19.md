# Notes de session, 19 et 20 octobre 2026

Ce qui a été changé, trouvé et vérifié pendant ces deux jours. Les changements
visibles par l'utilisateur sont aussi résumés dans `CHANGELOG.md` (section
« Non publié ») ; les notes techniques pour le développement restent dans
`CLAUDE.md`.

## 1. Graphiques : plus de zoom trompeur

- **Constat (run 186, « test refill 3 ») :** le graphique de History semblait
  partir dans tous les sens. Le débit pesé variait seulement de ±1 % (8,24 à
  8,43 ml/min pour 8,33 demandés), mais l'axe vertical était calé sur le
  minimum et le maximum des données, sans graduation : ±1 % remplissait toute
  la hauteur.
- **Correction :** l'axe part de 0, monte jusqu'à une valeur ronde un peu
  au-dessus des données, et ses lignes portent des graduations chiffrées
  (`ui/src/lib/chartscale.js`, testé). Vaut pour tous les graphiques
  (Overview, History, aperçu de New run, suivi pesé).
- **Sur le graphique :** les remplissages apparaissent en bandes bleues, en
  direct et dans History (légende « Bottle refilled »).

## 2. L'état du run lisible d'un coup d'œil

Avant, « en marche normale » et « remplissage » ne se distinguaient que par un
petit rond. La refonte a été faite sur maquette, ajustée, puis mise en place.

- **Bandeau de l'Overview :** il prend la couleur de l'état seulement quand il
  faut y regarder : **bleu** pour un remplissage ou une stabilisation,
  **orange** pour un remplissage arrêté ou un flacon qui ne se remplit pas,
  **rouge** pour une alarme. La **marche normale reste blanche** : le vert
  attirait l'œil sans raison.
- **Grand picto animé** dans un rond : tête de pompe dont les galets tournent
  **dans le sens réel du run** (cw ou ccw, transmis par le daemon), flacon qui
  se remplit, flacon plein avec ses bulles, flacon avec point d'exclamation,
  triangle d'alarme centré sur ses coins. Le picto ne bouge que si quelque
  chose se passe : un daemon muet le fige.
- **Étiquette d'état** (Feeding, Refilling, Settling, Refill stopped, Not
  filling, Alarm) et, pendant un remplissage, une **barre de progression** :
  grammes, pourcentage, montée mesurée (g/min) et temps restant.
- **Sur les autres pages** (History, Tubing calibration, Settings) : une barre
  fine en haut, avec le picto, l'état et « Open Overview ». Une première
  version mettait une pastille dans le menu de gauche : retirée, on aurait dit
  une entrée de menu.
- **Titre de la fenêtre et icône d'onglet** suivent l'état (« Refilling 42 % ·
  Fermentool », icône colorée seulement quand il faut regarder).
- La deuxième pompe animée à côté du débit a été retirée : elle faisait
  doublon avec celle du bandeau.
- **Polices :** titres d'état en **Barlow** (intégrée à l'app, comme IBM
  Plex), le reste en IBM Plex Sans. Manrope, Barlow, Archivo et Recursive ont
  été comparées sur la maquette ; Plex reste la plus sûre pour lire des
  chiffres (pas de confusion 1/l/I, 0/O).

## 3. Audit UX et ses corrections

Audit fait sur la dernière version, sans rien cliquer sur le matériel (un run
tournait) : contrôles automatiques, axe sur les quatre pages en clair et en
sombre, jugement visuel des captures. Corrections validées sur maquette, puis
codées.

- **Overview :** le bandeau ne répète plus de débit (il affichait la valeur
  envoyée à la pompe, 8,043, à côté des 8,330 demandés). « Next refill around
  15:04 » au lieu d'un « Next thing for you » sans rien à faire. Le panneau
  Delivery dit « 100.0 % delivered · +0.0 % vs asked » (un « 0.0 % » seul se
  lisait « rien livré »).
- **History :** colonne avec le numéro du run, durée réellement tournée
  (« 49 m of 8 h » au lieu des 8 h prévues), « stopped » en gris neutre (le
  rouge reste pour « aborted »), « Reset history… » en lien discret sous la
  liste.
- **Contrastes :** 67 échecs de contraste WCAG AA trouvés par axe sur les
  quatre pages, en clair et en sombre ; **aucun** après correction. Gris
  secondaire un cran plus foncé, encre foncée sur les boutons turquoise du
  thème sombre (le blanc donnait 2,4:1), couleurs de texte dédiées sur les
  fonds teintés.
- **Divers :** dates en anglais quel que soit le PC (elles sortaient en
  français, « lun. 19 oct. »), titre de page pour les lecteurs d'écran, champs
  de calibration en orange seulement une fois laissés vides.
- **Reste :** l'ordre des titres sur l'Overview (h1 puis h3) et deux zones
  latérales sur Tubing calibration, remarques mineures d'accessibilité.

## 4. Remplissage automatique : jugé sur la balance seule

- **Constat (runs 186 et 192) :** le remplissage était coupé avec
  `alarm_refill_no_flow` alors que la pompe de remplissage tournait bien. La
  balance montrait un gain régulier d'environ 1,7 g/s dès la première seconde,
  soit environ 100 ml/min.
- **Cause :** le daemon comparait le gain à un débit attendu fixe,
  `expected_ml_min = 285`, mesuré une fois au banc à 350 rpm. La vitesse avait
  été baissée à 145 rpm pour simuler un remplissage lent ; le débit attendu,
  lui, ne suivait pas la vitesse. La pompe de remplissage n'est pas calibrée :
  tout débit supposé devient faux dès qu'on change de tuyau ou de vitesse. Le
  délai maximal (« too slow ») dépendait du même chiffre.
- **Nouvelle règle :** pendant un remplissage, le poids doit **monter** d'au
  moins 10 pas de balance (2 g au minimum) sur les 30 dernières secondes, donc
  la pompe de remplissage apporte plus que ce que l'alimentation tire.
  - S'il ne monte pas : alarme `alarm_refill_no_rise` sur le téléphone
    (relancée jusqu'à acquittement), **la pompe continue** : l'amorçage n'a
    peut-être pas pris, et on veut le remplissage même mauvais. Quand le poids
    remonte, `refill_rising` efface l'alarme.
  - Seuls arrêtent la pompe : le niveau haut atteint, le dépassement de 50 g,
    un Stop non confirmé.
  - Le temps restant, l'arrêt juste avant le niveau haut et le journal
    utilisent la montée mesurée ; `refill_start` et `refill_stop` notent la
    vitesse en rpm et la montée nette en g/min.
- `expected_ml_min` est retiré ; s'il reste dans `config.toml`, il est ignoré.
  La vitesse en rpm est le seul réglage de la pompe de remplissage.
- Spécification mise à jour :
  `docs/superpowers/specs/2026-10-02-auto-refill-design.md`.

## 5. Panneau « Feed bottle » pendant un remplissage

- **Constat (run 194, 20/10 à 8:33) :** en plein remplissage, « weighed out »
  affichait environ 200 g de moins que la tourie pesée à la main, puis
  redevenait juste une fois le remplissage enregistré.
- **Cause :** le panneau calcule poids de départ − poids actuel +
  remplissages terminés ; pendant un remplissage, le poids remonte avant que
  le remplissage soit enregistré. Le livré pesé du daemon (panneau Delivery),
  lui, était juste.
- **Correction :** pendant un remplissage et 20 s après, le panneau affiche le
  livré pesé du daemon, marqué « estimated while refilling »
  (`ui/src/lib/balance.js`, testé sur le cas du run 194).
- La légende sous la barre de remplissage pouvait aussi se couper (« g » ou
  « min » seuls sur une ligne) : chaque morceau reste entier.

## 6. Run 194 : nuit de validation

Simulation, avec 5 L sur la balance de 6 kg, du comportement d'une bouteille
de 10 L (remplissages d'environ 30 min).

| Réglage | Valeur |
|---|---|
| Alimentation | 3,5 ml/min, puis 0 à 9:00 (courbe Step), ml/min, tuyau n°14, ccw |
| Régulation | Par la balance |
| Remplissage | Pompe de remplissage à 55 rpm, tous les 1,0 kg, retour à 4 656,7 g |
| Durée | 17:15:35 → 08:59:59, Stop à 09:00:12 |

| Résultat | Valeur |
|---|---|
| Demandé | 3 305,4 ml |
| Tourie pesée à la main | **3 305,4 g** |
| Livré pesé par le logiciel | 3 305,6 g (+0,006 %) |
| Écart à tout moment | −0,31 % … +0,29 % |
| Remplissages | 3, environ 27 min chacun, +37 g/min net, +998 g à chaque fois |
| Correction c autour des remplissages | −0,44 %, −0,33 %, −0,08 % |
| Alarmes | aucune |

- La courbe Step n'est pas proposée dans New run : le run a été créé par l'API
  du daemon, avec les mêmes réglages. Le palier final à 0 arrête vraiment la
  pompe d'alimentation (consigne sous le minimum de la pompe : Stop), sans
  attendre qu'on appuie sur Stop. Une courbe Constant aurait continué à
  alimenter après 9:00.
- Rendement de la pompe d'alimentation mesuré à 1,040 (elle livre 4 % de plus
  que sa calibration), corrigé par c ≈ ×0,961.
- La pompe de remplissage a donné environ 0,7 ml par tour sur ce montage
  (270 ml/min à 350 rpm le 15/10, environ 100 ml/min à 145 rpm, environ
  40 ml/min à 55 rpm).
- Rapport QC (en anglais) : publié comme page privée, tous les critères OK.

## 7. Commits de la session

| Commit | Contenu |
|---|---|
| `7d376f5` | Graphiques depuis 0, axe gradué |
| `c4f4da5` | Bandeau d'état coloré, pictos, barre de remplissage, titre d'onglet, bandes de remplissage |
| `62222ec` | Marche normale neutre, barre d'état hors Overview, pompes dans le sens réel |
| `e740b2d` | Corrections de l'audit UX |
| `a2f747c` | Remplissage jugé sur la balance seule, jamais coupé pour une pompe lente |
| `06141c8` | Légende de la barre de remplissage |
| `1c508cc` | « Feed bottle » pendant un remplissage |
| `cbfa9e1` | `.gitignore` : skills Claude Code installés localement |

## 8. Reste à faire

- **Courbe Step dans New run :** elle existe dans le moteur mais pas dans
  l'interface. Utile pour finir un run à 0 à une heure donnée sans passer par
  l'API.
- **Vitesse de la pompe de remplissage :** réglée à 55 rpm pour le run 194 ;
  la remettre selon le prochain essai (350 rpm avec une vraie bouteille de
  10 L et la balance de 15 kg).
- Remarques d'accessibilité mineures restantes (voir section 3).
- Un `write_fail` isolé sur la liaison série pendant le run 192 (12:28 UTC),
  revenu tout seul ; à surveiller s'il se répète.
