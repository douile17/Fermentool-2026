# Changelog

## Non publié (depuis 0.1.5)

### Autres pompes : LabV et PDS II (23 octobre)

- Settings › Pumps : chaque pompe se choisit parmi **LabQ, LabV et PDS II**
  (avec sa photo), avec son adresse et sa tête (1 à 8, PDS II seulement).
  Sa vitesse maximale est celle du modèle, fixe (LabQ 350, LabV 150, PDS II
  600 tr/min) : plus de champ « Max rpm ». Le câble est affiché
  « 9600 baud · 8E1 » (le choix du baud disparaît).
- LabV et PDS II sont pilotés au **0,01 tr/min** et au **0,01 ml/min** près
  (vérifié sur les pompes du banc), au lieu du 0,1 tr/min de la LabQ, qui ne
  change pas. Les champs de New run et de la vitesse de remplissage suivent
  ce pas.
- Une vitesse au-dessus du maximum de la pompe est refusée (courbe, remplissage) :
  une courbe en tr/min qui le dépasse est refusée au démarrage, New run le dit
  avant Start. Seule la régulation peut demander plus que le maximum : la
  pompe reste alors à son maximum.
- LabV et PDS II n'acceptent pas moins de 0,1 ml/min : sous ce seuil (une
  marche à 0), la pompe est arrêtée, puis redémarrée quand la courbe remonte.
- Un run interrompu ne reprend que sur la pompe qu'il pilotait (même modèle,
  même tête d'un PDS II) ; sinon la reprise est refusée et dit pourquoi.
  Settings › Pumps verrouille la pompe d'alimentation tant qu'un run attend
  sa reprise.
- Une calibration prend le modèle de pompe de ses trois mesures, et non plus celui
  de la configuration au moment de l'enregistrement.
- Un `config.toml` corrigé à la main ne change plus jamais le modèle d'une
  pompe : seul le champ hors limites revient à sa valeur par défaut.
- Une pompe laissée dans un autre mode que « transferring » est remise en
  transferring au démarrage, à la reprise, au début d'un remplissage et au
  Test (jamais pendant le réglage de la vitesse).
- Le tuyau calibré appartient au modèle de pompe sur lequel il a été mesuré :
  un nouveau run ne propose que les calibrations du modèle de la pompe
  d'alimentation. Les calibrations existantes sont des LabQ. Chaque run
  retient le modèle et la tête de sa pompe.
- Alimentation et remplissage peuvent être deux têtes d'un même PDS II (même
  adresse, têtes différentes).
- Les protocoles LabV et PDS II sont dans le dépôt ; `docs/wiring.md` donne le
  DB9 du LabV (broche 7 = masse du RS-485, 8 = B, 9 = A).

**À vérifier sur le matériel** (une LabV, puis un PDS II ; rien n'a encore été
essayé sur une vraie pompe autre que la LabQ) :

1. Settings › Pumps : la pompe est branchée, réglée à 9600 et à son adresse,
   et affiche « answers ». LabV : la parité est paire (9600 8E1) ; si elle ne
   répond pas, regarder d'abord ce réglage dans son menu.
2. Un court run en tr/min démarre, s'arrête, et tourne dans le sens choisi.
3. Une pompe laissée dans un autre mode (dosage, volume fixe) est remise en
   transferring toute seule au démarrage du run.
4. Une vitesse au-dessus du maximum réglé est refusée avec un message qui le
   dit.
5. PDS II : une tête réglée en « logical operation » est refusée avec
   l'indication de la passer en independent operation ; deux têtes d'un même
   contrôleur (alimentation et remplissage) tournent sans que l'une
   démarre ou arrête l'autre.
6. Une calibration de tuyau faite sur la LabQ n'est pas proposée pour la
   LabV ni le PDS II.
