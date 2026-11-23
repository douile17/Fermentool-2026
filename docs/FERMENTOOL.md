# Fermentool

Le document de référence du logiciel : ce qu'il fait, comment il le fait, comment
on l'installe et ce qui reste à faire. Il remplace les plans, specs, notes de
session et fiches qui s'étaient accumulés (tous récupérables dans l'historique
git, tag `v0.1.8` et avant). À côté de lui ne restent que `README.md`
(présentation courte), `CLAUDE.md` (consignes pour Claude Code, lu à chaque
session), `CHANGELOG.md` (ce qui change pour l'utilisateur, version par version)
et les protocoles MODBUS des fabricants (`docs/protocoles/`).

Version décrite : **0.1.8** (18 novembre 2026).

## Sommaire

1. Ce que fait Fermentool
2. Les pages de l'application
3. Les runs
4. La régulation par la balance
5. Le remplissage automatique
6. La calibration des tuyaux
7. La caméra et les timelapses
8. Les notifications
9. La reprise après un arrêt brutal
10. Pompes, balance et câblage
11. Alimentation exponentielle : le calculateur F₀
12. Architecture technique
13. Règles de l'interface
14. Installer et exporter
15. Développer
16. Reste à faire, idées

---

## 1. Ce que fait Fermentool

Fermentool pilote une **pompe péristaltique** par MODBUS-RTU pour que sa
consigne (**rpm** ou **ml/min**) suive un **profil dans le temps** pendant des
runs de fermentation d'environ 100 h. Il journalise chaque seconde, reprend au
bon point de la courbe après un arrêt brutal, peut corriger la pompe d'après une
**balance** sous le flacon d'alimentation, remplir ce flacon avec une **seconde
pompe**, prendre des **timelapses** à la webcam et prévenir la personne
responsable du run sur son téléphone.

Matériel :

- PC Windows 10 ou 11, avec l'application Fermentool (fenêtre + daemon).
- Pompe d'alimentation : Baoding Shenchen **LabQ** ou **LabV**, ou une tête
  d'une Innofluid **PDS II**. Pompe de remplissage optionnelle sur le même
  câble (autre adresse, ou autre tête de la même PDS II).
- Adaptateur USB vers RS-485 (FTDI), un câble pour les deux pompes.
- Balance optionnelle (Ohaus Ranger 7000 sur le banc, protocole SICS) sur son
  propre port série. Une seconde balance sous le récipient de réception a
  servi aux tests seulement.
- Webcam optionnelle.

Le **daemon** (`fermentool-core.exe`) fait tout le travail : port série, courbe,
journal SQLite, balance, caméra, notifications, et il sert l'interface sur
`http://127.0.0.1:8730`. La **fenêtre** (`fermentool.exe`, Tauri) n'est qu'un
affichage : la fermer ne touche pas au run. La pompe garde sa dernière consigne
sans nouvelle trame, donc un daemon arrêté laisse la pompe tourner à la
dernière valeur envoyée.

## 2. Les pages de l'application

Navigation : un rail à gauche avec **Feeding profile** (onglets Overview,
History, Tubing calibration), **Camera**, et **Settings** (roue dentée), plus le
thème clair ou sombre. En bas du rail : l'état de la pompe, de la balance et du
daemon.

### Overview

L'état du run en cours, en un seul endroit : un bandeau qui dit l'état (feeding,
holding, refilling, alarme…) avec son pictogramme, le **débit actuel en grand**,
le graphe de la courbe demandée et du débit pesé, l'avancement, le prochain
remplissage, l'activité récente (chaque événement avec le temps depuis le début
du run), et les actions : **Change rate**, **Stop**, **Refill now**.

### New run

Un assistant en cinq étapes, résumées dans la colonne de gauche :

1. **Run** : nom, responsable (obligatoire dès que des personnes sont
   configurées pour les notifications).
2. **Pump and tube** : unité (rpm ou ml/min), sens, régulation par la balance
   ou non, tuyau (calibration), remplissage automatique.
3. **Feed profile** : forme de la courbe, durée, aperçu.
4. **Timelapse** : une image toutes les N minutes pendant le run, si une caméra
   est connectée, et **Images saved in** : le dossier où le run crée le sien
   (`<date>_<heure> <nom> (<n°>)`), Browse… pour un autre (le sélecteur de
   Windows, ouvert par le daemon), « Use the default folder ». Le dossier
   choisi est gardé au Start (`[camera] captures_dir`) : le run suivant part
   de là. Check and start redit le chemin complet.
5. **Check and start** : la liste de ce qui est prêt, puis Start.

### History

La liste des runs, filtrable, avec suppression groupée. Le détail d'un run :
nom, dates, durée, responsable, la courbe en pastilles, puis :

- le **verdict** (On target ou Off target à ±2 %, le plus grand écart après les
  15 premières minutes, quatre chiffres dits une fois) ;
- **Feed rate** : débit demandé et pesé, axe en temps du run ;
- **Running total vs requested** : l'écart cumulé, moyenne sur 5 min ;
- **Feed bottle** : poids avant et après chaque remplissage, versé, et
  l'estimation de ce qui a été donné pendant le remplissage (≈) ;
