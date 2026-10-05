# Remplissage automatique de la bouteille pesée

**Date :** 2026-10-02
**Statut :** proposition, à relire avant le plan d'implémentation

## Problème

Un run de 100 h vide la bouteille posée sur la balance une à deux fois. Aujourd'hui,
un remplissage casse la régulation par la balance :

1. **La fin d'un remplissage est détectée au bout de 10 s, quoi qu'il se passe.**
   Le test de stabilité du poids n'a jamais été branché : `recent_variance_g` vaut
   `0.0` en dur dans `Engine::scale_tick`
   ([engine/mod.rs](../../../crates/fermentool-core/src/engine/mod.rs)), donc
   `RefillPending` passe à `RefillSettling` dès la lecture suivante, puis à `Normal`
   10 s plus tard. Le mode manuel (`/api/scale/refill_mode`) a le même défaut, et
   aucun bouton de l'UI ne l'utilise.
2. **Un remplissage par pompe n'est pas reconnu comme un remplissage.** Le poids
   monte régulièrement, il ne saute pas de plus de 50 g d'un coup
   (`REFILL_THRESHOLD_G`). Au-delà du seuil de perturbation (1 à 5 g par lecture
   selon le débit), chaque lecture passe pour un choc : la régulation reste figée
   et le volume est estimé. En dessous, chaque lecture passe pour **du débit
   normal** : la bouteille gagne du poids, l'alimentation semble délivrer en
   négatif, c monte vers la butée ou l'alarme « mauvais côté » se lève.
3. **Après chaque remplissage, la régulation repart de zéro.** L'historique du
   rendement (`tr.points`) est effacé. c reste figé jusqu'à 50 pas de balance et
   2 min de nouvel historique, puis repart sur la fenêtre de démarrage (c par pas
   de 2 %) jusqu'à 200 pas et 5 min. Avec une balance au gramme et une
   alimentation à 1 ml/min, ça fait près d'une heure figé puis plus de 2 h en
   régime nerveux, le régime qui faisait osciller c de ±10 % au run 19.

Aucun de ces cas n'a été vérifié en vrai : le journal ne contient aucun événement
`refill`. Le seul remplissage visible (run 48, à 16 h de run) date d'avant cet
événement, bouteille vide et pompe en butée.

## Montage

- Bouteille pesée : bidon Nalgene 10 L (plastique, environ 1 à 1,5 kg à vide) sur
  une balance de 15 kg (la future ; précision probablement 0,5 ou 1 g).
- Réservoir principal : bidon Nalgene 10 L, relié par le bas à la bouteille pesée
  par une **deuxième LabQ**, sur **le même câble RS-485** que la pompe
  d'alimentation, avec sa propre adresse MODBUS.
- Débit de transfert mesuré : 1,5 L en 5 min à 350 rpm (le maximum), soit
  **300 ml/min**, environ 5 g/s, 0,86 ml par tour.
- **La LabQ redémarre seule après une coupure de courant**, dans l'état où elle
  était.
- Le protocole MODBUS n'a **pas de mode « doser X ml puis s'arrêter »** (registres
  1000 à 1009 : tête, tuyau, vitesse, débit, marche/arrêt, sens, pleine vitesse,
  aspiration). Une pompe lancée tourne jusqu'à ce qu'on lui envoie Stop.

## Objectif

1. Fermentool pilote la pompe de remplissage : sous un niveau bas, il remplit la
   bouteille pesée jusqu'à un niveau haut, sans intervention.
2. La régulation traverse le remplissage sans redémarrer : même c avant et après,
   historique du rendement conservé.
3. Un remplissage qui tourne mal (réservoir vide, tuyau débranché, débordement,
   pompe qui ne répond pas) arrête la pompe de remplissage et sonne jusqu'à
   acquittement.
4. Corriger au passage le remplissage manuel (points 1 et 3 du problème), utile
   sans pompe de remplissage.

## Hors périmètre

- Remplir le réservoir principal : reste manuel, Fermentool suit seulement ce
  qu'il contient.
- Balance sous le récipient receveur (`position = receiver`) : le remplissage
  automatique n'a de sens que balance sous la bouteille d'alimentation ; il est
  refusé dans l'autre position.
- Runs de calibration : jamais de remplissage automatique (bursts courts, opérateur
  présent).
- Écrire les registres tête/tuyau de la pompe de remplissage : comme pour
  l'alimentation, uniquement vitesse (rpm), marche/arrêt et sens.

## Design

