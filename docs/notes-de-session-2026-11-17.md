# Notes de session, 16 et 17 novembre 2026

## 1. Question

Pendant un refill, la balance du flacon ne pèse pas la pompe d'alimentation
(le refill verse dans le même flacon). Le daemon compte alors `k × commandé`,
avec le k d'avant le refill. Hypothèse testée : la moyenne du k d'avant et du
k d'après estimerait mieux ce qui a vraiment été délivré, et pourrait servir
de correction après coup, avec la seule balance du flacon.

Critère fixé avant le test : la moyenne est retenue si elle est plus juste
sur au moins 4 refills sur 5, avec un gain moyen de plus de 0.2 g par refill.

## 2. Montage

- Balance du flacon : COM7 (Ohaus Ranger 7000), lue par le daemon.
- Balance de réception : COM9 (Ohaus Ranger 7000), sous le récipient où
  arrive l'alimentation, lue une fois par seconde par
  `scripts/receiver_log.py` (hors daemon), dans
  `essai-refill/recepteur.csv`. Elle ne sert qu'au test : la version finale
  n'a qu'une balance.
- Analyse : `scripts/refill_k_check.py <run> recepteur.csv`. Pour chaque
  refill, de son début à sa fin + `REFILL_SETTLE_S` (la période non pesée),
  le k sur 10 min avant et 10 min après, sur chaque balance, et la masse
  réellement arrivée.

Run 266 : Constant 12 mL/min, régulation sur la calibration 17, refill
automatique tous les 180 g, 69 min. Un premier essai (run 265) a été arrêté
au bout d'une minute : la ligne n'était pas amorcée, la balance du flacon ne
bougeait pas.

## 3. Résultats

| Refill | non pesé | réel (réception) | k d'avant (actuel) | k d'après | moyenne |
|---|---|---|---|---|---|
| 1 | 319 s | 63.80 g | -0.24 g | +0.08 g | -0.08 g |
| 2 | 316 s | 63.20 g | -0.14 g | +0.07 g | -0.04 g |
| 3 | 315 s | 62.90 g | -0.05 g | +0.21 g | +0.08 g |

Le 4e refill est tombé 2 min avant le Stop, sans période après : non compté.

- Les deux balances donnent le même k, à 0.1 % près : pas de perte ni
  d'évaporation entre le flacon et le récipient, la comparaison est valable.
- La moyenne fait mieux sur 2 refills sur 3. Erreur absolue moyenne :
  0.14 g avec le k d'avant, 0.07 g avec la moyenne, soit un gain de 0.08 g
  par refill.
- k a monté de 1.048 à 1.057 en une heure (usure du tube) : le k d'avant est
  en retard d'un demi-refill, d'où son léger défaut. Sur le 3e refill, k
  n'a presque pas bougé et le k d'avant était le meilleur.
- Bilan du run : demandé 831.3 mL, délivré selon le daemon 831.3 mL (écart
  +0.06 mL, 0.007 %), arrivé dans le récipient 832.3 g (dont l'égouttage
  après le Stop).

## 4. Décision

Critère non atteint (2/3, gain 0.08 g au lieu de 0.2 g) : **pas de
correction après coup**. L'estimation actuelle est déjà à quelques dixièmes
de gramme par refill, de l'ordre de la résolution de la balance. Les deux
scripts restent dans `scripts/` pour refaire le test si le montage change
(autre tube, débit très différent).

## 5. Démarrage sur le k appris (`feat/seed-from-last-k`, fusionnée le 17)

Vu pendant le test : la calibration 17 (27 octobre) donnait 4.8 % de moins
que la pompe réelle. Les 2 min avant le premier ratio ont donc délivré
0.9 mL de trop, rattrapés ensuite.

Changement : chaque run régulé enregistre son k stabilisé par calibration de
tube (`app_state`, clé `trim_learned_k`, au plus une fois par minute, jamais
un k gelé pendant un refill, en alerte ou hors bornes). Le run suivant sur la
même calibration démarre à c = 1/k (`tracking::seed_from_learned`), sauf si
l'écart avec le c0 de la calibration dépasse 15 % (tube changé ou remonté :
±6 % entre deux montages sur ce banc). Journal : événement `trim_seed`, ligne
« Correction from the last run ».

Ce qui reste : le premier run sur un tube neuf démarre toujours sur le c0.

Le run 266 a tourné sur l'ancien daemon : aucun k n'est encore enregistré.
Le prochain run régulé sur la calibration 17 partira du c0 et enregistrera
son k ; c'est le suivant qui démarrera à environ x0.95.

## 6. Branches et banc, 17 novembre après-midi

- `feat/camera` (vue en direct, réglages, timelapses) fusionnée dans
  `steps-raw-audit` (merge du 16 terminé), avec « Show in folder » après
  « Take a picture », puis `feat/seed-from-last-k` par-dessus. Tests Rust
  (546 dans le daemon), `npm run check` et `npm test` passés, poussé,
  déployé. `npm run smoke` pas relancé. `steps-raw-audit` n'est pas encore
  dans `main`.
- `cargo test` sur tout le workspace échoue sur `fermentool-tauri` tant que
  `copy-sidecar.ps1` n'a pas mis `fermentool-camera` dans
  `src-tauri/binaries` : lancer `--exclude fermentool-tauri`, ou le script
  d'abord.
- Nouveau câble RS-485 : les pompes sont maintenant sur **COM10** (COM8
  avec l'ancien). Balances : COM7 (flacon), COM9 (réception, test seulement).
  Les deux pompes testées à la main (amorçage 2 s, Test du refill 3 s) : OK.
