# Notes de session, du 26 au 28 octobre 2026

Ce qui a été changé, trouvé et vérifié pendant ces trois jours. Les
changements visibles par l'utilisateur sont aussi résumés dans `CHANGELOG.md`
(section « Non publié ») ; les règles pour le développement sont dans
`CLAUDE.md`. Suite des notes du 20 et 21 octobre
(`notes-de-session-2026-10-21.md`).

Tout est déployé le 28 octobre (run 242 arrêté pour ça, enregistré
`completed`). **Rien n'est commité** : voir la section 12.

## 1. Place sur le disque (26 et 27 octobre)

C: était à 8,3 Go libres. Supprimé : anciennes versions de VS Code et de
l'extension Claude Code, cache des extensions VS Code, anciennes versions de
Claude CLI, toolchain Rust `stable` (le projet est fixé sur 1.98.1, devenue la
toolchain par défaut), fichiers temporaires, et 2,3 Go laissés dans `Temp` par
une installation de Visual Studio. Gardé : les dossiers `target` (ils
accélèrent les builds), l'historique Fermentool, les données d'expérience du
Bureau.

- Trois dossiers restent verrouillés tant que VS Code n'a pas redémarré : il
  tourne depuis le 15 octobre sur une ancienne version, sa mise à jour du
  23 octobre attend (`2a59476c9b`, `Temp\vscode-stable-user-x64`, extension
  Claude 2.1.296). À supprimer après un redémarrage complet de VS Code.
- `target\debug` (3,8 Go) revient à chaque `cargo test`.

## 2. Temps depuis le début du run dans les journaux

- L'Overview (« Recent activity ») ne donnait que l'heure de chaque
  événement : il dit maintenant aussi le temps depuis le début du run, en
  dessous (`runTimeAt`, `lib/rundetail.js`).
- Un changement de débit : « since Tue 08:15, at 18 h 05 » sous le débit et
  dans « Change rate », de même dans le choix du débit de « Run again »
  (`rateLabel`, `lib/ratechange.js`).
- Le journal de History comptait déjà en temps du run.

## 3. Prochain refill faux après un changement de débit

- **Constat :** après « Change rate », l'heure du prochain remplissage
  arrivait trop tôt après une baisse, trop tard après une hausse.
- **Cause :** l'Overview projette la consommation avec
  `rate_g_per_min / last_target`. Ce `rate_g_per_min` était la pente de tout
  le poids pesé depuis le dernier remplissage : il mélangeait l'ancien et le
  nouveau débit longtemps après le changement (run 224 : 7,2 g/min annoncés
  86 s après un passage à 10 ml/min).
- **Correction :** la pente repart de zéro à chaque changement de débit, comme
  après un remplissage (`set_constant_rate`). L'heure disparaît ~10 s, puis
  peut bouger un peu la première minute.
- **Pas touché :** le remplissage lui-même part sur le poids réel, il était
  déjà au bon moment. Seule la prédiction était fausse.

## 4. Calibration : pompe qui tourne à l'envers

- **Constat (27 octobre, run 227) :** mesure lancée en cw au lieu de ccw. La
  pompe a repoussé le feed du tuyau dans la bouteille, le poids est monté, et
  la page l'a compté comme du feed recueilli. L'alarme « slow » n'est venue
  qu'une fois le tuyau vide, au bout de plusieurs minutes.
- **Cause :** le poids recueilli était pris en valeur absolue et le sens
  attendu déduit du premier mouvement, sans regarder la place de la balance.
- **Correction (`lib/stall.js`) :** le sens attendu vient de `[scale]
  position` (`weighDir` : feed = le poids descend, receiver = il monte). Un
  poids qui part dans le mauvais sens de 3 × `movedG` (0,6 g sur une balance
  au 0,1 g) lève tout de suite l'alerte `backwards` : « The weight is going
  the wrong way ». Le poids recueilli ne compte que dans le bon sens
  (`collectedG`) : une mesure à l'envers ne peut plus être enregistrée.