7. Débit plancher en ml/min sans calibration de tuyau en rpm : la doc donne
   0,1 ml/min pour le registre de débit (LabV 1004, PDS II n030), et
   Fermentool arrête la pompe en dessous. Essayer 0,05 ml/min sur la pompe :
   si elle l'accepte, abaisser ce plancher (`PumpModel::flow_min`). Avec un
   tuyau calibré en rpm, ce plancher ne s'applique pas (piloté en rpm).

### History, calibration, remplissage (20 et 21 octobre)

- Fiche d'un run dans History refaite : le nom et les dates d'abord, puis le
  verdict (On / Off target à ±2 %, livré, % du demandé, remplissages,
  alarmes), le débit sur un axe en heures, l'écart cumulé au demandé, un
  tableau du flacon, et repliés les réglages, la régulation et un journal en
  mots. Pendant un remplissage, non pesé, les courbes passent en pointillés
  gris.
- Courbe Step : la fin s'affiche en marche, plus en pente. L'Overview ne
  montre plus un débit qui descend alors que la pompe tourne encore.
- Le µ du débit (« µ asked / µ delivered ») n'est plus affiché ni calculé :
  il se lisait comme un taux de croissance de la biomasse. Le µ saisi en mode
  Physio reste.
- Calibration du tuyau : « measurements » au lieu de « bursts », chaque
  mesure s'ouvre (poids, débit, pente, courbe de la balance), « Redo » sur
  chaque carte à tout moment, « Redo all 3 » qui garde le tube et le réglage,
  pesée sur la moyenne des lectures stables (5 s) au lieu d'une seule.
- Le menu s'appelle « Feeding profile ».
- L'étiquette « refill » du graphique ne s'écrit plus sur un autre texte ni
  sur la courbe.
- Plus d'heure « Empty around » ni de « runs dry » : la balance pèse aussi le
  flacon et ses tuyaux, l'heure était fausse.
- Une vitesse de pompe de remplissage changée pendant un remplissage
  s'applique aussitôt (avant : au remplissage suivant).
- Settings › About : « Open folder » / « Show in folder » ouvrent le dossier
  des données, des logs, ou le dossier de l'historique et des réglages avec le
  fichier sélectionné, dans l'Explorateur (au lieu de « Copy »). Le dossier
  des données est écrit une fois, son contenu en liste dessous.
- Settings › **Pumps** remplace « Pump » et « Refill pump » : le câble (port,
  baud, Connect, ou Disconnect en rouge une fois connecté) et la liste des pompes branchées dessus, chacune avec son
  picto, son adresse MODBUS et son état (« answers », ou « Not answering » en
  rouge avec quoi vérifier). Les réglages de la pompe de remplissage
  s'ouvrent par « Change ». Un seul Save, qui ne reconnecte que si le port, le
  baud ou l'adresse de la pompe d'alimentation change.
- La barre de connexion en haut devient une ligne fine, seulement en cas de
  problème, avec un lien vers Settings › Pumps (le choix du port est là-bas).
- Tout Settings allégé (audit UX du 22 octobre) : plus de surtitre ni de
  phrase d'introduction, les explications iront dans la documentation
  technique. Balance : Connect / Disconnect comme les pompes, une ligne
  d'état (« COM7 connected: 4349.6 g, stable. »), Save grisé tant que rien
  ne change. Crash resume : un seul interrupteur, appliqué au clic ; le délai de
  reprise d'une mesure de calibration n'est plus que dans `config.toml`
  (`[resume] grace_minutes`). Display : le choix s'applique au clic, sans Save. Daemon : « Shut
  down daemon » en contour rouge, le rouge plein reste pour la confirmation.
  Notifications : par personne, le nom, le topic en entier, « Generate » puis
  « QR », « Test », et le reste (nouveau topic, Test alarm, Teams, Remove en
  rouge) dans un menu « … ».
