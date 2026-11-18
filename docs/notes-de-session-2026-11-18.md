# Notes de session, 17 et 18 novembre 2026

## 1. Version 0.1.8

- `steps-raw-audit` fusionnée dans `main` le 17 (avance rapide, sans
  conflit) : Steps, consigne brute, audit, caméra, démarrage sur le ratio
  appris. `main` et `steps-raw-audit` sont au même commit depuis.
- Installateur `Fermentool_0.1.8_x64-setup.exe` construit le 18 depuis
  `main`, tag `v0.1.8`. Contrôles : tests Rust (551 dans le daemon), smoke
  test dans la vraie fenêtre (103 contrôles), daemon vivant après la
  fenêtre tuée, pas de `VCRUNTIME140`, ni `.sqlite` ni `config.toml` dans
  le paquet. **Pas encore installé sur un PC vierge.**
- `copy-sidecar.ps1` a échoué à son `npm ci` : VS Code tient le binaire de
  rolldown. Étapes faites à la main avec `npm install` (RELEASE.md, étape 4).
- Le daemon du banc tourne en 0.1.8 (`target\release`), lancé par la
  fenêtre de contrôle.

## 2. La fenêtre de bureau n'est pas un navigateur (17 novembre)

Vérifié dans la vraie fenêtre WebView2 : pas de nouvel onglet, pas de
téléchargement par lien blob. Tout ce qui ouvre ou enregistre passe par le
daemon (`/api/files/save`, `/api/about/open`,
`/api/timelapses/images/{id}/open|show`). Le smoke test peut piloter la
fenêtre (`SMOKE_CDP=9444`). L'interface de la fenêtre est compilée dans
`fermentool.exe` : un déploiement du daemon ne la change pas.

Le 18 : la fenêtre s'ouvre agrandie, ignore F5 et Ctrl+R, n'a plus le menu
du navigateur au clic droit (sauf champs et texte sélectionné). Les cinq
points de l'audit d'interface du 3 novembre sont faits (CHANGELOG).

## 3. Valeur affichée par la pompe après Stop : résolu, rien à corriger

Run 267 (2.000 ml/min, sans régulation), registres lus après l'arrêt :
vitesse 43.3 rpm, débit 2.0018 ml/min, marche 0. L'écran de la LabQ
affiche 43.3 : la vitesse moteur qu'elle a calculée elle-même depuis sa
table tête/tuyau. Le daemon n'envoie que « stop ». Pour un chiffre parlant
sur la pompe, lancer en rpm. Le run 267 peut être supprimé.

Trouvé au passage et corrigé : « Curve done » donnait la consigne corrigée
envoyée à la pompe (11.363 sur le run 266 à 12 ml/min), il donne la
valeur de la courbe.

## 4. `feat/rate-change-reset` : pas fusionnée

Changer le débit pendant le maintien de n'importe quelle courbe, et
remettre la correction à zéro à chaque changement. Branche locale
seulement (`.worktrees/rate-reset`), partie d'un `main` ancien. La fusion
est dans la branche locale `merge/rate-change-reset` : conflits résolus,
compile, **deux tests du moteur échouent** :

- `the_regulation_rides_a_change` : c plonge de 1.3 % environ 140 s après
  un changement (6 → 3 ml/min), quand la première fenêtre courte du ratio
  se remplit, puis revient. Tolérance 0.6 %. La branche seule a la même
  plongée à 0.5 %. Essayé sans succès : geler le ratio comme après un
  refill (`ratio_frozen`, casse deux tests de la branche qui veulent un
  ratio réappris vite à basse vitesse), écarter les 5 s qui suivent le
  changement (aucun effet).
- `a_change_in_a_perturbation_keeps_its_c_through_a_slow_rise` : suppose
  une part de rattrapage dans c juste avant le changement, que le retard de
  pesée de 3 min (`WEIGH_DELAY_S`) cache maintenant. Probablement à
  réécrire (arrêt de 3 s plus tôt), à confirmer.

À trancher d'abord : comment la remise à zéro et le `k_ref` appris se
combinent à un changement de débit.

## 5. Rangement

Worktrees fusionnés supprimés (`camera`, `seed-k`, `Fermentool-2026-rate`)
et leurs branches locales. En supprimant `seed-k` à la main, le
`ui/node_modules` principal a été vidé (un lien de jonction) : reconstruit
par `npm install`, lockfile inchangé. Avant de supprimer un worktree à la
main, retirer d'abord ses jonctions. Il reste `.worktrees/rate-reset`.

## 6. Liste ouverte (remplace la section 5 du 9 et la section 4 du 6)

- Installer 0.1.8 sur un PC vierge et vérifier : fenêtre agrandie, pompe,
  balance.
- Terminer `merge/rate-change-reset` (section 4).
- Sur un vrai run à plus de 4 ml/min : pas de fausse alarme « Pump ratio
  moved too fast » sur plusieurs heures, le ratio suit l'usure, pas de pic
  de c au refill suivant.
- Démarrage sur le ratio appris : le prochain run régulé sur la
  calibration 17 enregistre son k, le suivant démarre à environ x0.95.
- Balance muette plus de 3 s : gardé tel quel (décidé le 9), à reprendre
  (piste dans les notes du 9, section 5).
- Refill, limites connues (notes du 6, section 4) : bouteille tenue hors de
  la balance plus de 30 s, Stop retenté sur une pompe muette, crash en plein
  remplissage, perturbation pendant un remplissage, cycle en heure murale.
- Hub RS-485 isolé (éteindre une pompe coupe l'autre).
- Caméra, plan D : lecteur, onglet Timelapses, section History, exports,
  renommer et supprimer.
- Une icône ajoutée se prend dans Solar (`linear`).
- Fait depuis les notes du 6 : les commits des 3 et 6 novembre,
  `App.svelte` (lien direct `#settings/...` et clic sur la barre de
  connexion rétablis, commités).