- **Limite connue :** calibrer avec un bécher sur la balance alors que le
  réglage dit `feed` donne maintenant une fausse alerte. Proposé, pas fait :
  un choix « Balance under: feed bottle / beaker » dans l'étape Pump setup.

## 5. Calibration : le sens reste modifiable après l'arrêt

- Avant, une seule mesure, même ratée et sans poids, verrouillait le sens :
  il fallait « Drop it » ou « Abandon ».
- Maintenant le sens se verrouille au premier poids enregistré
  (`directionLocked`) ; avant, le changer jette les mesures non pesées, qui
  ont tourné dans l'autre sens (`withDirection`, `lib/measurement.js`). Le
  tube et le réglage sont gardés.

## 6. Amorcer le tuyau depuis la calibration

- **Demande :** après une mesure à l'envers, le tuyau est vide ; l'amorcer
  au clavier de la pompe demande de rester appuyé environ une minute.
- **Maquette d'abord** (sur une page à part, quatre états, puis le choix du
  sens dans le bandeau, puis les fondus), mise en place sur « on peut partir
  là-dessus ».
- **Daemon (`engine/prime.rs`) :** `POST /api/pump/prime {direction}` fait
  tourner la pompe d'alimentation à sa vitesse max, dans ce sens ; demandé
  dans l'autre sens pendant l'amorçage, la pompe s'arrête et repart aussitôt
  (le compteur repart à zéro). `POST /api/pump/prime/stop` l'arrête. Arrêt
  automatique à `PRIME_MAX` (5 min) par la boucle de contrôle, comme le Test
  du refill. Refusé pendant un run ; un run qui démarre l'arrête d'abord.
  Événements `prime` / `prime_stop` sans run. `status.priming` (sens, depuis
  quand, max) pour l'interface.
- **Interface (`lib/prime.js`, `TubingCalibration.svelte`) :** bandeau dans
  Measurements : orange après une mesure à l'envers (`line_emptied` dans le
  brouillon), bleu pendant l'amorçage (compteur, « Stop priming »), « Line
  primed » ensuite. Le sens se choisit dans le bandeau (c'est celui de la
  calibration). Aucune mesure ne démarre pendant l'amorçage. Fondus au
  changement d'état.
- **Vérifié** sur une copie locale de la page avec le daemon simulé dans le
  navigateur (faux status, faux brouillon, toute écriture interceptée) : la
  pompe réelle n'a pas bougé, un run était en cours.
- **Deux bugs trouvés pendant cette vérification :** le compteur restait à
  0:00 (l'horloge de la page relancée à chaque status, avant d'avoir battu une
  fois) ; et voir section 7.
- **Limite :** si le daemon plante pendant l'amorçage, la pompe continue :
  l'arrêt à 5 min est celui du daemon. Un arrêt normal du daemon arrête bien
  l'amorçage.

## 7. Boutons de choix : le fond glisse

- Les 10 groupes de boutons de choix (cw | ccw, mm | inches, rpm | ml/min…)
  font glisser leur fond teal vers le choix cliqué (`lib/segthumb.js`,
  action `use:segThumb`). Le fond est le `::before` du `.seg`, placé par
  `--thumb-x` / `--thumb-w` : rien n'est ajouté au DOM de Svelte. Mesuré, pas
  en fractions : les boutons n'ont pas tous la même largeur.
- **Bug trouvé avant déploiement :** l'observateur réagissait au changement
  de classe qu'il faisait lui-même sur le `.seg`, en boucle : la page se
  figeait entièrement. Corrigé (il ignore ses propres changements) ; une
  vérification du smoke test l'attrape désormais.

## 8. Police des chiffres : IBM Plex Sans

- L'utilisateur n'aime pas le zéro pointé d'IBM Plex Mono (qui n'a pas de
  zéro simple, seulement un barré). Six variantes comparées sur une page :
  choix **IBM Plex Sans**, chiffres de largeur égale (`tabular-nums` sur
  `body`), via le jeton `--mono` dont le nom est resté.
- Plex Mono retiré du bundle. La largeur des chiffres de Plex Sans est
  exactement 0,600 em, la valeur que supposait déjà le placement des
  étiquettes de graphique.