### 1. Réglages (`[refill]` dans `config.toml`, carte « Refill » dans Settings)

| Réglage | Défaut | Rôle |
|---|---|---|
| `enabled` | `false` | Remplissage automatique actif |
| `pump_address` | `2` | Adresse MODBUS, différente de `pump.address` |
| `speed_rpm` | `350` | Vitesse de la pompe de remplissage |
| `direction` | `cw` | Sens qui va du réservoir vers la bouteille pesée |
| `bottle_tare_g` | à peser | Poids de la bouteille pesée vide, pour passer du poids au volume |
| `bottle_capacity_ml` | `10000` | Contenance de la bouteille pesée |
| `low_ml` | `1000` | Sous ce volume, un remplissage démarre |
| `high_ml` | `7000` | Le remplissage s'arrête à ce volume |
| `expected_ml_min` | `300` | Débit attendu ; mis à jour par la mesure de chaque remplissage |

Le volume vient du poids : `(poids − tare) / densité` (densité de `[scale]`).

**Contrôle à l'enregistrement :** `high_ml` laisse au moins 10 min de
remplissage libres sous `bottle_capacity_ml` (à 300 ml/min : 3 L). C'est la marge
contre la reprise automatique de la LabQ après une coupure de courant, le temps
que le PC et Fermentool redémarrent (voir §5). Les valeurs par défaut la
respectent.

### 2. Le cycle de remplissage

Un nouvel automate, à côté de celui de la balance, évalué à chaque lecture de la
balance (1 s) :

```
Idle ──(volume < low_ml, conditions OK)──> Filling ──(volume ≥ high_ml − dépassement)──> Settling ──(poids stable 20 s)──> Idle
                                            │
                                            └─(garde-fou, §4)──> Stopped (alarme, plus de remplissage auto jusqu'à acquittement)
```

- **Conditions pour démarrer :** run de dosage actif avec trim gravimétrique,
  balance connectée et en `Normal`, pompe joignable, réservoir non signalé vide,
  aucun garde-fou en cours. Une alarme `FeedStopped` ne bloque pas : une bouteille
  vide en est justement une cause.
- **Filling :** Fermentool écrit la vitesse, le sens, puis Start à la pompe de
  remplissage ; il mémorise le poids et l'heure de départ. La pompe
  d'alimentation **continue de doser** à sa consigne × c.
- **Arrêt anticipé :** le Stop part à `high_ml − dépassement`, le dépassement
  étant ce que la pompe ajoute pendant une lecture et l'envoi du Stop (environ
  1,5 s × débit mesuré, soit 7 à 8 g à 300 ml/min).
- **Settling :** la pompe de remplissage est arrêtée (Stop confirmé par la pompe),
  le poids doit rester dans ±2 pas de balance pendant 20 s.
- **Retour à Idle :** un seul événement `refill` est journalisé : poids avant et
  après, durée, débit mesuré. Le débit mesuré met à jour `expected_ml_min`
  (moyenne glissante) : un débit qui baisse d'un remplissage à l'autre signale un
  tuyau usé ou un réservoir presque vide.
- **Bouton « Refill now »** dans l'UI : démarre un cycle sans attendre le niveau
  bas (même garde-fous). **« Stop refill »** l'interrompt.

### 3. La régulation pendant et après le remplissage

- Pendant `Filling` et `Settling`, l'automate de la balance passe dans un nouvel
  état `Refilling` (distinct de `RefillPending`, qui reste pour le remplissage
  manuel) : c est figé, la détection « feed stopped » est suspendue, pas de mise à
  jour du rendement. C'est ce que font déjà les états de remplissage existants,
  avec un début et une fin connus au lieu d'être devinés.
- La masse délivrée pendant le trou est estimée comme aujourd'hui par
  `settle_anchor` : rendement mesuré `k` × masse commandée pendant le trou.
- **Changement clé : l'historique du rendement n'est plus effacé** à la fin d'un
  remplissage (suppression de `tr.points.clear()` sur `a.refill`). Le trou y
  apparaît comme un segment de pente exactement `k`, neutre pour la pente
  Theil-Sen. La fenêtre « settled » reste disponible, c repart avec des pas de
  0,5 % au lieu de 2 %, sans repasser par le démarrage.
- La référence du débit de diagnostic (`refill_weight_g`, `weight_buffer`) est
  remise à zéro comme aujourd'hui : elle ne sert qu'à l'affichage.

### 4. Garde-fous

