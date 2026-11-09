# Notes de session : 6 au 9 novembre 2026

Suite des notes du 6 (`notes-de-session-2026-11-06.md`, dont la section 4
reste ouverte). Rien n'est commité depuis : tout ce qui suit est dans la
copie de travail, et c'est elle qui tourne sur le banc.

## 1. Profil Steps dans New run (6 novembre)

- Nouvelle forme **Steps** : un tableau de débits, chacun à partir d'une
  heure donnée (heures et minutes), jusqu'à 1000 lignes. La courbe est la
  `CurveParams::Step` du moteur, qui existait déjà : aucun changement du
  daemon.
- **Import depuis Excel** (`.xlsx` ou `.csv`) et **modèle à télécharger**
  (`step-profile-template-ml-min.xlsx` ou `-rpm`) : validation de données
  dans les cellules, feuille « Read me ». Lecteur et écrivain xlsx sans
  dépendance (`ui/src/lib/xlsx.js`). Les cellules au format heure (12:30)
  sont lues comme des heures. Les erreurs citent le numéro de ligne Excel.
- **Un palier qui commence avant le précédent est une erreur** (cas
  signalé : la 4e ligne remise dans le passé). Le message s'affiche sous la
  ligne fautive et l'aperçu attend la correction.
- L'écran suit la maquette v2 : compteur, Import from Excel, Template,
  menu « … » avec Clear all, barre de durée par palier, survol qui surligne
  le palier sur le graphique, ligne End (fin du run). Logique testée dans
  `ui/src/lib/steps.js`.
- Overview d'un run Steps : « 4 steps, 1.000 to 6.000 ». Run again
  reprend les paliers.

## 2. Audit d'utilisabilité avant export (6 novembre)

- Fenêtre de 900 px : plus aucun débordement (colonne de page en
  `minmax(0, 1fr)`, sections de Settings en `@container pane`). Le smoke
  test le vérifie.
- Balance regulation dessiné à sa vraie largeur. Settings > Balance
  grisé pendant un run régulé. Plus de « Saved » avant d'avoir enregistré
  dans Notifications. Recent activity en phrases.
- **Graphique de correction en paliers** : c'est voulu. c change toutes
  les 10 s, par pas de 2 % au plus (0,5 % une fois stabilisé). Run 259 :
  67 changements, un toutes les 11 s, pas médian 0,42 %. Pas modifié.

## 3. Installateur 0.1.6 (6 novembre)

- `target\release\bundle\nsis\Fermentool_0.1.6_x64-setup.exe` (6,8 Mo),
  construit depuis la copie de travail non commitée.
- Contrôles de `docs/RELEASE.md` passés : CRT statique, version 0.1.6,
  le daemon survit à la fenêtre tuée, aucun historique dans le paquet.
- Écarts : `npm install` au lieu de `npm ci` (VS Code verrouille le
  binaire natif de rolldown), étapes de `copy-sidecar.ps1` faites à la
  main ; `tauri-cli` 2.12.4 installé.
- Run 260 arrêté à la demande avant le déploiement.

## 4. Sans régulation : consigne brute (9 novembre)

- Demande : sans régulation, ne pas choisir de tuyau, envoyer les rpm ou
  ml/min tels quels, l'opérateur calibre la pompe lui-même.
- Avant : en ml/min sans régulation, New run proposait un tuyau calibré
  et présélectionnait celui du dernier run ; la pompe était alors pilotée
  en rpm d'après la calibration.
- Maintenant : « No regulation » ne montre plus le choix du tuyau, le run
  part sans `tubing_calibration_id`, la ligne Tube du récapitulatif
  disparaît, une phrase rappelle de régler tête et tuyau sur la pompe en
  ml/min. Avec Balance regulation, rien ne change.
- Smoke test « No regulation asks for no tube » (75/75). Déployé sur le
  banc ; **pas dans l'installateur 0.1.6**.

## 5. Pas fait, à faire

- Commité et poussé sur la branche `steps-raw-audit` (pas encore fusionnée
  dans `main`). Installateur 0.1.7 construit depuis elle ; son contrôle de
  fenêtre (RELEASE.md, étape 6) reste à faire, le run 264 occupait le
  port 8730. Puis taguer `v0.1.7`.
- Le daemon du banc tourne encore en 0.1.6 + la section 4 : le redéployer
  en 0.1.7 une fois le run 264 fini.
- La section 4 des notes du 6 reste ouverte (`App.svelte` à trancher,
  vérification sur un vrai run à plus de 4 ml/min, limites du refill).
- Le détail d'un run dans History n'a pas été repris à 900 px.
- C: : 3,3 Go libres ; `target\debug` pèse 4,4 Go.
- **Balance muette plus de 3 s : gardé tel quel (décidé le 9 novembre),
  à reprendre à tête reposée.** Tout silence est compté `k_est` x commandé,
  jamais pesé, même long (`settle_anchor`, `anchor_a_silent_balance`). Revue
  de code : une balance éteinte 3 h sur une pompe qui dérive de 8 % laisse
  ce manque invisible, le run paraît dans la cible. Piste proposée : au-delà
  de 10 min de silence, garder la pesée au retour si elle est plausible
  (poids en baisse, entre 0,7 et 1,3 fois l'estimation), sinon l'estimation
  signalée au journal. À tester : bouteille manipulée, remplissage, coupure
  longue avec dérive. Même revue : chaque silence de 3 s vide la fenêtre du
  ratio et le fige ~8 min (PC chargé) ; acceptable tant que les silences
  restent rares.
