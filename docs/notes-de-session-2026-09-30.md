# Notes de session, 30 septembre au 2 octobre 2026

Ce qui a été fait, trouvé et décidé pendant la mise en route de Fermentool sur
le PC du labo, et ce qui reste à faire. Les changements visibles par
l'utilisateur sont résumés dans `CHANGELOG.md` (section « Non publié ») ; les
notes techniques pour le développement sont dans `CLAUDE.md`.

## 1. Installation du PC de développement

- Installés : Rust (rustup, toolchain stable), Node.js 24 LTS.
- Les Visual Studio Build Tools 2022 installés par rustup étaient **cassés** :
  erreur de lecture disque (CRC) sur un paquet pendant l'installation. On
  compile avec **Visual Studio Community 2022** et sa charge de travail
  « Développement Desktop en C++ ». Tant que les Build Tools cassés ne sont pas
  désinstallés, `cargo` les choisit par défaut : compiler depuis un
  environnement `vcvars64.bat` de Community (voir `CLAUDE.md`).
- `rustc` a planté trois fois avec `STATUS_HEAP_CORRUPTION`, et passé en
  relançant. Avec l'erreur CRC, c'est un signe possible de mémoire vive ou de
  disque défaillant : **lancer le Diagnostic de mémoire Windows et
  `chkdsk C: /scan`** avant de confier des runs longs à ce PC.

## 2. Le daemon qui tourne aujourd'hui

- C'est `target\release\fermentool-core.exe`, lancé à la main, **sans
  console**. Il n'est ni installé ni relancé automatiquement (pas de tâche
  planifiée sur ce PC).
