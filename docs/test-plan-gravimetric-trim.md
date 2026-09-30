# Plan de test : trim gravimétrique et calibration des tuyaux

Version testée : `Fermentool_0.1.3_x64-setup.exe`, construit depuis `main` (`d31f4db`).

Objectif : vérifier sur le vrai matériel (pompe LabQ, balance Ohaus Ranger 7000) que le volume réellement délivré suit la courbe demandée, avec un R² proche de 1, et que les garde-fous (recharge, perturbation, alarme, reprise après crash) se comportent comme prévu.

## Matériel

- PC de manip avec l'installateur, pompe LabQ et son adaptateur USB-RS485, balance Ranger 7000 (câble série ou USB).
- Un flacon d'alimentation (eau, ou milieu dilué : densité ≈ 1,00 g/mL) posé sur la balance.
- Un bécher de récupération en sortie de pompe, **hors de la balance**.
- Un tuyau neuf ou identifié (lot, Ø intérieur, Ø extérieur).
- Une pince (pour simuler une ligne bouchée), un chronomètre.
- Une seconde balance, si possible, pour peser le bécher en fin de test (contrôle indépendant).

## Rappels utiles

- La balance est lue une fois par seconde. La correction c est recalculée toutes les 10 s.
- **Le trim ne corrige rien pendant les premières minutes** : il attend d'avoir vu passer au moins 200 fois la résolution de la balance (20 g pour une balance au 0,1 g) et au moins 2 min. À 10 ml/min, c'est environ 2 min ; à 1 ml/min, environ 20 min. Pour des tests courts, utiliser des débits de 5 à 10 ml/min.
- c reste entre 0,80 et 1,25 et varie d'au plus 2 % par mise à jour.
- L'alarme « trim alarm » se déclenche seulement si la pompe reste hors de portée de correction pendant 5 min. Elle gèle c jusqu'au run suivant.

---

## 1. Installation

