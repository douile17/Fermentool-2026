# Notes de session : 3 au 6 novembre 2026

Suite des notes du 2 novembre. Objectif fixé par l'utilisateur : « ça ne
doit arriver dans aucun cas de figure, pas de montée ou de baisse dans
aucun cas ». Une manipulation de la bouteille, ou toute perturbation, ne
doit jamais faire bouger le facteur de correction c.

## 1. Bouteille manipulée : c ne bouge plus (3 novembre)

Constat (run 256) : un « Refill now » sur un réservoir vide. La ligne a
poussé ses derniers 1,4 g, puis plus rien, et c est monté de 0,963 à
1,013, puis redescendu en 10 minutes. Les grammes qui ne venaient pas de
la pompe d'alimentation passaient pour du débit dès qu'ils restaient sous
la tolérance d'une perturbation (~1 g).

Correction (`settle_anchor`, `Tracker::settling`) : toute période hors de
l'état `Normal` compte `k_est` x commandé, jamais la pesée. Cela couvre la
bouteille touchée ou soulevée, le refill, un remplissage qui n'apporte
rien et une balance muette `REFILL_BLIND_S`. Les `REFILL_SETTLE_S` (3 min)
qui suivent sont comptées de même et journalisées comme estimées. Les
points du ratio sont ensuite effacés, et c reste sur `k` en attendant.
Seule une reprise après crash reste pesée, parce que la pompe a pu
s'arrêter pendant la panne. Ces 3 minutes apparaissent en pointillé gris
sur les graphiques, comme le refill (`Chart`, `GapChart`, `chartscale`).

Les tests comparent c au même run laissé tranquille (`c_moved`, ±0,004).

## 2. Ce que la balance ne voit pas comme une manipulation (6 novembre)

Il restait deux cas que la section 1 ne couvre pas, parce que la balance
les lit comme du débit normal :

- un poids sous le seuil d'un contact (1 g) laissé sur le plateau :
  c +2,7 % pour 0,8 g ;
- une ligne de refill qui goutte dans la bouteille : c +15 % à 0,02 g/s.

Le ratio de Theil-Sen résiste à un point aberrant isolé, pas à un
décalage de niveau qui reste, ni à une pente ajoutée.

Fait mesuré sur les vrais runs (192, 210, 251) : le ratio de la pompe
dérive de 0,1 à 0,4 % par heure (usure du tuyau), jamais plus.

Correction (`tracking.rs`, `engine::scale_tick`) :

- **`k_ref`** : une fois la fenêtre settled atteinte, c régule sur `k_ref`,
  qui suit le ratio mesuré à 1 %/h au plus (`K_DRIFT_PER_S`,
  `drift_toward`).
- **Pesée en retard** : le déficit pesé n'atteint c qu'avec 3 min de
  retard (`WEIGH_DELAY_S`, `deficit_weighed_until`). Les dernières minutes
  comptent `k_ref` x commandé.
- **Écart** : un ratio mesuré à plus de 1 % de `k_ref` (`K_JUMP_TOLERANCE`,
  `ratio_jumped`) est un écart, `k_jump`, daté d'une fenêtre settled en
  arrière. c tient, et la période compte `k_ref`. À sa fin, elle est
  passée en perte dans `discounted_g` (pas `forgiven_g`, que l'UI affiche
  comme « missed while the feed was stopped »).
- **Alarme `RatioJump`** (`alarm_ratio_jump`, « Pump ratio moved too
  fast ») : un écart tenu 15 min (`K_JUMP_ALARM_S`). Elle est notifiée,
  famille « trim », et c revient à sa valeur d'avant le début. Elle se
  lève quand `recent_ratio` revient près de `k_ref`.
- **Changement de débit** de plus de 10 % (`K_RATE_CHANGE`) : `k_ref` est
  réappris une fois qu'une fenêtre settled a tourné à la nouvelle vitesse
  (`k_learn_t`). Le ratio mesuré régule en attendant.
- **Seuil de précision** (`ratio_precise`, `K_REF_MIN_STEPS`) : il faut
  que 1 % d'écart fasse 2 pas de balance en 5 min, soit environ 4 ml/min
  avec la balance à 0,1 g. En dessous, la régulation reste celle d'avant.
  Sans ce seuil, à 0,05 ml/min, `k_ref` était adopté à 0,903 pour un vrai
  0,95, d'où une fausse alarme.

Deux bugs trouvés en mesurant, chacun corrigé avec son test :

- **Siphon pile à la tolérance** (0,002 g/s) : l'écart s'allumait et
  s'éteignait, et chaque fin d'écart passait de nouveau en perte les mêmes
  5 minutes. Résultat : 24 g passés en perte pour 2,4 g de siphon, c
  −9,6 %. Corrigé par `discounted_until_t` : rien n'est passé en perte
  avant la fin de la précédente perte.