- Données : `%APPDATA%\Fermentool\` (`config.toml`, `fermentool.sqlite`,
  `logs\`). Pompe sur COM8, balance sur COM7.
- Ne jamais lancer la version de développement (`target\debug\…`) : elle ouvre
  une console, et un clic dedans la met en mode sélection, ce qui **fige tout
  le daemon** (vécu le 30/09).

## 3. Ce qu'on a appris sur le matériel

- **Calibration et sens de rotation** : une calibration faite en `cw` ne vaut
  pas pour des runs en `ccw`. Ici la pompe débitait 7 à 12 % de moins en
  `ccw` (rendement 0,88 à 0,93). Toujours calibrer dans le sens et avec le
  montage des runs.
- **Première calibration** : `c0` = 6,08, la pompe ne débitait que 1/6 de sa
  consigne en ml/min. Cause probable : réglage tête/tuyau de la LabQ ne
  correspondant pas au tuyau de 0,8 mm (Fermentool n'écrit jamais ces
  registres). Corrigé côté pompe, puis recalibré (`c0` ≈ 1,007, CV 0,4 %).
- **Retard au démarrage** : environ 2 s de débit « manquent » au départ
  (pompe qui accélère et/ou filtre d'affichage de la balance). Négligeable
  (0,3 g à 9 ml/min), absorbé par la régulation. Le test qui départagerait
  pompe et balance (vitesse relue toutes les 100 ms) n'a pas été fait.
- **Comparaison avec une deuxième balance** (run 45) : 270,0 g sortis de la
  bouteille contre 268,4 g pesés dans le récipient. 0,2 % vient de
  l'étalonnage des deux balances (596,0 contre 594,8 g pour un même objet), le
  reste probablement du tuyau qui tire sur la bouteille. Fixer le tuyau sur
  une potence, avec du mou au-dessus de la bouteille. Laquelle des deux
  balances est juste reste à vérifier avec une masse étalon.
- **Balance au gramme et tourie de 10 L** prévues pour les runs de 100 h :
  tout reste juste, mais la régulation démarre plus lentement à bas débit
  (premier rendement mesuré après 50 g, soit environ 25 min à 2 ml/min). Une
  bonne calibration dans le bon sens devient d'autant plus importante.
  Prévoir un évent (filtre stérile) sur la tourie.

## 4. Runs analysés

| Run | Ce qu'il a montré | Suite donnée |
|---|---|---|
| 14 | Balance sous le récipient : poids qui monte, déficit à 199 %, R² à −14 | Réglage « Balance weighs », alarme « mauvais côté » |
| 16 | Choc de 3,4 g sur la balance compté comme du débit, correction poussée à +25 % pendant 7 min | Seuil de perturbation selon le débit |
| 19 | Cumul juste (100,5 %), mais facteur qui oscille de ±10 % | Fenêtre de mesure qui s'agrandit, pas réduits |
| 22, 23 | −10 % pendant 2 à 3 min au démarrage (calibration `cw` pour un run `ccw`) | Calibration `ccw` |
| 33 | 99,98 % après plus d'une heure | Référence de bon fonctionnement |
| 39 | Tuyau déplacé exprès : pompe à 78 %, correction en butée à +25 % | Limite réglable, démarrage rapide sur gros écart |
| 45 | Comparaison avec une deuxième balance (voir §3) | Fixation du tuyau |
| 48 | Bouteille vidée exprès la nuit : alarme après 9 min, pompe à vide 10 h, personne prévenu, facteur figé à +36 % | Alarme « bouteille vide », retour au facteur d'avant, reprise auto, notifications |

## 5. Décisions prises

- **Fin de courbe** : un run de dosage continue à la valeur finale, régulé et
  enregistré, jusqu'à Stop. Les essais de calibration, eux, s'arrêtent pile.
- **Volume manqué** pendant une alarme : gardé dans les totaux, **pas
  rattrapé** (une surdose brutale est pire pour la culture que le manque).
- **Volume affiché** : un seul endroit, la synthèse du suivi pesé. Le volume
  « commandé à la pompe » n'est plus affiché (il gonflait du facteur de
  correction).
- **Notifications par personne** : chacun reçoit seulement les alertes de ses
  runs ; le responsable est obligatoire au lancement dès qu'une personne est
  enregistrée.
- **ntfy plutôt que Teams** : un message Teams privé demande de construire un
  flux dans l'éditeur Power Automate (le modèle tout fait ne vise que des
  conversations de groupe), trop lourd à refaire pour chaque utilisateur.
  ntfy : une appli, un sujet secret, un QR code. Teams reste possible par
  personne.
- **Alarmes qui reviennent (2026-10-02)** : une seule notification se perd
  dans la pile du téléphone. Pushover (relance jusqu'à acquittement) est
  payant, un fork de l'appli ntfy demanderait une installation par APK et un
  réglage « Ne pas déranger » que seul l'utilisateur peut accorder. Choix :
  l'insistance est dans le daemon. L'alarme est renvoyée toutes les 3 min
  jusqu'à « Acknowledge » (bouton ntfy qui publie `ack <run>` sur le sujet
  `<sujet>-ack`, que le daemon lit toutes les 15 s : le téléphone n'a jamais
  besoin de joindre le PC). Essayé le jour même avec « Test alarm » : bouton
  affiché et acquittement reçu sur le téléphone d'Andrew (Android).
- **Déploiement** : quand l'utilisateur demande « déploie », le run en cours
  est arrêté proprement sans redemander.

## 6. Utiliser les notifications (pour chaque utilisateur)

1. Installer l'appli gratuite **ntfy** sur le téléphone (aucun compte).
2. Fermentool, Settings, Notifications : **Add a person**, son nom,
   **Generate** (sujet privé). L'enregistrement est automatique (« ✓ Saved »).
3. **QR** : scanner avec le téléphone (Android : abonnement automatique ;
   iPhone : copier le sujet, puis + dans l'appli).
4. **Test** : une notification doit arriver. **Test alarm** : une alarme
   d'essai revient toutes les minutes jusqu'à **Acknowledge** sur la
   notification (5 fois au plus).
5. Dans l'appli, autoriser les messages urgents à passer outre « Ne pas
   déranger », pour être réveillé par une alarme la nuit.

Le sujet est la seule clé : ne pas le partager.

## 7. Reste à faire

Par ordre d'importance pour des runs de 100 h sans surveillance :

1. **Commiter** le travail depuis `eb4b72e` : alarmes, remplissages, export
   de la balance, notifications ntfy et Teams, enregistrement automatique.
2. **Relance automatique** : le réglage `resume.prompt = false` (case « Ask
   before resuming » dans Settings) n'est branché sur rien. Après un plantage
   ou une coupure, la reprise attend un clic. À brancher.
3. **Version installable** (installeur, `docs/RELEASE.md`) avec tout ce
   travail : démarrage à l'ouverture de session et relance en cas de
   plantage. Aujourd'hui, rien ne relance le daemon.
4. **Réglages Windows** du PC des runs : pas de mise en veille, mises à jour
   suspendues pendant les runs, mise en veille sélective USB désactivée.
5. **Balance au gramme** : relier aux pas de la balance les seuils encore
   fixes en grammes (alarme « mauvais côté » à 2 g), et simuler des runs à 1 g
   de précision, dont un run accéléré de 100 h (exponentielle, usure du tuyau,
   remplissages, reprise).
6. **Run d'essai long** (12 à 16 h, eau, exponentielle) sur le montage final,
   avec la deuxième balance si possible.
7. **Diagnostic mémoire et disque** du PC (voir §1).
8. Désinstaller les Build Tools 2022 cassés, pour que `cargo` marche sans
   `vcvars64.bat`.
9. Mettre à jour `devalue` (alerte npm sur un outil de compilation de
   l'interface, sans effet sur l'application).
10. Optionnel : test de démarrage (pompe ou balance en retard ?), masse
    étalon pour la balance, éditer une calibration déjà enregistrée.