- `DESIGN.md` et `CLAUDE.md` mis à jour : pas de zéro pointé ni barré.

## 9. Réserve de refill vide : la pompe ne s'arrêtait jamais

- **Constat (28 octobre, run 242, 13:12) :** réserve vide en plein
  remplissage. L'alarme « no rise » est partie, mais la pompe de refill a
  continué jusqu'à un arrêt à la main (« Stop refill »).
- **Cause :** l'alarme était volontairement « dite, pas suivie d'effet »
  depuis le 19 octobre, et aucune durée maximale de remplissage n'existait.
  Le 19 octobre, la coupure avait été retirée parce qu'elle comparait le
  remplissage à un débit attendu (285 mL/min), ce qui coupait une pompe lente
  qui remplissait bien (runs 186 et 192).
- **Correction (`refill_cycle.rs`), à la balance seule, sans débit attendu :**
  si la bouteille a monté pendant ce remplissage puis ne monte plus sur 30 s,
  la pompe de refill s'arrête, alarme `alarm_refill_dry` (notifiée), le
  remplissage automatique attend « Refill now ». Si rien ne monte dès le
  départ (amorçage), la pompe continue `PRIME_GRACE_S` (3 min) au plus, puis
  même arrêt. Une pompe lente qui remplit monte toujours : jamais coupée.

## 10. Feed bottle : 9 g de trop après le refill raté

- **Constat :** juste après l'arrêt du refill raté, « weighed out » de la
  carte Feed bottle a pris ~10 g de plus que la balance de récupération,
  alors que tout concordait avant.
- **Vérifié :** carte 2134,6 g contre 2125,4 g pour le livré du daemon (qui
  concorde avec la balance de récupération). **Erreur d'affichage seulement :**
  la régulation utilise le livré du daemon, qui était juste ; le verdict et la
  courbe d'écart de History aussi.
- **Cause :** la carte (et le tableau Feed bottle de History, « Poured »)
  ajoute le feed tiré pendant un refill (`drawn_g`, estimé). Pour ce refill,
  l'estimation a couru jusqu'à 13:16:18 alors que le poids « après » avait été
  lu à 13:15:20 : 58 s de feed (~9,8 g) comptées deux fois. C'est la pompe
  restée en marche à vide qui a créé le décalage.
- **Correction (`tracking_report`) :** l'estimation s'arrête au poids
  « après ». Vaut aussi pour les runs passés : le run 242 donne 2252,2 g
  contre 2252,7 g après déploiement.
- Pendant le refill, la carte affiche le livré du daemon, qui collait à la
  réalité en temps réel ; le saut venait du passage au calcul de la bouteille
  à la fin du refill.

## 11. Déploiements

- 27 octobre : run 224 arrêté, déploiement (temps du run dans le journal,
  prochain refill). Puis déploiement de la détection « à l'envers ».
- 28 octobre : run 242 arrêté (courbe finie depuis 11:16, en maintien),
  déploiement de tout le reste. Smoke test : 58 vérifications passent.
- Tests : 430 côté daemon, 241 côté interface ; `npm run check` propre.

## 12. Pas commité, à faire

- **Rien n'est commité.** Le CHANGELOG, `CLAUDE.md` et `DESIGN.md` sont à
  jour.
- **`ui/src/App.svelte` a une modification qui n'est pas de cette session :**
  elle retire le lien direct `#settings/pumps` (dont le smoke test se sert)
  et le clic sur la barre de connexion vers Settings, ce qui ressemble à un
  ancien état du fichier réenregistré par-dessus le merge. Elle a été tenue
  hors de tous les bundles (mise de côté le temps de chaque build) et laissée
  telle quelle. À trancher : l'annuler ou la garder.
- La branche `feat/rate-change-reset` (`.worktrees/`) touche aussi au
  changement de débit : vérifier la fusion avec la section 3.
- Proposé : choix « Balance under: feed bottle / beaker » dans la
  calibration (section 4).
- Supprimer les trois dossiers verrouillés après redémarrage de VS Code
  (section 1).
- Premier amorçage réel à observer à la prochaine calibration.