Chacun **arrête la pompe de remplissage** et lève une alarme qui sonne jusqu'à
acquittement (relances ntfy, famille `refill`) :

| Condition | Alarme | Cause probable |
|---|---|---|
| Après 30 s de `Filling`, le poids monte de moins de 50 % du débit attendu | `refill_no_flow` | Réservoir vide, tuyau débranché ou pincé, pompe à l'arrêt |
| Poids au-dessus de `high_ml` + 50 g | `refill_overfill` | Stop pas reçu, ou débit plus fort que prévu |
| `Filling` plus long que `(high − low) / débit attendu` × 1,5 | `refill_timeout` | Débit trop faible |
| Balance perdue pendant `Filling` | `refill_scale_lost` | Plus de mesure : on ne remplit pas à l'aveugle |
| Le Stop n'est pas confirmé par la pompe après 3 essais | `refill_stop_failed` | **Critique** : la pompe peut continuer ; Stop renvoyé à chaque lecture |

S'y ajoutent :
- **Stop, fin de run, abort, arrêt du daemon :** Stop à la pompe de remplissage.
- **Réaffirmation de l'état :** hors `Filling`, un Stop est renvoyé à la pompe de
  remplissage toutes les 10 s (une transaction de plus toutes les 10 s sur le
  câble). Une LabQ qui redémarre seule après une micro-coupure est donc arrêtée
  en 10 s tant que Fermentool tourne.
- **Démarrage du daemon :** la toute première commande envoyée sur le câble est un
  Stop à la pompe de remplissage, avant toute reprise de run.

### 5. Ce que le logiciel ne peut pas garantir

Si Windows plante, si le PC est coupé, ou si le daemon est bloqué pendant
`Filling`, la pompe de remplissage continue. Après une coupure de courant, elle
**repart seule** dès le retour du courant, avant que le PC ait redémarré. Trois
protections, du plus fort au plus faible :

1. **Volume total ≤ contenance de la bouteille pesée.** Si réservoir + bouteille
   pesée ne dépassent jamais 10 L, rien ne peut déborder, même si la pompe ne
   s'arrête jamais. Fermentool le vérifie (§6) ; c'est la seule protection
   absolue.
2. **La marge sous la contenance** (§1) : au moins 10 min de remplissage libres au
   niveau haut.
3. **Le Stop au démarrage du daemon**, qui suppose que Fermentool redémarre seul
   (version installable avec la tâche à l'ouverture de session, « Reste à faire »
   n° 3).

À vérifier sur la LabQ : si son menu permet de désactiver la reprise automatique,
la désactiver **sur la pompe de remplissage seulement** (la pompe d'alimentation
doit reprendre, c'est elle qui nourrit la culture).

### 6. Suivi du réservoir principal

- Au lancement d'un run avec remplissage automatique, champ **« Reservoir
  volume (L) »** ; bouton **« Reservoir refilled »** pendant le run pour le
  remettre à jour.
- Contenu estimé = volume saisi − somme des volumes transférés (pesés).
- **Avertissement au lancement** et à chaque saisie si réservoir + bouteille
  pesée > `bottle_capacity_ml` : « un remplissage qui ne s'arrête pas ferait
  déborder la bouteille pesée ». Il faut cocher une case pour continuer.
- **Alarme `reservoir_low`** quand le réservoir ne suffit plus pour le prochain
  remplissage, avec l'heure prévue de ce remplissage (intégrale de la courbe
  jusqu'à `low_ml`), pour laisser le temps de préparer du milieu.

### 7. Partage du câble RS-485

- Une seule `Pump<T>` et un seul `WatchdogTransport` : les commandes de la pompe
  de remplissage passent par le même transport, à une autre adresse
  (`Pump::with_address(addr, |p| ...)`, qui remet l'adresse de l'alimentation
  ensuite). Tout reste sur le thread de contrôle : pas d'accès concurrent, l'écart
  de 100 ms entre trames est déjà garanti par le transport.
- Trafic ajouté : quelques trames par remplissage, plus un Stop toutes les 10 s.
  La consigne d'alimentation (toutes les 150 ms) n'est pas ralentie de façon
  mesurable.
- L'état du lien de la pompe de remplissage est suivi à part (une adresse qui ne
  répond plus n'est pas une perte du câble) et affiché dans Settings et dans la
  barre latérale.
- Settings, carte « Refill » : **Test** (Start 3 s puis Stop) pour vérifier
  l'adresse et le sens, refusé pendant un run.

### 8. Reprise après plantage

- L'état du cycle est enregistré avec l'état du trim (`PersistedTrim`).
- À la reprise : Stop à la pompe de remplissage d'abord ; si le plantage a eu lieu
  pendant `Filling`, le poids peut avoir beaucoup monté : l'ancre est marquée
  `refill = true`, la masse délivrée pendant la coupure est estimée par `k` comme
  aujourd'hui, puis le cycle repart de `Idle` (un nouveau remplissage démarre si
  le volume est sous `low_ml`).