- **Siphon de 0,05 g/s** : l'alarme `Saturated` se levait alors que le
  ratio récent valait 0,877, dans les bornes mais encore marqué par la fin
  du siphon. `k_ref` était alors abandonné, c +5,9 %. Corrigé : avec un
  `k_ref`, toute alarme du trim ne se lève qu'une fois `recent_ratio`
  revenu près de lui.

Résultat (écart de c au même run laissé tranquille) :

| Perturbation | Avant | Après |
|---|---|---|
| Poids de 0,3 à 1,2 g resté sur le plateau | 1,2 à 3,9 % | 0,18 à 0,33 % |
| Poids de 0,8 g retiré | | 0,40 % |
| Siphon de 0,005 à 0,02 g/s | 3,7 à 15 % | 0,20 à 0,41 % |
| Siphon de 0,002 g/s (à la tolérance) | | 0,42 % |
| Siphon de 0,05 g/s | 23 % | < 0,4 % |
| Tuyau qui s'use de 0,5 %/h sur 3 h | | suivi, c à 0,003 de 1/k |

Tests ajoutés : `a_weight_too_small_to_read_as_a_touch_does_not_move_c`,
`a_line_seeping_into_the_bottle_does_not_move_c_and_is_said`,
`tubing_that_wears_is_still_followed`,
`a_seep_at_the_tolerance_is_written_off_once`,
`a_seep_past_the_correction_limit_clears_on_the_ratio_from_before`, plus
deux tests unitaires dans `tracking.rs`. Côté UI : `ratio_jump` a sa
propre bannière, et le test de `runview` vérifie qu'elle ne s'affiche pas
comme « out of bounds ».

Une borne de test élargie : `the_regulation_rides_a_change`, de 0,5 à
0,6 %. Sans le mécanisme, le test donnait 0,468 %, et 0,57 % avec. Le
ratio réel du banc diffère de 0,25 % entre les deux débits, et à 3 ml/min
un pas de balance vaut 0,33 % de c.

Limites connues :

- Un tuyau changé ou déplacé volontairement en cours de run donne
  l'alarme, et c reste à l'ancien ratio tant qu'elle dure.
- Une pompe dont le ratio change pour de bon reste dans son alarme.
- Sous ~4 ml/min (balance à 0,1 g), un poids laissé ou un siphon fait
  encore bouger c.

## 3. Déploiement (6 novembre)

- **Run 257 « test const » arrêté** (Stop, à la demande) à 3 h 05 sur
  5 h, à 8,03 ml/min.
- Daemon compilé depuis **la copie de travail**, rien n'étant commité :
  les corrections du 3 et du 6 novembre, **avec aussi la modification
  locale d'`App.svelte`** (voir section 4). Le daemon déployé ne
  correspond donc à aucun commit.
- Rust : 448/448 (`fermentool-core`), lancé en basse priorité pendant le
  run 257. Interface : 244 tests, `npm run check` propre, smoke test 61/61,
  aucune instance Edge orpheline.

## 4. Pas fait, à faire

- **Commiter** les corrections du 3 et du 6 novembre (rien n'est commité).
- **À vérifier sur un vrai run à plus de 4 ml/min** : pas de fausse
  alarme « Pump ratio moved too fast » sur plusieurs heures, et le ratio
  qui suit l'usure. Vérifier aussi qu'il n'y a pas de pic de c au
  prochain refill (reste ouvert depuis les notes du 2, section 3 bis).
- **`ui/src/App.svelte` : toujours à trancher** (notes du 28, section 12).
  La modification locale retire le lien direct `#settings/...` et le clic
  sur la barre de connexion vers Settings > Pumps. Elle est maintenant
  **dans le daemon déployé**. Il faut la séparer au moment des commits :
  la bannière `ratio_jump` ajoutée dans le même fichier doit partir dans
  le commit du 6 novembre.
- Ajouter une icône : la prendre dans Solar (`linear`), jamais dans une
  autre bibliothèque.
- Refill, limites connues non corrigées (reprises des notes du 2) :
  - une bouteille tenue hors de la balance plus de 30 s peut encore lancer
    un remplissage ;
  - un Stop dû à une pompe muette est retenté chaque seconde, et chaque
    essai raté occupe la ligne le temps de son timeout ;
  - un crash en plein remplissage perd `refill_before_g` et `resume_fill`
    (pas vérifié par un test) ;
  - une perturbation pendant un remplissage peut l'arrêter trop tôt ou
    lever une fausse alarme « dry » ;
  - le cycle compte en heure murale brute, pas avec l'horloge gardée de
    `run_now()`.
