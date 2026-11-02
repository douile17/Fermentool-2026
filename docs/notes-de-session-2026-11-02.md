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

## 3. Déploiements

- Hugeicons, puis Solar : aucun run actif.
- Étiquettes : **run 249 « test const » arrêté** (Stop, à la demande),
  déploiement, smoke test 60/60.
- Tests : 241 côté interface, `npm run check` propre.

## 4. Pas fait, à faire

- **`ui/src/App.svelte` : toujours à trancher** (voir notes du 28,
  section 12). La modification locale retire le lien direct
  `#settings/...` et le clic sur la barre de connexion vers Settings > Pumps.
  Elle reste hors des commits. Le bundle commité est construit **avec** ces
  deux fonctions, mais **le daemon déployé aujourd'hui a été compilé sans**
  (les builds de la matinée partaient de la copie locale). Si la
  modification est annulée : redéployer après le run en cours, pour que le
  daemon corresponde à GitHub.
- Ajouter une icône : la prendre dans Solar (`linear`), jamais dans une
  autre bibliothèque.
