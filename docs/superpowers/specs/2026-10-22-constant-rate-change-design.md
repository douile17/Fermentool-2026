# Changer le débit d'un run constant en cours d'expérience

Date : 2026-10-22. Statut : à valider.

## Intention

Pendant une culture, l'opérateur observe quelque chose (un changement
morphologique, par exemple) et veut passer d'un débit constant à un autre
pour mieux conduire le réacteur, **sans arrêter le run**. Chaque changement
doit rester tracé : History doit montrer la consigne de départ, chaque
nouvelle consigne et quand elle a été demandée, et « Run again » doit
permettre de repartir de n'importe laquelle.

Aujourd'hui, la seule façon de changer le débit est d'arrêter le run et
d'en lancer un autre : l'expérience est coupée en deux runs, le volume et
la précision ne sont plus comptés d'un bout à l'autre.

## Objectifs

1. Sur un run **Constant** en cours (phase courbe ou phase de maintien
   après la fin de la courbe), l'opérateur saisit une nouvelle consigne
   dans l'unité du run (ml/min ou rpm). Elle s'applique tout de suite et
   reste jusqu'à la fin du run ou jusqu'au changement suivant.
2. La régulation par la balance continue normalement : le facteur de
   correction `c` est conservé, il s'applique à la nouvelle consigne.
3. Le changement est journalisé, notifié à la personne responsable, et
   reconstruit à l'identique après un crash (la reprise remet la pompe à
   la dernière consigne demandée, pas à celle de départ).
4. History : le graphique « Feed rate » montre la consigne en paliers, le
   volume demandé et la précision (verdict, écart cumulé) en tiennent
   compte, le journal dit chaque changement, les pastilles de l'en-tête
   disent que la consigne a changé.
5. Run again : sans changement, comme aujourd'hui. Avec au moins un
   changement, une question avant d'ouvrir New run : « Start from which
   rate? », chaque consigne avec son jour et son heure
   (« Mon 14:10, 5.000 ml/min, at start », « Mon 20:30, 3.000 ml/min »).
   La consigne choisie devient le débit constant du nouveau run.

## Non-objectifs

- Changer une courbe exponentielle, linéaire, sigmoïde, step ou custom en
  cours de run. Le bouton n'existe que pour un run Constant.
- Changer la durée du run, le sens de rotation, le tube ou la calibration
  en cours de run.
- Un changement progressif (rampe) vers la nouvelle consigne : le passage
  est immédiat.
- Annuler un changement : on en fait un autre, vers l'ancienne valeur.
- Les runs de calibration (mesures) : jamais concernés.

## Anti-objectifs (ce qui en ferait un échec même si ça marche)

- Une reprise après crash qui repart de la consigne de départ, ou à la
  mauvaise consigne : `value_at(elapsed)` doit rester pur, calculé depuis
  ce qui est enregistré en base.
- Un volume demandé, un verdict On / Off target ou un écart cumulé faux
  dans History parce qu'il ignore les changements.
- Un changement qui casse la régulation (sursaut de `c`, alarme
  `Saturated` ou `FeedStopped` déclenchée par le simple changement).
- Une consigne hors limites acceptée (au-delà de ce que le tube peut
  donner, plus de 350 rpm pour un run ml/min piloté en rpm, négative).
- Une trace incomplète : un changement qui n'apparaîtrait pas dans le
  journal, le graphique ou Run again.

## Contraintes (de l'existant)

- `CurveSpec::value_at(elapsed)` est la seule entrée et doit rester pure
  en temps écoulé (`crates/fermentool-curves`, CLAUDE.md « Curve engine »).
- La reprise après crash (`Engine::resume`) relit `run.curve` en base ; le
  rapport (`tracking_report`) intègre `run.curve` pour le volume demandé ;
  la régulation (`Tracker`) intègre `active.spec`.
- Un run ml/min avec une calibration en rpm est piloté en rpm
  (`drive_ml_per_rpm`) et refusé au-delà de 350 rpm : la même vérification
  s'applique à une nouvelle consigne.
