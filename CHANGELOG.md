# Changelog

## Non publié (depuis 0.1.5)

### Régulation par la balance

- La correction agit dès les premières secondes, autour du facteur de la
  calibration du tuyau, sans attendre la mesure du rendement de la pompe. Un
  écart net au démarrage (plus de 1 g et 5 %, tuyau déplacé, mauvaise
  calibration) est corrigé à pleine vitesse.
- Moins nerveuse ensuite : rendement mesuré sur 5 g pour démarrer, puis sur
  20 g (au moins 5 min) avec des pas de 0,5 % au lieu de 2 % ; fini les
  oscillations de ±10 % vues à bas débit.
- Limite de correction réglable (Settings, Balance, « Correction limit »,
  ±25 % par défaut), modifiable pendant un run.
- Un choc sur la balance (bouteille ou tuyau touché) n'est plus pris pour du
  débit : le seuil suit le débit (1 g à 2 ml/min, 5 g dès 60 ml/min).
- Balance sous la bouteille ou sous le récipient receveur (Settings, Balance,
  « Balance weighs ») ; une balance du mauvais côté fige la correction au lieu
  de la pousser à +25 %.

### Alarmes

- « Feed stopped : bottle empty or line blocked » quand la pompe tourne mais
  que le poids ne bouge plus pendant 3 min (testé sur le run 48 : 3,5 min au
  lieu de 9).
- À toute alarme, la correction revient à sa valeur d'avant le problème et se
  fige, au lieu de rester en butée (sinon +36 % de débit après un remplissage).
- Reprise automatique dès que le débit redevient normal. Le volume manqué
  pendant l'arrêt reste dans les totaux mais n'est pas rattrapé d'un coup.

- Chaque alarme, reprise et remplissage est écrit dans le journal du run et
  marqué sur le graphique.

### Fin de courbe

- Un run de dosage ne s'arrête plus à la fin de sa courbe : il garde la valeur
  finale, régulé et enregistré chaque seconde, jusqu'à Stop (enregistré
  « completed »). La fenêtre de fin de courbe informe seulement. Une reprise
  après plantage repart dans ce maintien.

### Suivi et export

- Une seule ligne de synthèse : délivré pesé, demandé, %, écart, dans ou hors
  de ±2 %. Le volume n'est plus répété ailleurs ; sans balance, une seule
  mention « Volume added (est.) ».
- Courbe pesée en orange, graphique de l'écart avec une bande de ±2 %,
  marqueurs numérotés (début de correction, rendement mesuré, alarmes,
  remplissages).
- Poids de la balance au début, avant et après chaque remplissage, à la fin,
  et poids sorti total, à l'écran et dans le CSV.
- CSV : bloc de synthèse en tête (responsable, poids, volumes), colonnes
  `balance_g` et `delivered_g` à chaque seconde.

### Notifications

- Chaque utilisateur reçoit les alertes de ses runs sur son téléphone avec
  l'appli gratuite ntfy (sujet privé, abonnement par QR code sur Android).
  Les alarmes sonnent, les infos restent discrètes. Teams reste possible.
- Le lancement d'un run demande obligatoirement le responsable dès qu'une
  personne est enregistrée ; ses alertes ne vont qu'à lui.
- Settings, Notifications : enregistrement automatique, bouton Test.
- Une alarme revient toutes les 3 min (« Reminder 2: … ») tant que personne
  ne l'a acquittée : bouton « Acknowledge » sur la notification ntfy, ou
  bandeau rouge en haut de l'interface. Elle s'arrête aussi quand le problème
  disparaît (débit revenu, balance ou pompe qui répond) ou au Stop. Rien à
  régler sur le téléphone ; chaque acquittement est écrit dans le journal du
  run (`alarm_ack`, avec qui l'a fait). Bouton « Test alarm » dans Settings
  (toutes les minutes, 5 fois au plus).

### Calibration des tuyaux

- Archiver une calibration (elle disparaît de Nouveau run, reste dans
  l'historique), la restaurer.
- « Discard » arrête la pompe d'un essai en cours ; un essai orphelin peut être
  arrêté ; un poids mal saisi peut être corrigé sans repomper.

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
