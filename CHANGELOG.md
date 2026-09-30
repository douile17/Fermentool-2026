# Changelog

## 0.1.5 (2026-09-30)

- Balance affichée « disconnected » pour de bon après quelques lectures
  ratées, même au repos et bien branchée : la reconnexion rouvrait le port COM
  alors que l'ancienne connexion le tenait encore, ce que Windows refuse. Elle
  libère maintenant l'ancienne connexion d'abord et revient en quelques
  secondes. Les lectures ratées sont écrites dans les journaux avec leur cause.
- « Run again » depuis l'historique reprend aussi le trim gravimétrique et la
  calibration du tuyau du run d'origine.

## 0.1.4 (2026-09-29)

Remplace les builds 0.2.0 et 0.2.1, retirés (installateur de 220 Mo inutile).

### Trim gravimétrique

- Une balance sous le flacon d'alimentation (Ohaus Ranger 7000, protocole
  MT-SICS) corrige la consigne de la pompe pendant le run, en option par run
  (« Enable gravimetric trim »).
- **Suivi du cumulé** : le daemon compare en continu la masse demandée par la
  courbe, la masse commandée à la pompe et la masse réellement sortie du
  flacon. Toutes les 10 s, la correction c vaut 1/k (k = rendement réel de la
  pompe, mesuré) plus un terme qui rattrape le retard cumulé en 10 min. Le
  volume délivré suit ainsi la courbe, quelle que soit sa forme (linéaire,
  exponentielle, sigmoïde, constante, palier).
- Garde-fous : c entre 0,80 et 1,25, au plus 2 % par mise à jour ; compte
  conservé à travers les manipulations du flacon, les recharges et un crash ;
  alarme si la pompe reste hors de portée de correction 5 min.
- Graphique « délivré (pesé) vs demandé » avec R², écart cumulé, et µ demandé
  vs µ obtenu pour une exponentielle (Overview pendant le run, History ensuite).
- Balance réglable depuis l'interface : Settings, carte « Balance » (port COM
  détecté, baud, densité du liquide), appliqué sans redémarrer ; refusé pendant
  un run avec trim gravimétrique.
- Poids en direct dans la barre latérale (« Balance · 742.5 g · stable »),
  détection d'une balance débranchée et reconnexion automatique.

### Calibration des tuyaux

- Onglet « Tubing calibration » : 3 salves à une consigne, pesées à la main ;
  débit, CV et c₀ calculés côté daemon ; tuyau identifié par lot, référence
  interne, Ø intérieur et Ø extérieur ; export CSV ; session conservée en cas
  de rafraîchissement ou de redémarrage.
- Obligatoire pour un trim en rpm, optionnelle en ml/min. En ml/min, c₀ sert de
  point de départ du trim ; en rpm, la calibration fournit la conversion
  rpm vers ml/min.

### Installation

- Installateur autonome pour un PC neuf (environ 5 Mo) : runtime Visual C++
  lié en statique ; WebView2, déjà présent sur Windows 10 et 11, téléchargé
  seulement s'il manque.
- Une mise à jour arrête proprement le daemon avant de remplacer les fichiers,
  et refuse de s'installer pendant un run.
- Procédure d'export : `docs/RELEASE.md`. Plan de test :
  `docs/test-plan-gravimetric-trim.md`.
