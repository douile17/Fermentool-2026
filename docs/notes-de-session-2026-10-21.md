# Notes de session, 20 et 21 octobre 2026

Ce qui a été changé, trouvé et vérifié pendant ces deux jours. Les changements
visibles par l'utilisateur sont aussi résumés dans `CHANGELOG.md` (section
« Non publié ») ; les règles pour le développement sont dans `CLAUDE.md`.
Suite des notes du 19 et 20 octobre (`notes-de-session-2026-10-19.md`).

## 1. Fiche d'un run dans History, refaite

Audit UX de la fenêtre qui s'ouvre au clic sur un run, maquette (run 194,
vraies données), puis mise en place.

- **Avant :** le nom du run en troisième ligne, la courbe demandée en premier,
  la réponse à « est-ce que ça s'est bien passé ? » repliée, le même chiffre
  dit trois fois, un paragraphe « Balance: » illisible, des noms internes dans
  le journal (`REFILL_START`), un gros bouton rouge « Delete run ».
- **Maintenant, dans l'ordre :** nom en grand, `Mon 19 Oct 17:15 → Tue 20 Oct
  09:00 · ran 15 h 44 m · Andrew`, croix pour fermer ; **verdict** (On / Off
  target à ±2 %, plus grand écart après 15 min, 4 chiffres dits une fois) ;
  **Feed rate** sur un axe en heures d'horloge ; **Running total vs
  requested** (écart cumulé, moyenne 5 min, échelle symétrique) ; tableau
  **Feed bottle** ; puis repliés **Settings**, **Regulation**, **Journal** en
  mots (une ligne par remplissage). « Delete run… » en lien discret.
- **Pendant un remplissage la balance ne pèse pas** : les deux graphiques
  passent en pointillés gris dans les bandes bleues ; sur l'écart, une droite
  entre les points pesés de part et d'autre (l'estimation brute faisait un
  gribouillis).
- **« Fed during refill » est une estimation** (rendement de la pompe × ce
  qu'on lui a demandé, jamais corrigée après coup) : marquée `≈`, en gris.
  « Poured » (gain sur la balance + cette estimation) est juste à ~1 g près,
  affiché normalement.
- Les explications (le `≈`, le lissage, la tolérance) iront dans la
  documentation technique, pas en phrases dans la fenêtre.
- Code : `ui/src/lib/rundetail.js` (testé), `GapChart.svelte`.
  `TrackingPanel.svelte` supprimé (plus utilisé).

## 2. Courbe Step : fausse pente à la fin

- **Constat (run 194, lancé par Claude Code directement par l'API du daemon,
  courbe Step 3,5 ml/min puis 0 à 9:00) :** l'Overview a affiché le débit
  descendant de 3,5 à 0 pendant ~5 min alors que la pompe ne bougeait pas.
- **Vérifié dans le journal :** consigne écrite ~3,34 ml/min (3,5 × c) chaque
  seconde jusqu'à 08:59:58, relue identique dans la pompe, balance −3,5 g/min
  jusqu'au bout, puis 0 d'un coup. **Le contrôle de la pompe était juste**,
  seul l'affichage se trompait.
- **Cause :** l'aperçu de la courbe (`/api/preview`) prend 200 points
  réguliers que l'interface relie en ligne droite ; le saut tombait entre les
  deux derniers. Le grand chiffre de l'Overview interpole sur ces points.
- **Correction :** chaque saut d'une courbe Step (ou Custom « hold ») a deux
  points au même instant, avant et après (`fermentool-curves`, testé).
- Step n'existe pas dans New run (seulement dans le moteur et l'API) : c'est
  pour ça que « Run again » est grisé sur le 194.

## 3. µ du débit retiré

- « µ asked / µ delivered » était l'exposant de la courbe d'alimentation et
  celui de l'exponentielle ajustée sur le volume pesé. Un microbiologiste lit
  µ comme le taux de croissance de la biomasse : 3,8 h⁻¹ sur un run de test
  (6 → 9 ml/min en 1 h), ou un « µ mesuré » qui n'est pas une mesure de la
  culture. Et l'ajustement variait de 5 % en début de run, volume juste à
  0,06 %.
- **Retiré** de l'Overview, de History, du rapport de suivi du daemon et du
  calcul. Le µ saisi en mode Physio (taux de croissance visé de la souche)
  reste, c'est bien un taux de croissance de la biomasse.
- Rappel : une exponentielle 6 → 9 ml/min en 3 h (×1,5, µ 0,135 h⁻¹) ne
  s'écarte d'une droite que de 0,15 ml/min au milieu, invisible sur un axe
  0–10 : la courbe est bien exponentielle.

## 4. Calibration du tuyau

- **Le mot « burst » disparaît de l'interface :** une calibration, ce sont
  trois **measurements** (ni « calibration », qui désigne l'ensemble, ni
  « run », qui est un run d'alimentation). Le code, l'API et le brouillon
  gardent `burst`. Le smoke test échoue si la page affiche « burst ».
- **Cartes cliquables :** le détail d'une mesure montre poids, débit, durée,
  pente lue à la balance, variation entre les deux moitiés, et la courbe de
  la balance seconde par seconde (`MeasurementChart.svelte`).
- **« ↻ Redo » sur chaque carte**, à tout moment (avant : seulement les 3
  faites), jamais pendant qu'une mesure pompe ; confirmation dans le détail.
- **« ↻ Redo all 3 »** garde le tube et le réglage de la pompe (Abandon efface
  tout) ; en haut du panneau des mesures, et à l'étape Result à la place des
  trois Redo.
- **Pesée = moyenne des lectures stables** sur 5 s, au départ et à la fin.
  Une balance qui oscille 27,4 / 27,5 / 27,6 g donnait la valeur du moment du
  clic. Limite : l'interface reçoit ~1 lecture/s, donc ~5 lectures.
- **Écart de 0,6 g le matin du 21 :** pas la moyenne. La pente pendant le
  pompage (indépendante de la pesée) était 2 à 3 % plus basse le matin
  (5,95 → 6,02 → 6,06 g/min) que la veille (6,15 à 6,19) et remontait : mise
  en route du tuyau ou de la pompe après la nuit. Conseil : faire tourner la
  pompe quelques minutes avant une calibration.
- Code : `ui/src/lib/measurement.js` (profil, moyenne, résumé du Redo all,
  testés).

## 5. Overview

- **Menu « Pump control » → « Feeding profile ».** (L'étape 3 de New run
  s'appelle « Feed profile » : à harmoniser si ça gêne.)
- **Étiquette « refill 13:45 »** : elle s'écrivait sur la valeur de fin de
  courbe. Elle prend maintenant la première place libre à côté de son trait
  (droite puis gauche, de haut en bas), à l'écart des autres textes et de la
  courbe ; sans place libre, elle n'est pas affichée (`lib/chartlabels.js`).
- **« Empty around … » retiré** (et la projection « runs dry », et les
  conseils qui en dépendaient) : la balance pèse aussi le flacon, son bouchon
  et ses tuyaux, dont le poids change d'un flacon à l'autre ; l'heure était
  trop tardive. L'heure du prochain remplissage reste (écart de poids, juste).

## 6. Remplissage : vitesse et coupure du 21 octobre (run 210)

- **Vitesse changée pendant un remplissage :** elle n'était écrite à la pompe
  qu'au début d'un remplissage. Elle l'est maintenant aussitôt (événement
  `refill_speed`). Le sens de rotation, lui, attend le remplissage suivant
  (inversé en cours, il viderait le flacon).
- **Coupure à 14:19:03 :** remplissage arrêté 42 g sous l'objectif, repris
  30 s plus tard (à 350 rpm), compté comme deux remplissages.
  Log : `scale: serial I/O error: failed to write whole buffer`.
- **Cause trouvée :** sur octobre, 170 erreurs de ce type, dont 163 pendant
  une session Claude Code (alors qu'une session n'est active que 28 % du
  temps de travail), presque toujours juste après une compilation (`cargo
  build`/`test`), un `npm run build`, Vite ou Edge sans fenêtre. PC saturé,
  Windows mettait plus de 200 ms à remettre 3 octets à l'adaptateur USB de la
  balance, délai au-delà duquel l'écriture était déclarée ratée.
- **Corrections :**
  - écriture vers la balance réessayée jusqu'à **800 ms** (`write_within`,
    `SCALE_WRITE_WITHIN`) ; une transaction tient sous 2,3 s, sous la limite
    d'âge d'une lecture (2,5 s). Sans effet sur la régulation (fenêtres de 2
    à 5 min) ni sur la pompe d'alimentation (écrite toutes les 150 ms, sans
    attendre la balance) ;
  - un remplissage n'est coupé qu'après **3 s sans lecture**
    (`REFILL_BLIND_S`), pas à la première lecture manquée ; à ~4 g/s, une
    douzaine de grammes au pire au-dessus du niveau ;
  - **pendant un run, les compilations et tests sont lancés en priorité
    basse** (`start "" /b /low /wait cargo …`, voir `CLAUDE.md`). Deux
    compilations complètes faites ainsi pendant le run 210 : aucune erreur de
    balance.

## 6 bis. Settings › About : ouvrir les dossiers

- « Copy » remplacé par **« Open folder »** (dossier des données, logs) et
  **« Show in folder »** (historique, réglages : l'Explorateur s'ouvre avec le
  fichier sélectionné). Une page ne peut pas ouvrir un dossier : c'est le
  daemon qui lance l'Explorateur (`POST /api/about/open`), et seulement pour
  ces quatre emplacements nommés (`data`, `db`, `config`, `logs`), jamais un
  chemin venu de la page (testé ; `C:/Windows` refusé, 400).
- Présentation refaite sur maquette : le dossier des données une fois en
  tête, son contenu en liste (icône, nom, ce que c'est), boutons alignés.
  `insideFolder` (`lib/fmt.js`, testé) écrit chaque fichier par son nom dans
  le dossier, ou son chemin complet s'il est ailleurs.

## 7. Commits de la session

| Commit | Contenu |
|---|---|
| `7ef6b2e` | Fiche d'un run dans History, verdict d'abord |
| `1c158a0` | Courbe Step : un saut, plus une pente |
| `fc6a36f` | µ du débit retiré |
| `2bd531d` | Calibration : mesures, détail, Redo, Redo all 3, pesée moyenne |
| `d2d3754` | Menu « Feeding profile » |
| `9ff778e` | Étiquette « refill » sans chevauchement |
| `cbcffb5` | Plus d'heure « runs dry » |
| `09c2b69` | Vitesse de remplissage en direct, balance plus tolérante |
| `fad63ac` | Notes de session, changelog, règles dans CLAUDE.md |

Les commits `build(ui)` qui les suivent portent le bundle.

## 8. Reste à faire

- **Déployer** les changements du 21 au premier arrêt du run 210 (étiquette
  refill, « runs dry », vitesse de remplissage, balance).
- **Un remplissage interrompu puis repris compte pour deux** dans History
  (+958,5 g et +38,7 g pour le run 210) : les réunir en un seul.
- **Brouillon de calibration disparu le 21** (24,3 / 24,5 / 24,6 g, non
  enregistré) : abandonné par l'utilisateur ? À confirmer ; les mesures
  restent dans le journal (runs 201, 204, 205).
- **Documentation technique** : estimation pendant un remplissage (`≈`),
  précision de « Poured », lissage de l'écart, tolérance ±2 %.
- **Step dans New run** (« arrêter l'alimentation à … »), sur maquette.
- History : lecture au survol du graphique de débit (dans la maquette, pas
  faite).
- Harmoniser « Feed profile » (New run) et « Feeding profile » (menu).