- Le temps écoulé de référence est celui de `run_now()` (avec sa garde
  contre les sauts d'horloge, CLAUDE.md « Crash-resume clock anchor »).
- Le thread de contrôle sert toutes les commandes : le changement passe
  par une `Command` comme Start / Stop, sans attente bloquante.
- UI : texte en anglais, pas de tiret cadratin, maquette avant
  implémentation (`docs/DESIGN.md`), logique testable dans `ui/src/lib/`.
- Le CSV d'export et la fiche History lisent l'historique de la base.

## Ce qui est décidé

- **Portée** : runs Constant seulement (décision de l'utilisateur), c'est-à-
  dire `CurveParams::Constant` (la forme « Constant » de New run). Une
  linéaire de 9 à 9 n'en est pas une : pas de bouton.
- **Trace** : consigne de départ + chaque changement avec son moment
  (utilisateur).
- **Run again** : choix de la consigne de départ parmi toutes, avec jour
  et heure, seulement s'il y a eu au moins un changement (utilisateur).
- **Application** : immédiate, jusqu'à la fin ou au changement suivant ;
  la régulation continue ; journal et notification ; History et volume
  demandé en tiennent compte (proposé, validé par l'utilisateur).

## Proposition technique (laissée au constructeur dans le détail)

- **Stockage** : une table `setpoint_changes (id, run_id, elapsed_s, value,
  wall_time)` (migration `0014`). `elapsed_s` est le temps écoulé du run au
  moment du changement, pris sur `run_now()`.
- **Courbe effective** : une fonction pure qui, à partir de la courbe
  Constant de départ et de la liste des changements, construit une courbe
  `Step` (premier palier = consigne de départ, un palier par changement,
  même durée, mêmes bornes `clamp`). Ses paliers ont `at_seconds` =
  `elapsed_s` ; sa valeur de fin = dernière consigne, pour que la phase de
  maintien tienne la bonne valeur. `run.curve` en base reste la courbe de
  départ (Run again, pastilles) ; partout où l'on pilote ou compte (moteur,
  reprise, rapport, graphique History), on utilise la courbe effective.
- **Changement pendant la phase de maintien** (après `duration_s`) : la
  courbe effective doit être juste **à chaque instant**, avant comme après
  le changement. Piège : au-delà de sa durée, une courbe rend sa valeur de
  fin ; mettre simplement la nouvelle consigne en `end` la rendrait aussi
  pour le maintien d'avant le changement, et fausserait le volume demandé
  du rapport. Par exemple, porter la durée de la courbe effective au
  dernier changement (sans toucher `run.duration_s`, qui décide de
  `curve_done` et de « completed » contre « stopped »), ou toute autre
  construction qui garde `value_at` pur et juste partout. Test exigé :
  maintien de 3 h à 5 h, changement à 4 h, la consigne vaut l'ancienne à
  3 h 30 et la nouvelle à 4 h 30, et le volume demandé intègre les deux.
- **L'interface connaît les changements** : `GET /api/runs/:id` et le
  statut du run actif donnent la liste `setpoint_changes` (`elapsed_s`,
  `value`, `wall_time`) ; l'Overview, History et Run again construisent la
  courbe affichée et la question à partir d'elle (fonction pure dans
  `ui/src/lib/`, testée), jamais depuis `run.curve` seul.
- **Commande** : `POST /api/runs/active/setpoint { value }` →
  `Command::SetConstantRate`. Refusé (erreur structurée, `EngineError`
  avec `hint`) si : aucun run actif, run non Constant, run de calibration,
  valeur non finie ou ≤ 0, hors limites du tube / de la pompe, plus de
  350 rpm pour un run piloté en rpm.
- **Moteur** : remplace `active.spec` par la nouvelle courbe effective,
  enregistre la ligne, journalise `setpoint_change` (détail : « 5.000 ->
  3.000 ml/min »), applique la nouvelle consigne au prochain
  `apply_setpoint` (~150 ms). `c` et l'historique de régulation sont
  conservés.
- **Régulation** : le changement brusque de débit ne doit pas faire
  croire à une pompe défaillante. Les fenêtres de ratio (ml pesés / ml
  demandés) restent valides puisque le demandé suit la courbe effective ;
  à vérifier sur un banc d'essai (test) qu'aucune alarme ne part et que
  `c` ne saute pas.
- **Notification** : `setpoint_change` → message Info « Rate changed to
  3.000 ml/min ».
- **UI Overview** : sur un run Constant, une action « Change rate » près
  du débit actuel, qui ouvre une petite saisie (valeur, unité du run,
  valeur actuelle rappelée, bornes) et une confirmation.
- **UI History** : graphique en paliers (courbe effective), repère à
  chaque changement, ligne de journal « Rate changed, 5.000 → 3.000
  ml/min », pastilles : « Constant », « 5.000 ml/min, then 3.000, 4.000 »
  (ou « changed 2× ») ; Run again : la question décrite plus haut.
- **CSV** : la consigne effective par tick y est déjà (setpoint) ; ajouter
  les changements s'il y a une section événements.

## Laissé au constructeur

- Le détail de l'API (route exacte, forme de la réponse).
- La forme exacte de la saisie et de la question Run again, après la
  maquette validée par l'utilisateur.
- Le libellé des pastilles quand il y a beaucoup de changements.

## Mes choix (à vérifier en lisant)

- **Pas de note libre** à côté de la nouvelle valeur : seulement la
  valeur et le moment. La question a été posée sans réponse ; facile à
  ajouter plus tard (une colonne `note`).
- **Pas de nom de la personne** qui a fait le changement : l'app n'a pas
  de connexion utilisateur, le responsable du run est déjà connu.
- **Run again affiche jour et heure** (« Mon 20:30 ») dans la question,
  comme demandé, alors que les graphiques comptent en temps d'expérience :
  la question parle de moments réels de la semaine.
- **Une confirmation** avant d'appliquer le changement (comme Stop run) :
  une erreur de frappe changerait la culture.