| # | Étape | Attendu | OK ? |
|---|---|---|---|
| 1.1 | Lancer `Fermentool_0.1.3_x64-setup.exe`. Si SmartScreen s'affiche : « Informations complémentaires », puis « Exécuter quand même ». | L'installation se termine, Fermentool s'ouvre. | |
| 1.2 | Ouvrir le Planificateur de tâches. | Une tâche `Fermentool` existe (démarrage à l'ouverture de session). | |
| 1.3 | Brancher l'adaptateur de la pompe. Dans la barre de connexion en haut, choisir son port. | Le lien pompe passe au vert. | |
| 1.4 | Ouvrir `%APPDATA%\Fermentool\config.toml`. Dans `[scale]`, mettre `path = "COMx"` (port de la balance, voir le Gestionnaire de périphériques), `baud = 9600`, `density_g_per_ml = 1.0`. | Fichier enregistré. | |
| 1.5 | Icône de la barre des tâches : « Shut down daemon & quit », puis relancer Fermentool. | L'interface revient. | |

## 2. Balance en direct

| # | Étape | Attendu | OK ? |
|---|---|---|---|
| 2.1 | Regarder le bas de la barre latérale, balance vide et tarée. | « Balance · 0.0 g · stable », en vert. | |
| 2.2 | Poser un objet de masse connue. | Le poids suit en environ 1 s ; « ~ » s'affiche tant que la pesée bouge. | |
| 2.3 | Débrancher le câble de la balance. | Après environ 5 s : « Balance disconnected », en rouge. | |
| 2.4 | Rebrancher. | Retour au vert tout seul, en 15 s au plus. | |

## 3. Calibration du tuyau (onglet « Tubing calibration »)

Faire une calibration en ml/min, et une en rpm si des runs en rpm sont prévus.

| # | Étape | Attendu | OK ? |
|---|---|---|---|
| 3.1 | Bloc **Tube** : lot, référence interne (facultative), Ø intérieur, Ø extérieur. Essayer un Ø extérieur plus petit que l'intérieur. | Message rouge « The outer Ø must be larger than the inner Ø. » | |
| 3.2 | Bloc **Pumping** : ml/min, sens, consigne 10, Calibration time 2 min. Bloc **Weighing** : densité 1.00. | Le bouton « Start burst 1/3 » est actif. | |
| 3.3 | Placer un bécher taré sous la sortie, cliquer « Start burst 1/3 ». | Compte à rebours ; la pompe s'arrête seule au bout de 2 min. | |
| 3.4 | Peser le bécher, saisir le poids, « Save weight ». | La salve 1/3 affiche le poids et la durée réelle. | |
| 3.5 | Rafraîchir la page (F5) avant la salve 2. | La session reprend là où elle en était. | |
| 3.6 | Faire les salves 2 et 3 (vider et tarer le bécher entre chaque). | Aperçu : débits, moyenne, CV, c₀. | |
| 3.7 | « Record calibration ». | La calibration apparaît dans « Recorded calibrations ». CV attendu sous 5 % ; c₀ entre 0,80 et 1,25, sinon affiché en rouge. | |
| 3.8 | « Export CSV ». | Un fichier `tubing-calibrations.csv` est téléchargé. | |

Valeurs relevées : débits ___ / ___ / ___ ml/min, CV ___ %, c₀ ___

## 4. Runs avec trim

Pour chaque run : onglet New run, cocher **« Enable gravimetric trim »**, choisir la calibration du tuyau dans la liste.

### 4.1 Contrôle du choix de calibration

| # | Étape | Attendu | OK ? |
|---|---|---|---|
| 4.1.1 | Contrôle en rpm, trim coché, aucune calibration choisie. | Message rouge, « Start run » grisé, lien « Calibrate this tube ». | |
| 4.1.2 | Contrôle en ml/min, sans calibration. | Note « the trim starts at 1.0 », « Start run » actif. | |

### 4.2 Run A : constante, 10 ml/min, 30 min, avec calibration ml/min

| # | Étape | Attendu | OK ? |
|---|---|---|---|
| A.1 | Lancer le run. | Barre latérale : « Balance · xxx g · stable · ×c₀ ». | |
| A.2 | Après 2 à 3 min. | Overview affiche « Feed delivered vs requested (weighed) », les deux courbes se superposent. | |
| A.3 | Fin du run. | R² ___ (attendu ≥ 0,999), écart ___ % (attendu sous 1 %). | |
| A.4 | Peser le bécher de récupération. | Masse ___ g, soit ___ mL ; comparer au volume « delivered ». | |

### 4.3 Run B : exponentielle, 1 h, départ 5 ml/min, µ = 0,5 h⁻¹

(Le volume final demandé est d'environ 390 mL : prévoir un flacon suffisant.)

| # | Étape | Attendu | OK ? |
|---|---|---|---|
| B.1 | Lancer le run (Exponential, mode « rate µ », µ = 0.5). | Le suivi démarre comme au run A. | |
| B.2 | Fin du run, Overview ou History. | R² ___ (attendu ≥ 0,999), µ requested 0,5000, µ delivered ___ (attendu à ±0,01), écart ___ %. | |

### 4.4 Run C : en rpm, avec calibration rpm (si utilisé)

| # | Étape | Attendu | OK ? |
|---|---|---|---|
| C.1 | Constante en rpm, 30 min, calibration rpm choisie. | c reste proche de 1 ; R² ___, écart ___ %. | |

## 5. Garde-fous (pendant un run constant à 10 ml/min)

| # | Étape | Attendu | OK ? |
|---|---|---|---|
| 5.1 | Toucher ou appuyer légèrement sur le flacon (5 à 50 g d'écart). | « disturbed », puis retour à « stable » ; la courbe « delivered » ne fait pas de saut. | |
| 5.2 | Ajouter plus de 50 g de liquide dans le flacon (recharge). | « refilling… », puis « settling… » (10 s), puis « stable » ; le cumulé continue sans saut. | |
| 5.3 | Pincer le tuyau. | Après environ 5 min : « Balance · trim alarm » en rouge ; c ne bouge plus. Relâcher la pince : l'alarme reste jusqu'au run suivant. | |
| 5.4 | Débrancher la balance 1 min pendant le run. | « Balance disconnected » ; la pompe continue sur le dernier c. Rebrancher : le suivi reprend, écart cohérent. | |

## 6. Reprise après crash

| # | Étape | Attendu | OK ? |
|---|---|---|---|
| 6.1 | Pendant un run avec trim, tuer `fermentool-core.exe` dans le Gestionnaire des tâches. Noter l'heure. | La pompe continue sur sa dernière consigne. | |
| 6.2 | Relancer Fermentool (ou attendre le redémarrage par la tâche planifiée). | Fenêtre de reprise ; « Resume ». | |
| 6.3 | Regarder le suivi. | Le volume délivré inclut ce qui est passé pendant l'arrêt (mesuré par la différence de poids) ; pas de saut de l'écart. | |

## 7. Historique

| # | Étape | Attendu | OK ? |
|---|---|---|---|
| 7.1 | History, ouvrir le run A. | Graphique délivré vs demandé, R², écart. | |
| 7.2 | Ouvrir un run sans trim. | Pas de bloc de suivi. | |

---

## Critères d'acceptation

- R² ≥ 0,999 sur les runs A et B.
- Écart cumulé final sous 1 % (sous 2 % si le run comporte une recharge).
- µ délivré à ±0,01 h⁻¹ du µ demandé (run B).
- Volume « delivered » à ±1 % de la pesée indépendante du bécher.
- Aucune fausse alarme sur les runs sans pince.
- La perturbation, la recharge, la balance débranchée et la reprise après crash se comportent comme décrit.

## Remarques et anomalies

| Étape | Observé | Heure | Capture ou export |
|---|---|---|---|
| | | | |