- **Images** (run avec timelapse) : nombre, intervalle, taille, six images
  réparties sur le run (un clic l'ouvre), **Play** et **Open folder**
  (l'Explorateur, ouvert par le daemon), le chemin. Un dossier déplacé ou
  supprimé est dit à la place. **Play** ouvre le lecteur : les images l'une
  après l'autre, Play/Pause (Espace), image par image (flèches), une barre
  sur tout le run, 5 à 50 images/s, sur l'image le temps du run et le
  volume délivré alors, sous l'image la date et l'heure de la prise (heure
  locale, `taken_at` de `frames`). Échap ferme ;
- repliés : **Settings** (pompe, tuyau, calibration, balance, version),
  **Regulation** (correction, ratio de la pompe, plus grand écart, R²),
  **Journal** en phrases (le brut derrière « Show all N entries »).

Actions : **Run again** (reprend les réglages ; après un changement de débit,
demande à partir de quel débit), **Export settings**, **Export CSV**, **Delete
run**. Tous les temps comptent depuis le début du run, sauf la ligne de dates de
l'en-tête. Un remplissage n'est pas pesé : il est dessiné en tirets gris.

### Tubing calibration

La calibration d'un tuyau sur la pompe : trois mesures pesées (§6), le résultat
(ml/min par rpm ou c0, CV), la liste des calibrations avec archive et
restauration, l'amorçage de la ligne.

### Camera

La vue en direct, **Take a picture** (enregistre l'image, dit dans quel dossier,
avec **Open** et **Show in folder**), le timelapse en cours et ses images, et
le lancement d'un timelapse sans run (§7). Sur les dernières images, **Play**
ouvre le lecteur de History sur les images prises jusque-là, pendant le run
(celles prises le lecteur ouvert n'y entrent pas : Play à nouveau), et
**Open folder** le dossier du timelapse.

### Settings

Sections : **Pumps** (les pompes du câble, leur état, connexion), **Balance**
(port, densité du liquide, position, limite de correction), **Camera** (caméra,
réglages d'image, images par seconde, dossier des captures), **Runs** (reprise
après arrêt, intervalle de timelapse par défaut), **Notifications** (personnes,
topic ntfy avec QR, webhook Teams, test), **Display** (tailles de tuyau en mm ou
en pouces), **Daemon** (port), **About** (version, crédits, liens). `#settings/<id>`
ouvre une section directement.

## 3. Les runs

### Unité et pilotage

Un run est en **rpm** ou en **ml/min**.

- **Sans régulation** : aucun tuyau n'est demandé. La pompe reçoit la valeur
  telle quelle. En ml/min, c'est la pompe qui convertit en rpm avec sa propre
  table tête/tuyau : la calibrer elle-même pour le tuyau installé. Après un
  Stop, la LabQ affiche la vitesse moteur de sa dernière consigne (run 267 :
  43.3 rpm pour 2.000 ml/min), calculée par elle.
- **Avec régulation par la balance** (§4) : un tuyau calibré sur le modèle de
  la pompe d'alimentation est demandé (obligatoire en rpm, optionnel en
  ml/min). Un run en ml/min avec une calibration en rpm est **piloté en rpm** :
  la courbe, le journal, le volume et la correction restent en ml/min, seule la
  valeur écrite à la pompe et relue est en rpm. La table interne de la pompe ne
  compte alors plus. Refusé si la courbe demande plus que le maximum de la
  pompe.

Fermentool **n'écrit jamais les registres tête/tuyau** d'aucune pompe, ni le
registre 0010 de la PDS II (marche/arrêt de toutes les têtes du contrôleur).

### Formes de courbe

`value_at(temps écoulé)` est la seule entrée du moteur de courbes : pure (pas
d'horloge, pas d'E/S), c'est ce qui rend la reprise exacte.

| Forme | Paramètres |
|---|---|
| **Constant** | une valeur, une durée |
| **Linear** | début, fin, durée |
| **Exponential** | début, fin, durée (µ implicite `ln(fin/début)/durée_h`), ou début, µ, durée (fin `début·e^(µ·durée_h)`) ; calculateur F₀ (§11) |
| **Sigmoid** | logistique normalisée sur [début, fin], monotone, touche les deux bouts pour toute pente |
| **Steps** | une table de débits, chacun à partir d'un temps, jusqu'à 1000 lignes, importée d'Excel (`.xlsx`, `.csv`) avec un modèle téléchargeable ; un palier qui commence avant le précédent est refusé |
| **Custom** | segments linéaires ou paliers |

Seules Linear et Exponential correspondent à un modèle cinétique. Les valeurs
sont bornées à `[clamp_min, clamp_max]`. L'aperçu de New run est un tracé de 200
points, pour l'affichage seulement.

### Fin de courbe : le maintien

Un run de dosage **ne s'arrête pas avec sa courbe** : il tient la dernière
valeur, toujours régulé et journalisé chaque seconde, jusqu'à **Stop**, qui
l'enregistre `completed` (`stopped` si arrêté avant la fin de la courbe). Une
fenêtre informe de la fin de courbe. Un arrêt brutal pendant le maintien reprend
dans le maintien. Les mesures de calibration, elles, s'arrêtent à leur durée.

### Changer le débit d'un run Constant

**Change rate** pendant un run Constant : la nouvelle consigne est d'abord
envoyée à la pompe, et enregistrée seulement si elle l'accepte (une valeur
refusée ne laisse aucune trace). Chaque changement est une ligne de
`setpoint_changes` ; tout ce qui pilote ou compte utilise la courbe effective
(la courbe d'origine plus les changements). Dans le maintien, le changement
atteint la pompe à la seconde suivante.

### États d'un run

`running`, `completed` (arrêté après la fin de la courbe), `stopped` (arrêté
avant), `aborted`. Un run en simulateur est refusé sauf
`serial.allow_simulator = true`, de même qu'un run sur un port configuré mais
fermé.

## 4. La régulation par la balance

Le flacon d'alimentation est sur la balance. Le daemon compare la masse
réellement partie à celle demandée et corrige la pompe par un facteur **c**
(consigne envoyée = valeur de la courbe × c). Lecture de la balance toutes les
500 ms par son propre fil, correction recalculée régulièrement.

- **Départ** : c part du c0 de la calibration du tuyau, ou de 1/k, le ratio
  appris par le dernier run sur la même calibration (au plus 15 % de la
  calibration, sinon c'est un autre tuyau : retour au c0). Avant que le ratio
  soit mesuré, c rattrape le déficit cumulé autour de ce départ.
- **Ratio de la pompe** (masse pesée / masse commandée) : mesuré sur une
  fenêtre courte (50 pas de balance, 120 s), puis une fenêtre stabilisée (200
  pas, 300 s) avec des pas de c de 0.5 % au lieu de 2 %. Tous les seuils sont en
  pas de balance, donc une balance au gramme les adapte.
- **Usure du tuyau** : une fois stabilisé, le ratio de référence suit le ratio
  mesuré à au plus 1 %/h (l'usure réelle : 0.1 à 0.4 %/h), et la pesée n'atteint
  c qu'avec 3 min de retard. Un ratio qui s'écarte de plus de 1 % est une
  perturbation (poids posé, ligne qui fuit dans le flacon) : c tient, et après
  15 min l'alarme **RatioJump** se lève. Valable là où la balance voit 1 % en 5
  min (dès ~4 ml/min sur une balance au 0.1 g) ; en dessous, le ratio mesuré
  régule directement.
- **Bornes** : `[1/(1+p), 1+p]`, p = `[scale] trim_limit_pct`, modifiable en
  cours de run.
- **Alarmes** : **FeedStopped** (la pompe tourne, le poids ne baisse pas),
  **Saturated** (ratio hors bornes 5 min), **WrongSide** (le poids monte au lieu
  de baisser), **RatioJump**. Une alarme remet c à sa valeur d'avant le début du
  problème et le fige. FeedStopped et Saturated se lèvent seules quand le ratio
  revient ; le manque accumulé entre-temps n'est pas rattrapé d'un coup.
- **Manipuler le flacon ne bouge jamais c** : un toucher, un soulèvement, un
  remplissage, une balance muette plus de 3 s sont comptés comme ratio ×
  commandé (estimé, en gris dans History), puis les lectures des 3 minutes
  suivantes aussi, le temps que la balance se stabilise.
- **Position de la balance** : `feed` (sous le flacon, le poids baisse) ou
  sous un récepteur (le poids monte).

## 5. Le remplissage automatique

Une seconde pompe remplit le flacon d'alimentation quand le daemon a tiré la
quantité choisie au départ du run (« Automatic, after 180 g »).

- Un remplissage se reconnaît à un saut de plus de 50 g, ou une montée de plus
  de 50 g en 30 s, ou au bouton « Refill bottle ». Il est fini quand le niveau
  tient 20 s, puis 10 s.
- La vitesse de la pompe de remplissage (`[refill] speed_rpm`) est écrite à
  chaque départ, et tout de suite si elle change en cours de remplissage. Son
  adresse, son modèle et sa tête sont verrouillés pendant un run avec
  remplissage.
- **Un remplissage qui ne monte plus est coupé** (réservoir vide) :
  `alarm_refill_dry`, le cycle s'arrête jusqu'à « Refill now ». Jamais monté
  (amorçage), coupé après 3 min (`alarm_refill_no_rise`). Une pompe lente qui
  remplit continue de monter et n'est jamais coupée.
- Une balance muette coupe le remplissage après 3 s sans lecture ; le
  remplissage repris compte pour un seul.
- Settings, Pumps interroge la pompe de remplissage (lecture de son registre de
  vitesse, jamais un démarrage) pour montrer son état hors run.

## 6. La calibration des tuyaux

Trois **mesures** (le mot de l'interface, jamais « burst »), chacune pompe à une
vitesse connue pendant une durée, pesée par la balance (automatique) ou à la
main. Le résultat : ml/min par rpm (calibration en rpm) ou c0 (en ml/min), avec
le CV des trois mesures. Une calibration appartient au modèle de pompe sur
lequel elle a été mesurée.

- Le poids doit aller dans le sens de la position de la balance ; trois pas à
  l'envers lèvent l'alerte « backwards ». Le sens se verrouille à la première
  mesure pesée.
- **Amorçage** : la pompe d'alimentation à pleine vitesse dans le sens de la
  calibration jusqu'à Stop, 5 min au plus. Pas une mesure.
- Une mesure interrompue par un arrêt du daemon peut reprendre dans
  `[resume] grace_minutes`.

## 7. La caméra et les timelapses

- La webcam est lue par un processus à part, `fermentool-camera.exe` (Media
  Foundation, priorité basse), lancé et arrêté par le daemon, tué avec lui.
  Jamais d'appel caméra sur le fil de contrôle : la pompe et la balance passent
  d'abord.
- Vue en direct seulement quand quelqu'un regarde (15 images/s au plus,
  `[camera] live_fps`). Réglages d'image (exposition, mise au point,
  luminosité…) quand la caméra les propose, enregistrés par caméra.
- **Timelapse lié à un run** : une image toutes les N minutes tant que le run
  est piloté, arrêté avec lui. **Timelapse seul** : depuis la page Camera,
  repris après un redémarrage. Disque sous 200 Mo ou dossier absent : en pause,
  reprise automatique.
- Chaque image garde dans la base et dans `images.csv` : la caméra et ses
  réglages, le temps du run, la consigne, le volume délivré, le poids et c. Ces
  images serviront plus tard à entraîner un modèle.
- Dossier : `[camera] captures_dir`, par défaut `%APPDATA%\Fermentool\captures`.
  Supprimer un run supprime ses images.

## 8. Les notifications

Un fil `notifier` suit la table des événements et envoie les événements
notables d'un run à **son responsable seulement** : ntfy (application
téléphone, topic privé, QR dans Settings) et/ou un webhook Teams Workflow.
Jamais sur le fil de contrôle. Les alarmes qui durent sont renvoyées toutes les
3 min jusqu'à acquittement (bouton « Acknowledge » de ntfy, ou bandeau de
l'interface), disparition de la cause ou arrêt du run. Les messages de lien
perdu ou retrouvé : un toutes les 15 min au plus. Les mesures de calibration
ne sont jamais notifiées.

## 9. La reprise après un arrêt brutal

- Au démarrage, un run resté `running` est un run interrompu. Selon `[resume]
  prompt`, l'interface propose **Resume** ou reprend seule.
- Reprendre relance toute la séquence de démarrage de la pompe (elle a pu être
  éteinte) à la valeur de la courbe **pour le temps réellement écoulé** depuis
  `started_at`, qui ne change jamais. Un run reprend seulement sur la pompe
  qu'il pilotait.
- L'horloge est gardée : si le temps système saute de plus de 5 s pendant un
  run, le temps monotone fait foi.
- Hors session ouverte, rien ne tourne : le daemon démarre à l'ouverture de
  session (tâche planifiée). Régler Windows Update pour ne pas redémarrer
  pendant un run, et décocher « Ask before resuming » pour une reprise sans
  clic.

## 10. Pompes, balance et câblage

### Modèles de pompe

Chaque modèle a ses limites, constantes du code (jamais un réglage) :

| Modèle | Max | Pas rpm | Pas ml/min | Plancher ml/min |
|---|---|---|---|---|
| LabQ | 350 rpm | 0.1 | 0.001 | aucun |
| LabV (LabV1-II) | 150 rpm | 0.01 | 0.01 | 0.1 |
| PDS II (têtes 1 à 8) | 600 rpm | 0.01 | 0.01 | 0.1 |

Sous le plancher, le moteur arrête la pompe et la redémarre au-dessus. La LabV
et la PDS II ont un registre de mode, mis en « transferring » au démarrage d'un
run, d'une reprise, d'un remplissage. `config.toml` : `[pump] model | head`,
`[refill] pump_model | pump_head`. La consigne est écrite toutes les ~150 ms ;
la pompe veut au moins 100 ms entre deux trames.

### Liaison série

MODBUS-RTU, 9600 bauds, **8E1** pour les trois modèles (chaque pompe réglée à
9600 à la main), adresse 1 pour l'alimentation, 2 pour le remplissage (deux
têtes d'une PDS II partagent l'adresse). Les pompes n'acceptent les commandes
que sur leur écran principal. Flottants IEEE-754, mots dans l'ordre big-endian
(`8.9 → 41 0E 66 66`), CRC-16 maison.

### Câblage

```
PC ── USB ── adaptateur FTDI USB-RS485 ── A/B ── pompe 1 ── A/B ── pompe 2
```

- Une paire torsadée pour A/B, ne pas croiser A et B (sans trafic, inverser A et
  B est la première chose à essayer). Terminaison de 120 Ω inutile à la
  longueur du banc.
- **LabQ, DB15** : broche 5 = A+, broche 4 = B−. Pas de masse RS-485 sur ce
  connecteur. Ne rien brancher d'autre : 1 et 2 marche/arrêt et sens externes,
  6 à 9 entrées analogiques (**ne pas les relier entre elles, ça abîme la
  pompe**), 11 à 13 sortie d'état, 14 et 15 alimentation 5 V. Un câble à
  TXD+/TXD−/RXD+/RXD− : relier TXD+ et RXD+ vers la broche 5, TXD− et RXD− vers
  la broche 4.
- **LabV, DB9** : broche 9 = A, broche 8 = B, broche 7 = masse RS-485 (à
  relier à la masse de l'adaptateur).
- **PDS II** : connecteur à relever sur l'appareil.
- La webcam sur un autre contrôleur USB que les adaptateurs série si possible.
- Sur le banc (17 novembre) : pompes sur **COM10**, balance du flacon sur
  **COM7**, balance de réception (tests) sur COM9.

### Un bus qui se tait

Sur ce banc, éteindre **l'une ou l'autre** pompe rend le bus muet pour les
deux (cause probable, non mesurée : le transceiver d'une pompe éteinte charge
A/B). L'interface dit donc « Pumps not answering » sans accuser l'une ou
l'autre. Pendant un run, éteindre la pompe de remplissage empêche la pompe
d'alimentation de suivre sa courbe. Correctif prévu : un hub RS-485 à ports
isolés (une pompe par port, aucun changement logiciel). L'adaptateur FTDI
USB-RS485-WE n'est probablement pas isolé galvaniquement : à vérifier.

### Balance

SICS, 9600 bauds, sur son propre port. Un fil `scale-poll` demande un poids
toutes les 500 ms et garde la dernière réponse ; une balance muette est un
`Timeout` (le lien est gardé), un port cassé est rouvert. Un PC chargé retarde
la balance sans la casser (la demande peut attendre 800 ms). L'indicateur de
balance et la case de régulation n'existent qu'une fois `[scale] path` réglé :
**une fonction « absente » sur un autre PC, regarder d'abord ce qui la
conditionne**.

## 11. Alimentation exponentielle : le calculateur F₀

En Exponential, mode **rate µ**, la consigne est `Start · e^(µ·t)` (t en
heures) ; elle double tous les `0.693/µ` heures. Un feed linéaire ne garde pas µ
constant.

| µ (h⁻¹) | × par heure | doublement |
|---|---|---|
| 0.05 | ×1.05 | ~14 h |
| 0.10 | ×1.11 | ~7 h |
| 0.15 | ×1.16 | ~4.6 h |
| 0.20 | ×1.22 | ~3.5 h |
| 0.30 | ×1.35 | ~2.3 h |

Choisir µ comme une fraction de µmax de la souche (E. coli : µmax ≈ 0.5 à 0.7,
souvent 0.1 à 0.3 visé).

Le panneau « Fed-batch F₀ from strain parameters » (New run) calcule :

```
F₀ [L/h]    = (µ / Y_x/s + m_s) · X₀ · V₀ / S_f
F₀ [ml/min] = F₀[L/h] · 1000 / 60
t_max [h]   = (1/µ) · ln(1 + µ·(V_max − V₀) / F₀[L/h])
```

- X₀ biomasse au début du feed (g/L), V₀ volume (L), Y_x/s rendement (g/g ;
  glucose et E. coli ≈ 0.45), S_f substrat dans le flacon (g/L), V_max volume
  maximal du réacteur.
- **m_s** (maintenance, g·g⁻¹·h⁻¹, E. coli et glucose ≈ 0.02 à 0.04) est
  optionnel, 0 par défaut : sans lui, la forme croissance seule sous-alimente
  d'environ 5 à 10 %.
- Hypothèses : µ et Y_x/s constants, culture limitée par le substrat (S ≈ 0).
  t_max ignore évaporation, prélèvements et base de régulation du pH. En
  pratique le transfert d'oxygène limite souvent avant.
- F₀, t_max et la courbe Exponential-µ supposent tous `F(t) = F₀·e^(µt)` : qui
  en change un revérifie les deux autres.
- « Use → » remplit Start, µ et Duration.

Exemple : µ 0.15, X₀ 3 g/L, V₀ 1.5 L, Y_x/s 0.45, S_f 500 g/L, V_max 3 L →
F₀ = 0.003 L/h = **0.05 ml/min**, t_max ≈ **29 h** (0.054 ml/min avec
m_s = 0.025). À 20 h : ~1.0 ml/min ; à 29 h : ~3.9 ml/min.

Pièges : l'exponentielle explose (Start 5, µ 0.15, 24 h → ~183) et la consigne
plafonne au maximum de la pompe, d'où un palier : regarder l'aperçu avant de
lancer. **Limite connue** : aucune alerte si F₀ est sous le débit minimal que la
pompe tient réellement.

## 12. Architecture technique

### Les pièces

| Chemin | Rôle |
|---|---|
| `crates/fermentool-curves` | Les courbes (`CurveSpec`, `value_at`), pures, sans E/S |
| `crates/fermentool-modbus` | Trames MODBUS-RTU, CRC, cartes de registres LabQ / LabV / PDS II, limites |
| `crates/fermentool-core` | Le daemon : `config`, `store` (SQLite), `engine` (courbe, reprise, remplissage, amorçage), `tracking` (régulation), `control` (le moteur sur son fil), `api` (REST axum + WebSocket), `notify`, `camera`, `timelapse`, interface embarquée |
| `crates/fermentool-camera` | L'assistant caméra (processus séparé) |
| `ui/` | Svelte 5 + Vite, compilé dans `ui/dist/`, embarqué par le daemon ; logique testable dans `ui/src/lib/` |
| `src-tauri/` | La fenêtre Windows (Tauri v2), l'icône de notification, l'installateur NSIS |

### Les fils du daemon

- **Fil de contrôle** : la courbe, la pompe (consigne toutes les ~150 ms,
  découplée du tick de journal d'1 s), les commandes de l'interface une à une.
  Toute E/S bloquante qu'il atteint est enveloppée : un appel système coincé ne
  bloque que cet appel. Le port série tourne sur un fil `serial-worker`, avec 2
  s au plus par transaction (un adaptateur USB arraché peut bloquer le pilote
  indéfiniment ; le fil coincé est abandonné, un neuf rouvre le port).
- **`scale-poll`** : la balance. **`notifier`** : les notifications.
  **`timelapse`** : les images. Chacun avec sa propre connexion SQLite.
- L'API lit le journal sur sa propre connexion : runs, ticks, événements,
  calibrations ne passent jamais par le fil de contrôle.
- Une fois par minute, une ligne `timing` dans le journal du jour
  (`setpoint_late`, `tick`, `scale_age`, `scale_poll`) ;
  `scripts/timing_report.py` compare deux périodes.

### Persistance

SQLite (`%APPDATA%\Fermentool\fermentool.sqlite`), WAL, `synchronous = FULL`,
migrations maison numérotées (`PRAGMA user_version`, 17 aujourd'hui ; la 16 est
réservée à la branche `feat/rate-change-reset`). Tables principales : `runs`,
`ticks` (une ligne par seconde : consigne, écrit ou non, relecture, poids,
délivré), `events`, `setpoint_changes`, `calibrations`, `timelapses`,
`timelapse_images`, `app_state`. Une écriture de journal qui échoue n'arrête
jamais la pompe : l'interface alerte.

### L'API (127.0.0.1 seulement)

| Domaine | Routes |
|---|---|
| État | `GET /api/status`, `GET /api/health`, `WS /api/ws`, `POST /api/shutdown` |
| Runs | `GET/POST /api/runs`, `GET/DELETE /api/runs/{id}`, `…/ticks`, `…/events`, `…/tracking`, `POST …/stop`, `…/abort`, `POST /api/runs/active/rate`, `DELETE /api/history`, `POST /api/preview` |
| Reprise | `GET /api/recovery`, `POST /api/recovery/resume`, `…/discard` |
| Pompe et câble | `GET /api/serial/ports`, `POST /api/serial/reconnect`, `…/disconnect`, `POST /api/pump/stop`, `…/prime`, `…/prime/stop` |
| Balance et remplissage | `POST /api/scale/connect`, `…/disconnect`, `…/refill_mode`, `…/refill_done`, `POST /api/refill/test`, `…/ping`, `…/now`, `…/stop` |
| Calibrations | `GET/POST /api/calibrations`, `POST …/{id}/archive`, `…/restore` |
| Caméra | `/api/camera` (état, devices, connect, disconnect, controls, fps, picture, captures, interval, frame.jpg, live) |
| Timelapses | `/api/timelapses` (start, current, stop, pause, resume, interval, images, frames, open du dossier, fichiers, show, open) ; `POST /api/folders/pick` (sélecteur de dossier de Windows) |
| Notifications | `POST /api/notify/test`, `GET /api/notify/pages`, `POST /api/notify/ack` |
| Fenêtre de bureau | `POST /api/files/save`, `POST /api/about/open`, `GET /api/about` |

Les erreurs de démarrage et de reprise sont structurées : `{ message, code,
hint }`, `message` sur une ligne, `hint` derrière « Why? ».

### `config.toml`

Dans `%APPDATA%\Fermentool\`, écrit au premier lancement :

```toml
port = 8730
[serial]  path = "COM10", baud = 9600, allow_simulator = false
[pump]    address = 1, model = "labq", head = 1
[resume]  prompt = true, grace_minutes = 5
[storage] dir = ""             # vide = %APPDATA%\Fermentool
[log]     dir = "", level = "info", retain_days = 30
[scale]   path = "COM7", baud = 9600, density_g_per_ml = 1.0, position = "feed", trim_limit_pct = 25.0
[refill]  pump_address = 2, pump_model = "labq", pump_head = 1, speed_rpm = 150.0, direction = "ccw"
[notify]  ntfy_server = "https://ntfy.sh", [[notify.people]] name, ntfy_topic, webhook
[display] tube_sizes = "mm"
[camera]  device, enabled, live_fps, interval_min, captures_dir, [camera.controls."<caméra>"]
```

(Présenté ici sur une ligne par section ; le fichier réel a une clé par ligne.)
Les journaux du jour sont dans `%APPDATA%\Fermentool\logs`.

## 13. Règles de l'interface

À lire avant toute modification de l'interface, et à suivre plutôt qu'à
rediscuter. Les jetons sont dans `ui/src/app.css`.

### Comment on change l'interface

1. **Une maquette d'abord, sur une page à part**, jamais directement dans
   `ui/`. On itère dessus.
2. On implémente quand l'utilisateur le dit (« mets en place », « go »).
3. On garde la navigation (rail, onglets, roue dentée, thème).
4. On vérifie sur une copie locale avec des états simulés, jamais en agissant
   sur la vraie pompe.
5. Fini veut dire : `npm run check` propre, `npm test`, `npm run build`,
   déployé si aucun run n'est en cours, `npm run smoke`.

### La couleur seulement quand il faut regarder

Le fonctionnement normal est **calme**.

| État | Bandeau | Pictogramme |
|---|---|---|
| Alimentation normale, maintien | blanc | tête de pompe, rotor dans le vrai sens |
| Remplissage, stabilisation | bleu `--fill-bg` | flacon qui se remplit |
| Remplissage arrêté | orange `--warn-bg` | flacon avec « ! » |
| Alarme | rouge `--danger-bg` | triangle |
| Pas de nouvelles du daemon | blanc, figé | point gris |

- La logique d'état est une fonction pure, `runState()` dans
  `ui/src/lib/runview.js` ; un nouvel état y va, avec un test.
- Les images bougent **seulement quand il se passe quelque chose**, et se figent
  quand le daemon se tait. `prefers-reduced-motion` respecté.
- **Pas de mouvement sans fin, ni sur les clics courants** : pas de fondu entre
  onglets, pas de reflet sur une barre, pas de secousse ni de halo permanent ;
  un avertissement à revoir pulse une fois.
- Une seule pompe animée par écran. L'état dit **à un seul endroit** par page,
  jamais une pastille dans le menu. Titre de fenêtre et icône suivent l'état.

### Couleurs

`--teal-700` primaire (un bouton primaire par vue), `--teal-400` courbe
demandée, `--lime-300` remplissage sous la courbe (jamais du texte),
`--green-500` le vivant (trace pesée), `--fill` le remplissage, `--measured`
pesé et avertissements, `--danger` arrêt et alarmes. **Le texte sur une teinte
utilise son jeton `*-ink`** ; chaque paire de texte est WCAG AA (4.5:1) dans les
deux thèmes. Aucune couleur en dur dans un composant. Deux exceptions
demandées (2026-11-19) : **Play** des images en vert (`.btn-play`, `--play` /
`--on-play`, AA) et les pictogrammes de dossier en jaune (`--folder`, réglé
une fois dans `app.css` sur `svg.icon[data-icon="folder"]`), Open folder sur un
fond jaune pâle (`.btn-folder`).

### Typographie et pictogrammes

- **Red Hat Text** pour tout, chiffres compris (largeur égale,
  `tabular-nums`) ; pas de zéro pointé ni barré. Choisie le 23 novembre à la
  place d'IBM Plex Sans (vue dans trop d'applications générées) : une
  humaniste aux formes ouvertes, que les études de lecture d'un coup d'œil
  (MIT AgeLab, Reimer 2014, Dobres 2016) donnent plus rapide qu'une
  grotesque serrée, et libre (OFL), donc embarquable. **Barlow 600** pour les titres
  d'état seulement. Polices embarquées (le PC peut être hors ligne).
- Pictogrammes **Solar, style linear** (CC BY 4.0, crédit dans About), copiés
  dans `components/Icon.svelte` ; jamais une autre bibliothèque. Pompe,
  flacon, triangle et roue dentée sont nos dessins.

### Graphes

L'axe des valeurs **part de 0** et finit sur une valeur ronde au-dessus des
données, avec des lignes graduées (`lib/chartscale.js`). Courbe demandée
`--teal-400`, trace pesée `--green-500`, remplissages en bandes `--fill`. Temps
du run, jamais l'heure.

### Chiffres et mots

- Textes de l'interface **en anglais**, dates en anglais (`en-GB`, 24 h).
- Chaque chiffre dit **une fois** par écran, avec ce qu'il veut dire
  (« 100.0 % delivered », jamais un « 0.0 % » nu). Une estimation est dite
  comme telle.
- **« measurement », jamais « burst »** dans l'interface (le code garde
  `burst`) ; « run » désigne un run d'alimentation.
- **Pas de tiret cadratin (—), ni de trait d'union entouré d'espaces** à sa
  place : virgule, deux-points, point.
- Le rouge pour ce qui a mal tourné : un run « stopped » est gris, « aborted »
  rouge. Une action rare et destructive est un lien discret.

### Forme

Rayons 14 px (cartes), 9 px (contrôles), 6 px (étiquettes avec du texte, jamais
une pilule complète). Espacements 4 / 8 / 12 / 16 / 24 / 32 / 48. Rail de 200
px. Focus toujours visible. Une unité ne se retrouve jamais seule sur une
ligne.

### La fenêtre de bureau

C'est une **application de bureau**, pas un site : on la juge à partir de sa
taille minimale (900 × 600), avec la mise à l'échelle de Windows, jamais pour
le mobile. Dans la fenêtre (`lib/desktop.js`) : pas de menu du navigateur au
clic droit (gardé dans un champ et sur un texte sélectionné), F5 et Ctrl+R ne
rechargent pas, pas d'image glissée, boutons et menus non sélectionnables. Elle
s'ouvre agrandie.

**WebView2 n'ouvre aucun onglet et n'enregistre aucun téléchargement par lien
blob**, sans rien dire. Tout ce qui ouvre ou enregistre passe par le daemon
(`/api/files/save`, `/api/about/open`, `/api/timelapses/images/{id}/open|show`,
`/api/timelapses/{id}/open`, `/api/folders/pick`)
; le smoke test échoue sur un `target="_blank"`. **L'interface de la fenêtre
est compilée dans `fermentool.exe`** : déployer le daemon ne la change pas, il
faut un nouvel installateur.

## 14. Installer et exporter

### Ce qu'installe l'installateur

`Fermentool_<version>_x64-setup.exe` (~7 Mo) installe pour l'utilisateur, dans
`%LOCALAPPDATA%\Fermentool` : `fermentool.exe` (la fenêtre),
`fermentool-core.exe` (le daemon), `fermentool-camera.exe` (l'assistant
caméra), et une tâche planifiée **Fermentool** qui lance le daemon à
l'ouverture de session et le relance s'il plante (toutes les minutes, 3 fois).
Rien d'autre à installer : les exe sont liés en statique (pas de
`VCRUNTIME140.dll`) et WebView2 est déjà dans Windows 10 et 11 (téléchargé
sinon ; `offlineInstaller` pour un PC sans internet ni WebView2, ~220 Mo).

Une mise à jour arrête proprement le daemon avant de copier, et **refuse de
s'installer si un run est en cours**. L'historique (`fermentool.sqlite`) et la
configuration restent ceux du PC ; un PC neuf démarre vide. **Ne jamais mettre
de `.sqlite` ni de `config.toml` dans l'installateur.** La fenêtre fermée, le
daemon continue ; l'icône de notification propose Open, Shut down daemon &
quit, Quit. Le port 8730 est fixe pour la fenêtre.

### Produire l'installateur

Sur le PC de développement (Rust, Node, `cargo tauri`, Visual Studio
Community) :

1. Partir de `main` à jour, arbre propre.
2. **Monter la version**, toujours : `Cargo.toml` (`[workspace.package]`),
   `src-tauri/tauri.conf.json`, `ui/package.json` et `ui/package-lock.json`.
3. Tests : `cargo test --workspace --exclude fermentool-tauri`.
4. `.\src-tauri\scripts\copy-sidecar.ps1` : construit `ui/dist`, le daemon et
   l'assistant caméra, les place dans `src-tauri/binaries/`, refuse un exe qui
   dépend de `VCRUNTIME140`. **Il échoue à son `npm ci` si VS Code est ouvert
   sur le dépôt** (il tient le binaire de rolldown) : fermer VS Code, ou faire
   ses étapes à la main avec `npm install`.
5. `cargo tauri build` (dans `src-tauri`) →
   `target\release\bundle\nsis\Fermentool_<version>_x64-setup.exe`.
6. **Interface dans la vraie fenêtre** (si l'interface a changé) :

   ```powershell
   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9444"
   Start-Process target\release\fermentool.exe
   Remove-Item Env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
   cmd /c 'cd /d <dépôt>\ui && set SMOKE_CDP=9444&& npm run smoke'
   ```

   Essayer aussi à la main : Template de New run dans Téléchargements, Take a
   picture puis Open et Show in folder. Fermer la fenêtre ensuite.
7. **Contrôle obligatoire** : `/api/status` répond avec la bonne
   `app_version`, tuer `fermentool.exe`, le daemon et `/api/status` sont
   toujours là.
8. Commiter la version, taguer `vx.y.z`, pousser.

### Installer sur un autre PC

1. Copier l'installateur et le lancer (SmartScreen : « Informations
   complémentaires », « Exécuter quand même » ; non signé).
2. **Pompe** : brancher l'adaptateur, choisir son port dans la barre en haut.
3. **Balance** : Settings, Balance : son port (pas celui de la pompe), 9600
   bauds, densité, Save.
4. Vérifier en bas du rail la pompe et « Balance 0.0 g, stable ».

### Dépannage

| Symptôme | Cause probable | Correctif |
|---|---|---|
| « reconnecting… », pas de pompe | le daemon ne tourne pas | Gestionnaire des tâches ; lancer `%LOCALAPPDATA%\Fermentool\fermentool-core.exe` pour voir l'erreur ; journaux dans `%APPDATA%\Fermentool\logs` |
| ancienne version après une mise à jour | le daemon tournait | « Shut down daemon & quit », relancer l'installateur |
| pas de case de régulation, pas d'indicateur Balance | aucune balance réglée | Settings, Balance |
| « Balance disconnected » | port, câble, vitesse | Gestionnaire de périphériques, baud dans Settings |
| « Camera helper missing » | `fermentool-camera.exe` absent à côté du daemon | le construire avec `--features capture` |

Un PC sans session ouverte (pas de tâche à l'ouverture de session) : tâche
planifiée « au démarrage » sur `fermentool-core.exe`, ou NSSM.

## 15. Développer

```sh
cargo test                                   # toutes les crates (sauf la fenêtre)
cargo test -p fermentool-curves              # les courbes seules
cargo run -p fermentool-core                 # daemon + interface sur :8730 (simulateur par défaut)
cargo build --release -p fermentool-camera --features capture   # l'assistant caméra

cd ui
npm install
npm run dev      # :5173, /api renvoyé vers :8730
npm run build    # ui/dist, embarqué par le daemon
npm run check    # ESLint + svelte-check, zéro avertissement
npm test         # vitest sur src/lib
npm run smoke    # parcourt l'application déployée dans Edge sans tête, ne change rien
```

- Sur ce PC : compiler depuis l'environnement **Visual Studio Community**
  (`vcvars64.bat`) ; les Build Tools 2022 installés sont cassés. `rustc` meurt
  parfois en `STATUS_HEAP_CORRUPTION` : réessayer une fois. npm depuis `C:`
  majuscule (vitest échoue avec `c:`).
- Pendant un run, les compilations et tests lourds tournent en **priorité
  basse** : un PC saturé faisait rater des lectures de balance.
- Le daemon du banc : `target\release\fermentool-core.exe`, lancé sans console
  avec `FERMENTOOL_NO_BROWSER=1`. Jamais la version debug pour l'utilisateur.
- **Un bug corrigé est un test ajouté** : test unitaire si la logique est dans
  `ui/src/lib/` ou une crate, contrôle du smoke test si c'est un chemin dans
  l'application.
- Les détails pour Claude Code (règles, pièges, historique des décisions) sont
  dans `CLAUDE.md`.

## 16. Reste à faire, idées

### À faire

- Installer 0.1.8 sur un PC vierge : fenêtre agrandie, pompe, balance.
- **Changer le débit en fin de n'importe quelle courbe**, avec remise à zéro de
  la correction à chaque changement (archivée sur GitHub le 18 novembre,
  branches `feat/rate-change-reset` et `merge/rate-change-reset`, plus en local). Deux tests du moteur échouent une
  fois fusionné : `the_regulation_rides_a_change` (c plonge de 1.3 % ~140 s
  après un changement 6 → 3 ml/min, à la première fenêtre courte du ratio ; la
  branche seule plonge de 0.5 %) et
  `a_change_in_a_perturbation_keeps_its_c_through_a_slow_rise` (suppose un
  rattrapage dans c que le retard de pesée de 3 min cache maintenant). Essayé
  sans succès : geler le ratio comme après un remplissage, écarter les 5 s
  après le changement. À trancher : comment la remise à zéro et le ratio de
  référence appris se combinent. **Pas nécessaire** (décidé le 18 novembre) : le
  débit ne se change à la main que sur un run Constant, ce que « Change
  rate » fait déjà (§3) ; un accélérostat est une rampe, que la courbe Linear
  fait déjà. À reprendre seulement si un jour on veut changer le débit à la
  fin d'une autre courbe.
- Sur un vrai run à plus de 4 ml/min : pas de fausse alarme « Pump ratio moved
  too fast » sur plusieurs heures, le ratio suit l'usure, pas de pic de c au
  remplissage suivant.
- Démarrage sur le ratio appris : le prochain run régulé sur la calibration 17
  enregistre son k, le suivant démarre à ~×0.95.
- Balance muette plus de 3 s : tout le silence est compté comme estimé, ce qui
  peut cacher une dérive de la pompe. Piste : au-delà de 10 min, garder la
  pesée au retour si elle est plausible (poids en baisse, entre 0.7 et 1.3 fois
  l'estimation). Chaque silence vide aussi la fenêtre du ratio (~8 min figé sur
  un PC chargé).
- Remplissage, limites connues : un flacon tenu hors de la balance plus de 30 s
  peut lancer un remplissage ; un Stop vers une pompe muette est retenté chaque
  seconde ; un arrêt brutal en plein remplissage perd le poids d'avant ; une
  perturbation pendant un remplissage peut l'arrêter trop tôt ; le cycle compte
  en heure murale.
- Hub RS-485 isolé (§10), puis nommer de nouveau la pompe muette.
- Caméra, suite : onglet Timelapses (les timelapses seuls), export vidéo ou
  GIF d'un timelapse, renommer et supprimer. Fait le 18 novembre : dossier
  choisi au lancement, section Images et lecteur dans History.

### Idées

- **Fiche de fermentation en ligne** : un chercheur remplit une fiche (souche,
  milieu, µ, Y_x/s, X₀, V₀, S_f, profil, date, responsable) ; les techniciens
  sont prévenus sur Teams ; Fermentool préremplit New run (jamais de démarrage
  à distance) ; la fiche s'enrichit des prélèvements (OD600, glucose, volume) et
  se termine par un bilan (µ réel contre µ demandé, export pour le cahier de
  labo). Hébergement envisagé : le NAS Synology du labo (données au labo, accès
  par le VPN). Un recalage de la courbe par les prélèvements devrait rester un
  nouveau segment journalisé, pour garder `value_at` pure.
- **Accès depuis le téléphone** (acquitter, arrêter) : par un relais à nous
  (o2switch) ou le NAS, pas AnyDesk ni Cloudflare.
- **Calculateur d'accélérostat**, comme celui du F₀ : le taux de dilution monte
  en continu, D(t) = D₀ + a·t, à volume constant, donc F(t) = V·(D₀ + a·t),
  une rampe Linear (Start = V·D₀, End = V·(D₀ + a·durée)). Le calculateur
  prendrait V, D₀, a et la durée, et remplirait Start et End. Fermentool ne
  pilote que l'alimentation : le volume constant (débordement ou soutirage)
  reste au montage.
- **Calibration à plusieurs vitesses** (le ratio d'un tuyau varie avec la
  vitesse) : reportée le 23 octobre.
- Avertir quand F₀ est sous le débit minimal que la pompe tient (il faut
  d'abord ce chiffre pour la LabQ).
