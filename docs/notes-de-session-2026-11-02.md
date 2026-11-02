# Notes de session, 2 novembre 2026

Suite des notes du 26 au 28 octobre (`notes-de-session-2026-10-28.md`).
Les changements visibles sont dans `CHANGELOG.md` (section « Non publié »),
les règles de design dans `docs/DESIGN.md`.

Commité et poussé : `9abd538` (sources) et `1c3a8d8` (bundle). Le travail
des 26-28 octobre avait été commité le matin même (`8b102cb` à `55400d3`) :
la ligne « Rien n'est commité » des notes du 28 n'est plus vraie.

## 1. Pictogrammes : Solar

- Page de comparaison (artifact) : 29 bibliothèques sur les 31 pictogrammes
  de l'appli, avec un aperçu dans l'appli. Tracés tirés de l'API Iconify.
  Seules Lucide, Tabler, Phosphor, Carbon, Material Symbols, MDI et Fluent
  ont les 31.
- Hugeicons essayée et déployée, puis **Solar (linear) retenue**.
- Les tracés sont copiés dans `ui/src/components/Icon.svelte` (pas de paquet,
  marche hors ligne), mêmes noms qu'avant : aucun autre écran n'a changé.
  `eslint` interdit `{@html}` : chaque forme est un objet d'attributs SVG
  (`path` par défaut, `el: 'circle'` sinon).
- Le sélecteur de tube dessinait sa flèche et sa loupe à la main : il passe
  par `Icon` (`chevron-down`, `search`).
- Solar n'a ni main simple ni prise barrée : « By hand » utilise
  `user-hand-up`, Disconnect `unlink-minimalistic` (la prise branchée reste
  `plug-circle`).
- Licence CC BY 4.0 : ligne « Icons » dans Settings > About, à garder.
- La pompe, la bouteille, le triangle d'alarme et l'engrenage sont nos
  propres dessins : inchangés.

## 2. Étiquettes : rectangles aux coins arrondis

- Toute étiquette avec du texte (mot d'état du bandeau « FEEDING »,
  « choose », états des runs, tubes, badges de Settings > Pumps) prend
  `--radius-tag` (6 px) au lieu d'une pilule. 11 règles dans 7 fichiers.
- Restent ronds : points d'état, barres de progression, interrupteurs,
  boutons ronds à icône (thème, « ? »).

## 3. Audit du refill automatique

Commits `03e4119` (daemon, tests, CHANGELOG), `a1e93c1` (interface, check
du smoke test) et `fba71fa` (bundle). Huit bugs, chacun reproduit par un
test qui échouait avant la correction (détail dans `CHANGELOG.md`) :

1. Adresse de la pompe de refill changeable pendant un run : le Stop
   partait à la nouvelle adresse, l'ancienne pompe continuait (1,1 kg
   devenus 2,8 kg en essai). Verrouillée maintenant comme le modèle et la
   tête (`refill_change_refused`), champ grisé dans Settings > Pumps.
2. Bouteille soulevée de la balance : la lecture près de 0 g démarrait la
   pompe de refill. Un saut plus grand que `PERTURBATION_MIN_G` ne démarre
   plus rien.
3. Run arrêté en plein remplissage, Stop perdu : renvoyé au bout d'une
   minute, maintenant chaque seconde (`stop_owed` dans `refill_run_ended`).
4. Alarme « The refill pump does not stop » : le compte pouvait sauter
   selon qui retentait le Stop. Elle part une fois (`stop_failed_said`).
5. Pompe de refill qui ne démarre pas sur un refill dû : notifiée
   (`refill_start_failed`, famille `refill_start`) jusqu'au prochain
   démarrage.
6. « Refill now » après un démarrage raté : le Stop encore dû l'arrêtait,
   puis fausse alarme « dry ». `stop_owed` remis à zéro au démarrage.
7. « Refill now » après un remplissage coupé par la balance : un seul
   refill compté (`resume_fill` remis à zéro au démarrage).
8. Remplissage coupé faute de montée : la page « keeps going » ne sonne
   plus à côté de l'alarme (`clears` de `refill_rise`).

## 4. Déploiements

- Hugeicons, puis Solar : aucun run actif.
- Étiquettes : **run 249 « test const » arrêté** (Stop, à la demande),
  déploiement, smoke test 60/60.
- Tests : 241 côté interface, `npm run check` propre.
- Audit du refill : **run 251 « test const » arrêté** (Stop, à la demande),
  daemon compilé depuis une copie propre de `fba71fa` (sans les
  modifications locales en cours), smoke test 61/61.

## 5. Pas fait, à faire

- **`ui/src/App.svelte` : toujours à trancher** (voir notes du 28,
  section 12). La modification locale retire le lien direct
  `#settings/...` et le clic sur la barre de connexion vers Settings > Pumps.
  Elle reste hors des commits. Le bundle commité et le daemon déployé
  après l'audit du refill sont construits **avec** ces deux fonctions : le
  daemon correspond à `fba71fa`.
- Ajouter une icône : la prendre dans Solar (`linear`), jamais dans une
  autre bibliothèque.
- Refill, limites connues non corrigées :
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