- Pumps et Balance disent leur état d'abord, en grand, dans un bandeau
  teinté (« Connected » en vert avec une prise, « Disconnected » en gris,
  « Not answering » en rouge), le détail dessous (port, pompes qui
  répondent, poids), Connect ou Disconnect à droite ; le port et le baud
  passent en dessous. Disconnect est un bouton sans cadre, rouge sombre
  avec une icône « débrancher ».
- Câble des pompes rebranché : les deux pompes reviennent ensemble (avant,
  « 1 pump of 2 answers » jusqu'à 15 s). La pompe de remplissage est
  réinterrogée dès que la pompe d'alimentation répond, puis toutes les 3 s
  tant qu'elle ne répond pas, et plus du tout pendant un run.
- Courbe finie, refill automatique en cours : le bandeau de l'Overview dit
  le prochain refill (« Next refill, around 16:40, in 2 h 10 »), le rappel
  de Stop passe sous le titre, et le graphique marque le refill sur son
  bord droit avec une flèche (« refill 16:40, in 2 h 10 → »). Avant, le
  prochain refill n'était dit nulle part après la fin de la courbe.
- Tous les graphiques (Overview, New run, fiche d'un run dans History) ont
  une échelle en temps d'expérience : en minutes jusqu'à 2 h, en heures
  au-delà, le pas suivant la durée et la largeur du graphique. Jamais
  l'heure du jour.
- Fiche d'un run : le tableau du flacon (« Refill 2 at 2 h 21, 32 min »),
  la régulation et le journal comptent aussi depuis le début du run ;
  seule la ligne d'en-tête garde la date et l'heure de début et de fin.
- Fiche d'un run : la courbe est dite dans l'en-tête, en pastilles (forme,
  début → fin, durée, sens), au lieu d'être cachée dans Settings.
- Un remplissage coupé par une balance muette puis repris compte pour un
  seul, partout : tableau du flacon, nombre de refills, journal de la fiche,
  y compris pour les runs passés (run 210 : un remplissage de 32 min, et
  non « Refill 1 » après « Refill 2 »). La notification « Bottle refilled »
  ne part plus au moment de la coupure, seulement à la fin.
- Légende des graphiques de la fiche : « Refill or bump, not weighed »
  quand des à-coups de la balance s'ajoutent aux remplissages, « Not
  weighed » sans remplissage.
- Fiche d'un run : Settings ouvert et en premier, le tableau du flacon
  replié dessous (« 1 refill, 1 270.8 g poured »).
- Liens directs vers une section : `#settings/<id>` (par ex.
  `#settings/balance`) ouvre l'app sur cette section.
- Balance plus tolérante à un PC chargé : une demande de poids peut attendre
  0,8 s (avant 0,2 s), et un remplissage n'est coupé qu'après 3 s sans
  lecture. Un remplissage du run 210 avait été coupé par une seule lecture
  manquée pendant une compilation.

### État du run et interface (19 et 20 octobre)

- Bandeau de l'Overview coloré selon l'état, seulement quand il faut y
  regarder : bleu pendant un remplissage, orange pour un remplissage arrêté ou
  un flacon qui ne se remplit pas, rouge pour une alarme ; la marche normale
  reste blanche. Grand picto animé (pompe qui tourne dans le sens réel du run,
  flacon qui se remplit), étiquette d'état, barre de progression du
  remplissage avec montée en g/min et temps restant.
- Sur History, Tubing calibration et Settings, une barre fine en haut rappelle
  l'état du run, avec « Open Overview ». Le titre de la fenêtre et l'icône
  d'onglet suivent l'état.
- Graphiques depuis 0 avec un axe gradué : un écart de 1 % ne remplit plus
  toute la hauteur. Les remplissages sont en bandes bleues, en direct et dans
  History.
- Overview : un seul débit affiché, « Next refill around … », Delivery en
  « 100.0 % delivered · +0.0 % vs asked ». « Feed bottle » ne baisse plus
  pendant un remplissage (livré estimé par le daemon, marqué « estimated while
  refilling »).
- History : numéro du run, durée réellement tournée (« 49 m of 8 h »),
  « stopped » en gris neutre, « Reset history… » en lien discret sous la liste.
- Contrastes conformes WCAG AA en clair et en sombre (texte lisible sur les
  boutons du thème sombre), dates en anglais quel que soit le PC, titres
  d'état en Barlow.

### Remplissage automatique jugé sur la balance

- Plus de débit attendu pour la pompe de remplissage (non calibrée) : un
  remplissage est jugé sur la montée du poids. Une pompe lente (tuyau fin,
  vitesse basse) remplit jusqu'au niveau haut sans fausse alarme ; avant, elle
  était coupée en « no flow » (runs 186 et 192).
- Si le poids ne monte pas pendant 30 s : alarme « Refill: the bottle is not
  filling » sur le téléphone, la pompe continue (amorçage) ; message quand le
  poids remonte. Seuls le niveau haut, le dépassement de 50 g et un Stop non
  confirmé arrêtent la pompe de remplissage.
- Le journal note la vitesse et la montée nette de chaque remplissage.
  `expected_ml_min` est retiré de la configuration (ignoré s'il reste).

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

### Remplissage de la bouteille pesée

- Un remplissage par pompe de transfert (poids qui monte régulièrement, sans
  saut de 50 g) est reconnu : plus de 50 g gagnés en 30 s suffisent. Les
  lectures prises pour du débit avant la détection sont annulées.
- La fin d'un remplissage attend un poids immobile pendant 20 s (débit
  d'alimentation déduit), puis 10 s, au lieu de 10 s fixes ; un nouveau
  versement pendant ce délai prolonge le remplissage.
- La régulation traverse le remplissage sans repartir de zéro : l'historique
  du rendement est gardé, c reprend sur la fenêtre longue. Seul un
  remplissage après une alarme « Feed stopped » efface l'historique (la
  bouteille sèche).
- Bouteille soulevée puis reposée sans gain de poids : pas un remplissage, le
  volume délivré pendant ce temps est mesuré.
- Boutons « Refill bottle » / « Refill done » sur la page du run. Annoncé à la
  main, le remplissage attend le versement ; sans versement au bout de 15 min,
  la demande tombe (`refill_cancelled`).
- Chaque alarme, reprise et remplissage est écrit dans le journal du run et
  marqué sur le graphique.

### Calibration du tuyau

- Calibration automatique avec la bouteille d'alimentation sur la balance :
  les 3 essais démarrent, s'arrêtent et sont pesés seuls (balance stable 5 s
  avant et après chaque essai) ; arrêt avec un message si la bouteille n'a
  presque pas perdu de poids. En manuel, « Take balance reading » propose le
  poids lu.
- Un run en ml/min peut être piloté en rpm à partir d'une calibration en rpm
  (New run, « Drive in rpm from the tube's calibration »), avec ou sans
  balance : la courbe reste en ml/min, la table tête/tuyau de la pompe ne
  sert plus. Refusé si la courbe demande plus de 350 rpm.

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

### Robustesse

- Balance éteinte ou pompe muette : les pages History et Calibration ne
  rament plus. L'historique, les calibrations et le suivi sont lus sur une
  connexion à part, sans attendre le fil qui pilote la pompe et la balance.
- La balance est lue par son propre fil : une balance qui ne répond pas ne
  retient plus jamais le pilotage de la pompe (chaque lecture ratée le
  bloquait 1,5 s).
- Balance éteinte : plus de « balance lost » / « balance answering again »
  toutes les 20 s dans le journal. Une seule perte par panne, le retour est
  annoncé au premier poids lu, pas à la simple réouverture du port COM. Le
  port d'une balance seulement éteinte est gardé, il n'est rouvert que s'il
  est vraiment cassé (adaptateur USB débranché).
- Pompe perdue au repos : plus de sonde chaque seconde en plus des tentatives
  de reconnexion.

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