### 9. UI, journal, notifications

- Barre latérale : « Balance · refilling 3.2 L → 7 L » pendant un remplissage.
- Suivi pesé : chaque remplissage marqué sur le graphique (déjà le cas pour
  l'événement `refill`), avec son débit mesuré.
- Événements : `refill_start`, `refill` (fin, détail lu par `parse_refill`, à
  garder en phase avec `refill_detail`), les alarmes du §4, `reservoir_low`,
  `reservoir_set`.
- Notifications : les alarmes du §4 et `reservoir_low` sonnent jusqu'à
  acquittement ; un remplissage réussi est une info discrète (priorité 2).

## Correctifs du remplissage manuel (sans pompe de remplissage)

Indépendants du remplissage automatique, livrables en premier :

1. **Brancher la stabilité :** `RefillPending` ne passe à `RefillSettling` que
   quand le poids reste dans ±2 pas de balance pendant 20 s (calculé sur la fin
   de `weight_buffer`) et que la balance se dit stable.
2. **Pas de faux remplissage :** si le poids final n'est pas plus haut qu'avant
   (bouteille soulevée puis reposée), traiter comme un contact (`refill = false`).
3. **Garder l'historique du rendement**, comme au §3.
4. **Bouton « Refill bottle » / « Done »** dans l'UI, branché sur les routes
   existantes ; le mode manuel reste actif jusqu'à « Done » ou jusqu'à un poids
   stable plus haut qu'avant, avec une alarme après 15 min.

## Tests

**Simulation (tests Rust, accélérés) :** un banc simulé avec deux `SimPump` sur
un même transport (routage par adresse) et une balance qui modélise la bouteille
(tare + volume, sortie = débit d'alimentation × rendement, entrée = 300 ml/min
quand la pompe de remplissage tourne, bruit de ±1 pas, précision 0,1 g et 1 g).

| Scénario | Attendu |
|---|---|
| Run exponentiel de 100 h, deux remplissages automatiques | c varie de moins de 0,5 % autour de chaque remplissage ; aucune alarme ; volume délivré à 1 % près |
| Réservoir vide au 2ᵉ remplissage | `refill_no_flow` en moins de 35 s, pompe de remplissage arrêtée |
| La pompe de remplissage ignore Stop | `refill_stop_failed`, Stop renvoyé à chaque lecture, `refill_overfill` |
| Plantage du daemon en plein remplissage, reprise | Stop envoyé en premier ; volume délivré cohérent ; nouveau cycle |
| La pompe de remplissage redémarre seule (coupure simulée) hors remplissage | Arrêtée en moins de 10 s |
| Remplissage manuel en trois versements, bouteille soulevée | Un seul événement `refill`, poids justes, c inchangé |

**Banc, à l'eau :** les deux bidons, seuils rapprochés (par exemple remplir de
1 L à 2 L) pour voir plusieurs cycles en une heure ; puis un essai de 12 à 16 h
avec au moins deux remplissages (« Reste à faire » n° 6).

## Livraison par étapes

1. Correctifs du remplissage manuel (utile tout de suite, aucun matériel
   nouveau). **Fait le 2026-10-02**, plus la détection d'une montée lente
   (pompe de transfert pilotée à la main). Écart : la stabilité ne demande
   pas le drapeau « stable » de la balance, qui reste faux tant que la pompe
   d'alimentation tire.
2. Deuxième pompe sur le câble : réglages, `with_address`, test depuis Settings,
   Stop au démarrage et toutes les 10 s.
3. Cycle automatique, régulation pendant le remplissage, garde-fous, reprise
   après plantage.
4. Suivi du réservoir, UI, notifications.
5. Essais au banc.

## Questions ouvertes

1. Menu de la LabQ : la reprise automatique après coupure peut-elle être
   désactivée ?
2. Poids à vide exact de la bouteille pesée, et précision de la balance de 15 kg.
3. Sens de rotation de la pompe de remplissage qui va du réservoir vers la
   bouteille pesée.
4. Niveaux bas et haut définitifs (proposition : 1 L et 7 L).
