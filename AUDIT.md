# Audit du code, Fermentool (2026-10-07)

Méthode : chaque fichier lu en entier, un par un. Les constats sont notés au fil
de la lecture (fichier:ligne, catégorie, gravité, explication, correctif
proposé). Rien n'est corrigé dans le cadre de cet audit.

Gravités : **critique** (perte de dosage, corruption, blocage du daemon en
run), **haute** (comportement faux probable en usage réel), **moyenne** (faux
dans un cas limite plausible, ou ressource qui fuit lentement), **basse**
(robustesse, lisibilité, cas improbable).

## Synthèse

**78 fichiers audités sur 78** dans le périmètre ci-dessous, chacun lu en
entier. **60 constats** : 1 critique, 4 hauts, 17 moyens, 38 bas. Aucune fuite
mémoire franche ni aucune injection SQL : toutes les requêtes sont
paramétrées, les buffers et historiques sont bornés, et les threads sont
détachés volontairement et documentés. Les vrais risques sont **la pompe
laissée dans un état que l'UI ne montre pas**, le **thread de contrôle
bloqué par des attentes lentes**, et l'**API locale exposée aux pages
web**.

### Critique
| # | Où | Problème |
|---|---|---|
| A-10 | `engine/mod.rs:2506` `end_run` (+ `stop_pump`, `discard_recovery`) | Résultat de `pump.stop()` ignoré : un Stop pendant une panne de lien est enregistré, mais la pompe continue sans fin au rebranchement. |

### Haute
| # | Où | Problème |
|---|---|---|
| A-11 | `engine/mod.rs:1285` `start_run` | Pompe démarrée avant `insert_run` : si l'écriture en base échoue, la pompe tourne sans run. |
| A-23 | `api.rs` routes `POST` sans corps | CSRF : n'importe quelle page web peut faire Stop/Abort/Shutdown sur `127.0.0.1:8730`. |
| A-47 | `App.svelte:122`, `api.js` | `app.connected` ne repasse jamais à faux : daemon mort, l'UI affiche "online" et un run qui avance. |
| A-48 | `RunSettings.svelte:171`, `config.rs:162` | "Ask before resuming" n'est lu nulle part : la reprise automatique n'existe pas. |

### Moyenne
| # | Où | Problème |
|---|---|---|
| A-02 | `config.rs:336` | `config.toml` écrit de façon non atomique : un fichier tronqué empêche le daemon de redémarrer. |
| A-03 | `config.rs:310` | Valeurs du fichier non validées (densité 0, adresse 0, limite hors bornes). |
| A-04 | `transport.rs:229` | `respawn` ouvre le nouveau port avant de fermer l'ancien : Connect sur le même port bascule sur le simulateur. |
| A-07 | `control.rs:278` | Une partie de la boucle de contrôle est hors `catch_unwind` : une panique fige le run, HTTP toujours vivant. |
| A-08 | `control.rs:506` | Après un saut d'horloge, ~8 `WARN`/s pour tout le reste du run. |
| A-13 | `transport.rs:125`, `engine` | Le puits `SimPump` laissé en panne fait journaliser `written_ok = 1` et fait "réussir" le Stop. |
| A-14 | `engine/mod.rs:2394` | `reconfigure_scale` dort jusqu'à ~2 s sur le thread de contrôle pendant un run simple. |
| A-24 | `api.rs:76` | Pas de contrôle du `Host` : DNS rebinding possible, avec lecture des secrets ntfy/Teams. |
| A-29 | `notify.rs:590` | Les retries d'envoi bloquent le notifier ~3,5 min par cible : les alarmes partent en retard. |
| A-33 | `modbus/lib.rs:461` | Mode piloté en rpm : une consigne < 0,05 rpm est refusée, la pompe garde son ancienne vitesse au lieu de s'arrêter. |
| A-34 | `modbus/lib.rs:419` | Pompe muette : ~3,2 s par opération sur le thread de contrôle, Stop et `/api/status` attendent. |
| A-38 | `src-tauri/src/daemon.rs:14` | Fenêtre : un `status` > 500 ms fait lancer un second daemon. |
| A-40 | `src-tauri/src/tray.rs:40` | "Shut down daemon" du tray sans confirmation ni contrôle de run actif. |
| A-42 | `installer/hooks.nsh:9` | Installeur : daemon lent pris pour absent, code 3 ignoré, d'où une mise à jour partielle. |
| A-43 | `installer/remove-task.ps1` | La désinstallation coupe le daemon même pendant un run. |
| A-54 | `api.rs:434` | Le suivi des runs passés utilise la densité actuelle, pas celle du run. |
| A-55 | `Overview.svelte:362`, `DaemonSettings.svelte:324` | "Stop run" et "Shut down daemon" sans confirmation. |

### Basse
A-01, A-05, A-06, A-09, A-12, A-15 à A-22, A-25 à A-28, A-30 à A-32,
A-35 à A-37, A-39, A-41, A-44 à A-46, A-49 à A-53, A-56 à A-60 : voir le
détail plus bas (robustesse, petites incohérences, performance non
critique, documentation).

### Ordre de correction suggéré
1. **A-10 + A-13** ensemble (un vrai "puits en panne" qui renvoie des
   erreurs, et un Stop en attente rejoué à la reconnexion).
2. **A-47** (statut périmé) et **A-55** (confirmations) : peu de code, gros
   gain de sûreté opérateur.
3. **A-23 + A-24 + A-25** : un seul middleware (contrôle de `Origin`, de
   `Host` et d'un en-tête obligatoire).
4. **A-48** : implémenter la reprise automatique ou retirer la case.
5. **A-11, A-33, A-34, A-14** : chemins pompe et thread de contrôle.
6. Le reste au fil de l'eau.

## Suivi des corrections (2026-10-07)

Les 60 points ont été traités en 8 lots. Chaque lot a été compilé et testé
seul avant son commit : 345 tests au total, dont 37 nouveaux qui
reproduisent les bugs corrigés.

| Lot | Commit | Points |
|---|---|---|
| Arrêt de la pompe, lien mort | `0cf1154` | A-10, A-11, A-12, A-13, A-17, A-18, A-33, A-34, A-35 |
| Thread de contrôle, reprise auto | `2ba0659` | A-01, A-04, A-06, A-07, A-08, A-09, A-14, A-15, A-37, A-48 |
| Config et données | `03e8eac` | A-02, A-03, A-16, A-19, A-20, A-21, A-22, A-26, A-27, A-28, A-36, A-54 |
| Sécurité de l'API | `19364fc` | A-23, A-24, A-25 (et `/api/health` pour A-38) |
| Notifications | `fd24599` | A-29, A-30 |
| Bureau et installeur | `a85c870` | A-05 (documenté), A-38, A-39, A-40, A-41, A-42, A-43, A-44, A-45 (documenté) |
| Interface | `5690950` | A-46, A-47, A-49, A-50, A-51, A-52, A-53, A-55, A-56, A-57, A-58, A-59, A-60 |
| Outillage, doc | `7bb3047` | A-31, A-32 |

Restent des limites, pas des bugs, documentées dans `docs/RELEASE.md` :
- A-05 : un pilote USB-série bloqué coûte un thread abandonné ;
- A-39 : port 8730 fixe pour l'application de bureau ;
- A-45 : démarrage à l'ouverture de session.

Non vérifié ici :
- `hooks.nsh` n'a pas été compilé (NSIS n'est pas installé sur ce PC). À
  vérifier au prochain `cargo tauri build`, en testant une mise à jour et une
  désinstallation pendant un run.
- Les comportements matériels (Stop en attente au rebranchement, pause sous
  0,1 rpm en mode piloté, reprise automatique après coupure) sont couverts par
  des tests sur simulateur ; un essai au banc reste à faire.

## Périmètre (78 fichiers)

Exclus : `ui/dist/` (build), `docs/` et `*.md`, lockfiles, PDF, images,
`.gitignore`, `.gitattributes`, `.editorconfig`, `.vscode/`, `.claude/`,
`ui/.nvmrc`.

### Rust, daemon (`crates/fermentool-core`)
- [x] src/lib.rs
- [x] src/main.rs
- [x] src/config.rs
- [x] src/transport.rs
- [x] src/scale.rs
- [x] src/control.rs
- [x] src/engine/mod.rs
- [x] src/tracking.rs
- [x] src/trim.rs
- [x] src/store/mod.rs
- [x] src/store/migrations/0001_init.sql
- [x] src/store/migrations/0002_gravimetric_trim.sql
- [x] src/store/migrations/0003_tubing_calibration.sql
- [x] src/store/migrations/0004_tick_weight.sql
- [x] src/store/migrations/0005_calibration_archive.sql
- [x] src/store/migrations/0006_run_responsible.sql
- [x] src/api.rs
- [x] src/notify.rs
- [x] Cargo.toml

### Rust, bibliothèques
- [x] crates/fermentool-modbus/src/lib.rs
- [x] crates/fermentool-modbus/Cargo.toml
- [x] crates/fermentool-curves/src/lib.rs
- [x] crates/fermentool-curves/Cargo.toml

### Racine
- [x] Cargo.toml
- [x] .cargo/config.toml
- [x] rust-toolchain.toml
- [x] config.example.toml
- [x] logos/preview.html

### Tauri (`src-tauri`)
- [x] Cargo.toml
- [x] build.rs
- [x] src/main.rs
- [x] src/daemon.rs
- [x] src/tray.rs
- [x] tauri.conf.json
- [x] capabilities/default.json
- [x] installer/hooks.nsh
- [x] installer/fermentool-task.xml
- [x] installer/setup-task.ps1
- [x] installer/remove-task.ps1
- [x] scripts/copy-sidecar.ps1

### UI (`ui/`)
- [x] index.html
- [x] package.json
- [x] jsconfig.json
- [x] svelte.config.js
- [x] vite.config.js
- [x] src/main.js
- [x] src/App.svelte
- [x] src/app.css
- [x] src/lib/api.js
- [x] src/lib/balance.js
- [x] src/lib/config.js
- [x] src/lib/fmt.js
- [x] src/lib/link.js
- [x] src/lib/runfile.js
- [x] src/lib/state.svelte.js
- [x] src/components/Chart.svelte
- [x] src/components/ConnBar.svelte
- [x] src/components/ConnectControl.svelte
- [x] src/components/ErrorText.svelte
- [x] src/components/FigureBand.svelte
- [x] src/components/FinishModal.svelte
- [x] src/components/Icon.svelte
- [x] src/components/PumpHead.svelte
- [x] src/components/ResumeModal.svelte
- [x] src/components/TrackingPanel.svelte
- [x] src/routes/Overview.svelte
- [x] src/routes/NewRun.svelte
- [x] src/routes/History.svelte
- [x] src/routes/TubingCalibration.svelte
- [x] src/routes/Settings.svelte
- [x] src/routes/settings/sections.js
- [x] src/routes/settings/settings.css
- [x] src/routes/settings/SettingsCard.svelte
- [x] src/routes/settings/BalanceSettings.svelte
- [x] src/routes/settings/DaemonSettings.svelte
- [x] src/routes/settings/NotificationSettings.svelte
- [x] src/routes/settings/PumpSettings.svelte
- [x] src/routes/settings/RunSettings.svelte

## Constats

<!-- Ajoutés au fil de la lecture, dans l'ordre des fichiers. -->

### crates/fermentool-core/src/main.rs

**A-01** · `main.rs:214-259` · Concurrence / ressources · **basse**
Une deuxième instance (double-clic, tâche planifiée + fenêtre Tauri) construit
tout avant de découvrir que le port 8730 est pris : elle ouvre la base, tente
le port série et la balance, démarre le thread de contrôle et le notifier,
puis s'arrête sur `AddrInUse`. Pendant ce court instant, `set_serial` écrit
dans la base partagée un événement `serial_lost` ("COM8 not open at
startup"), car le vrai daemon tient le port. Le journal de l'instance réelle
reçoit donc une fausse erreur.
*Correctif* : faire le `bind` (ou prendre un verrou d'instance unique) en
premier, avant toute ouverture de port ou de base.

### crates/fermentool-core/src/config.rs

**A-02** · `config.rs:336-342` (`save`) · Gestion d'erreurs / robustesse · **moyenne**
`std::fs::write` n'est pas atomique. Une coupure de courant ou un crash
pendant une sauvegarde depuis Settings peut laisser `config.toml` tronqué.
Au démarrage suivant, `load_or_create` échoue (`Parse`) et `main` s'arrête :
le daemon, sans fenêtre, ne redémarre plus du tout, et la reprise après
crash d'un run en cours devient impossible tant que personne ne répare le
fichier à la main.
*Correctif* : écrire dans `config.toml.tmp`, `sync_all`, puis `rename`.
Au démarrage, en cas d'erreur de parse, garder une copie `.bak`, journaliser
fort et repartir des valeurs par défaut plutôt que de refuser de démarrer.

**A-03** · `config.rs:310-333` · Entrées non validées · **moyenne**
Les valeurs lues depuis le fichier ne sont jamais validées (seule l'API
valide ce qu'elle reçoit) : `density_g_per_ml` à 0 ou négatif (divisions
`g / density` dans `Engine::status`, `tracking_report`), `trim_limit_pct`
hors [5, 100] (bornes du trim inversées ou absurdes), `pump.address` à 0
(adresse broadcast MODBUS, aucune réponse) ou > 247, `baud` quelconque.
Un `config.toml` édité à la main suffit.
*Correctif* : une `Config::validate()` appelée au chargement, qui ramène
chaque champ hors domaine à sa valeur par défaut avec un `warn!`.

### crates/fermentool-core/src/transport.rs

**A-04** · `transport.rs:229-238` (`respawn`) · Concurrence / ressources · **moyenne**
Le nouveau worker est créé, et ouvre le port, **avant** que l'ancien ne
reçoive `Shutdown`. Sous Windows un port COM est exclusif : rouvrir le même
port échoue tant que l'ancien worker le tient. Conséquences :
- `Reconnect` (opérateur) sur le port déjà ouvert : `swap` retombe sur le
  **simulateur**, l'UI affiche "could not open COM8, running on the pump
  simulator" et l'alarme `serial_lost` est journalisée ;
- la reprise automatique (`swap_strict`) rate systématiquement sa première
  tentative et ne réussit qu'au retry suivant (≥ 1 s plus tard).
Le commentaire de `impl SwapTransport for Box<dyn Transport>` décrit
précisément ce piège, mais `WatchdogTransport` le réintroduit.
*Correctif* : envoyer `Shutdown` d'abord, attendre (borné, ~500 ms) un
accusé de fermeture du worker sain, puis lancer le nouveau. Un worker
bloqué reste détaché comme aujourd'hui.

**A-05** · `transport.rs:160-183` · Ressources · **basse** (connu, accepté)
Chaque blocage d'un syscall série laisse un thread `serial-worker` détaché
avec son handle de port. Le commentaire l'assume ("bounded leak, one thread
per wedge event"). Rien à corriger, à garder en tête si des débranchements
répétés sont observés sur un même run.

### crates/fermentool-core/src/scale.rs

**A-06** · `scale.rs:91-101`, `engine/mod.rs:2425-2431` · Concurrence / ressources · **basse**
Après `drop` d'un `PolledScale`, le thread de poll peut encore tenir le port
COM jusqu'à la fin de sa transaction en cours (jusqu'à 1,5 s). La pause de
300 ms de `reconfigure_scale` avant la réouverture ne suffit pas toujours :
la réouverture échoue, Settings répond "not connected" alors que la balance
se reconnecte quelques secondes plus tard grâce au retry.
*Correctif* : que `PolledScale` expose une attente de fin du thread
(canal "closed"), avec un délai borné, au lieu d'une pause fixe.

### crates/fermentool-core/src/control.rs

**A-07** · `control.rs:278-465` · Gestion d'erreurs / robustesse · **moyenne**
Seuls `tick`, `apply_setpoint`, `scale_tick`, `probe_*`, `recover_serial`,
`recover_scale` et `handle` sont protégés par `catch_unwind`. Le reste de
l'itération ne l'est pas : `engine.status()` en tête de boucle,
`current_status`, `burst_end`, le reste de `maybe_recover_scale`
(`journal`, `scale_link_up`). Une panique à ces endroits termine le thread
`fermentool-control` sans arrêter le process. Le serveur HTTP reste en vie
mais chaque commande répond `ControlDown`, plus rien n'est journalisé ni
alarmé, et la pompe reste indéfiniment à sa dernière consigne. C'est le pire
mode de panne pour un run de 100 h sans surveillance, même si la probabilité
est faible.
*Correctif* : envelopper le corps entier de l'itération dans
`catch_unwind`. Si le thread se termine malgré tout, quitter le process
(`std::process::exit(1)`) pour que la tâche planifiée `RestartOnFailure`
relance le daemon et que la reprise après crash prenne le relais.

**A-08** · `control.rs:506-523` (`run_now`) · Logique / performance (logs) · **moyenne**
Après un saut d'horloge système > 5 s (resynchro NTP), l'ancre n'est jamais
recalée : chaque appel suivant (tick 1/s **et** `apply_setpoint` ~7/s)
repasse dans la branche "stepped" et écrit un `WARN`. Pour le reste du run,
cela fait ~8 lignes/s, soit ~2,9 millions de lignes sur 100 h, qui noient
les vrais avertissements et gonflent les logs (gardés 30 jours).
*Correctif* : mémoriser le décalage mesuré au premier saut, journaliser
une seule fois (sur front), puis appliquer ce décalage en silence.

**A-09** · `control.rs:185-211, 280, 346, 405, 431` · Performance · **basse**
`engine.status()` est reconstruit (clones de `String`, `HoldingStatus`)
jusqu'à 4 fois par itération. Au repos, chaque diffusion de statut appelle
aussi `pending_recovery`, soit 2 requêtes SQL sur le thread de contrôle.
C'est négligeable aujourd'hui, mais c'est la même famille de coûts que la
règle "le thread de contrôle ne doit jamais attendre".
*Correctif* : un seul `status()` par itération, réutilisé.

### crates/fermentool-core/src/engine/mod.rs

**A-10** · `engine/mod.rs:2506-2521` (`end_run`), `:2490-2504` (`stop_pump`), `:2725` (`discard_recovery`) · Gestion d'erreurs · **critique**
Le résultat de `self.pump.stop()` est ignoré (`let _ =`). Si la trame Stop
échoue, le run est quand même enregistré `stopped`/`aborted`, `active`
passe à `None` et plus rien ne retente l'arrêt : la pompe continue à sa
dernière consigne pendant que l'UI affiche "idle".
Scénario concret : le câble est débranché, la reprise automatique a laissé
le puits `SimPump`, et l'opérateur fait Stop. Le `stop()` "réussit" sur ce
puits. Au rebranchement, `recover_serial` rouvre le port mais personne
n'envoie Stop à la vraie pompe, qui dose sans fin. Même effet avec un
`WatchdogTransport` en état `stuck`, qui échoue immédiatement.
*Correctif* : vérifier le résultat. En cas d'échec, ou si le lien est
`serial_lost`, garder un état "arrêt en attente", visible dans le statut
(alarme rouge dans l'UI), que la boucle de contrôle renvoie à chaque
reconnexion jusqu'à confirmation par relecture (`read_speed_rpm == 0`, ou
le registre run/stop).

**A-11** · `engine/mod.rs:1285-1314` (`start_run`) · Gestion d'erreurs / ressources · **haute**
La pompe est démarrée (`set_direction`, consigne, `start`) **avant**
`insert_run` et `log_event("start")`. Si l'un de ces deux appels échoue
(disque plein, base verrouillée au-delà du `busy_timeout` de 5 s), la
fonction renvoie une erreur alors que la pompe tourne, sans run actif. Si
c'est `log_event` qui échoue, la ligne `running` existe en plus en base et
sera proposée en "reprise après crash" au prochain démarrage.
*Correctif* : en cas d'échec après `pump.start()`, envoyer `stop()` et
marquer la ligne éventuellement insérée comme `aborted`, ou insérer la
ligne avant de démarrer la pompe, dans une transaction.

**A-12** · `engine/mod.rs:1212-1250` (`start_run`) · Logique · **basse**
L'état est modifié et persisté (reset du trim, `save_trim_state`,
`set_holding(None)`) **avant** les validations qui peuvent encore refuser
le run (`spec.validate`, contrôle du débit max, erreurs pompe). Une
tentative refusée efface donc le trim persisté et un éventuel "hold" hérité
d'un ancien daemon, alors que la pompe peut encore tenir ce hold.
*Correctif* : valider d'abord, muter ensuite.

**A-13** · `engine/mod.rs:112-129` + `transport.rs:125-127, 314` · Logique (intégrité du journal) · **moyenne**
Quand `swap_strict` échoue, il laisse un `SimPump` "puits" en place alors
que `self.transport` reste `Serial(..)`. Toutes les écritures "réussissent"
sur ce puits et la relecture lit la valeur du simulateur. Pendant toute la
panne, le journal enregistre donc `written_ok = 1`, `pump_confirmed` reste
vrai, et l'export CSV affirme que la pompe a reçu ses consignes. C'est
aussi ce qui fait "réussir" le Stop de A-10.
*Correctif* : un puits dédié qui renvoie `TransportError::Io("link down")`
à chaque transaction (la pompe garde physiquement sa dernière consigne
dans tous les cas).

**A-14** · `engine/mod.rs:2394-2444` (`reconfigure_scale`) · Performance (I/O bloquante sur thread critique) · **moyenne**
Hors run avec trim (donc **pendant un run de dosage simple**),
`reconfigure_scale` dort 300 ms puis boucle jusqu'à ~1,8 s en attendant
une pesée, sur le thread de contrôle. Pendant ce temps, ni les écritures
de consigne toutes les 150 ms ni le tick de 1 s ne passent. Cela viole la
règle du projet ("une attente lente bloque toute l'UI").
*Correctif* : répondre tout de suite ("connecting…") et laisser la
confirmation à la boucle (`scale_link_up` dans le statut), ou attendre
hors du thread de contrôle.

**A-15** · `engine/mod.rs:2685-2707` (`reconstruct_volume_ml`) · Performance · **basse**
La reprise charge **toutes** les lignes de tick du run en mémoire, sur le
thread de contrôle : environ 360 000 lignes pour un run de 100 h, plusieurs
dizaines de Mo et une pause sensible au moment de la reprise.
*Correctif* : intégrer en SQL (somme des trapèzes avec `LAG()`), ou lire
par pages.

**A-16** · `engine/mod.rs:629, 654` · Logique · **basse**
`Tracker.points` et `Tracker.c_hist` sont `#[serde(skip)]`. Après une
reprise, une alarme qui se déclenche dans l'heure ne peut pas remettre c à
sa valeur d'avant l'incident (`c_at` ne trouve rien) : elle retombe sur
`c_seed`.
*Correctif* : persister `c_hist`, borné à une heure et décimé, avec le trim.

**A-17** · `engine/mod.rs:1260-1272` · Logique (cas limite) · **basse**
Le contrôle "la courbe dépasse 350 rpm" échantillonne 201 points. Un pic
étroit d'une courbe `Custom` ou `Step` entre deux échantillons passe le
contrôle, puis `drive_rpm` plafonne silencieusement à 350 rpm.
*Correctif* : évaluer aussi chaque point de rupture de la courbe (sommets
`Custom`, marches `Step`).

**A-18** · `engine/mod.rs:2845-2847` (`calibration_ml_per_rpm`) · Logique (division) · **basse** (sous réserve de `store`)
`mean_measured_ml_min / setpoint` n'est pas protégé. Avec une calibration à
`setpoint` 0, le ratio vaut ∞, `drive_rpm` donne 0 rpm et le run démarre
pompe à l'arrêt, puisque le contrôle de pic (`peak / ∞ = 0`) passe. Avec
une moyenne mesurée nulle (tube non amorcé), le ratio vaut 0 et `drive_grid`
donne 0, d'où des quantifications `NaN`. Le contrôle de pic refuse le
départ dans ce second cas, mais pas la reprise.
*Vérifié ensuite dans `store/mod.rs:547-552`* : `insert_calibration` refuse
un `setpoint` ou des poids ≤ 0 ou non finis. Ce cas n'est donc pas
atteignable par l'API, seulement par une base modifiée à la main. Cela
reste de la défense en profondeur.
*Correctif* : refuser une calibration au ratio non fini ou ≤ 0, au
démarrage comme à la reprise.

### crates/fermentool-core/src/tracking.rs

**A-19** · `tracking.rs:362-388` (`fit_exponential_mu`) · Logique (overflow) · **basse**
`((mu * t).exp() - 1.0) / mu` déborde vers `inf` dès que `µ·t > ~709`. Sur
un run long avec un µ demandé élevé (borne haute de recherche `4·µ`, par
exemple 4 × 2 h⁻¹ × 100 h), `gg = inf`, `f0 = inf/inf = NaN`, `sse = NaN`.
Les comparaisons `NaN < NaN` sont fausses, la section dorée dérive et
renvoie un µ "livré" arbitraire, affiché dans History.
*Correctif* : borner la recherche pour que `µ_max · t_max < 700`, ou
travailler en log (`ln V`), et renvoyer `None` si `sse` n'est pas fini.

### crates/fermentool-core/src/trim.rs

**A-20** · `trim.rs:38` (`theil_sen_slope`) · Gestion d'erreurs (panique) · **basse**
`partial_cmp(b).unwrap()` panique au premier `NaN`. Une densité `nan`
(TOML accepte `density_g_per_ml = nan`, voir A-03) ou toute autre
non-finitude dans les points provoque une panique à chaque mise à jour du
trim. `catch_unwind` l'absorbe, mais le trim ne régule plus jamais et le log
reçoit une erreur toutes les 10 s.
*Correctif* : `total_cmp`, et filtrer les pentes non finies.

### crates/fermentool-core/src/store/mod.rs

**A-21** · `store/mod.rs:793-811` + `engine/mod.rs:1711` · Logique · **basse**
`events()` renvoie les N plus **récents**. `tracking_report` en prend
10 000 : sur un run dont le journal d'événements dépasse ce nombre (rafales
de `write_fail` sur un bus bruité, par exemple), les plus anciens
marqueurs (`trim_start`, `trim_ratio`, premiers `refill`) disparaissent du
graphe et du tableau des recharges, sans aucun message.
*Correctif* : une requête dédiée filtrée sur les `kind` utiles
(`WHERE run_id = ? AND kind IN (...)`), sans limite ou avec une limite
bien plus haute.

**A-22** · `store/mod.rs:383-393` · Robustesse · **basse**
`integrity_check()` existe mais n'est jamais appelé. Une base abîmée (disque
défaillant ; les notes de session mentionnent des doutes sur le disque et la
RAM de ce PC) n'est détectée qu'à la première requête qui échoue, parfois
en plein run.
*Correctif* : `PRAGMA quick_check` au démarrage, journalisé, et une
alarme visible dans le statut si le résultat n'est pas `ok`.

### crates/fermentool-core/src/store/migrations/*.sql

Aucun problème. Index présents (`ix_ticks_run_seq` unique,
`ix_events_run`, index partiel "un seul run `running`"), contraintes
`CHECK` cohérentes avec les énumérations Rust, colonnes ajoutées nullable
ou avec valeur par défaut.

### crates/fermentool-core/src/api.rs

**A-23** · `api.rs:87-96, 104-105, 109` · Sécurité (CSRF) · **haute**
Le CORS n'empêche pas une page web d'**émettre** une requête : il l'empêche
seulement d'en **lire** la réponse. Les routes `POST` sans corps JSON ne
déclenchent pas de pré-vol, elles sont donc exécutables depuis n'importe
quel site ouvert dans le navigateur du PC de labo
(`fetch("http://127.0.0.1:8730/api/shutdown", {method: "POST", mode: "no-cors"})`,
ou un simple `<form method=post>`) :
`/api/shutdown`, `/api/runs/{id}/stop`, `/api/runs/{id}/abort`,
`/api/pump/stop`, `/api/recovery/resume`, `/api/scale/refill_mode`,
`/api/scale/refill_done`, `/api/calibrations/{id}/archive|restore`.
Une page malveillante ou compromise peut donc arrêter, avorter ou couper le
daemon pendant un run de 100 h. Les routes à corps JSON (`POST /api/runs`,
`PUT /api/config`, `POST /api/serial/reconnect`) et les `DELETE` sont
protégées par le pré-vol.
*Correctif* : rejeter toute requête mutante dont l'en-tête `Origin` est
présent et hors de la liste autorisée (même origine + origines Tauri), et
exiger un en-tête personnalisé (`X-Fermentool: 1`) sur toutes les
méthodes non-GET, ce qui force un pré-vol.

**A-24** · `api.rs:76-114` · Sécurité (DNS rebinding) · **moyenne**
Aucune vérification de l'en-tête `Host`. Par DNS rebinding, un site distant
peut se faire passer pour la même origine que `127.0.0.1:8730` et obtenir un
accès complet : lire `GET /api/config`, qui renvoie les **secrets** (topics
ntfy, webhooks Teams), démarrer des runs, réécrire la configuration
(`PUT /api/config`).
*Correctif* : un middleware qui refuse (403) tout `Host` différent de
`127.0.0.1:<port>`, `localhost:<port>` et `tauri.localhost`.

**A-25** · `api.rs:687-689` (`ws_upgrade`) · Sécurité (fuite d'information) · **basse**
Les WebSockets ne sont pas soumis au CORS et `ws_upgrade` ne vérifie pas
l'`Origin`. N'importe quelle page peut ouvrir `ws://127.0.0.1:8730/api/ws`
et recevoir en continu le statut : nom du run, poids, débit, état des
alarmes.
*Correctif* : refuser l'upgrade si `Origin` n'est pas dans la liste
autorisée.

**A-26** · `api.rs:199-250, 823-861` · Concurrence (perte de mise à jour) / validation · **basse**
`put_config` et `serial_reconnect` font chacun "lire toute la config,
modifier, réécrire tout le fichier", sans contrôle de version. Une page
Settings ouverte avant un `Reconnect` réécrit l'ancien `serial.path` à sa
sauvegarde. `put_config` ne valide pas non plus `port`, `pump.address`
(alors que `serial_reconnect` vérifie 1..=247), `serial.baud` ni
`log.level`.
*Correctif* : mettre à jour uniquement les sections modifiées (merge
côté serveur) ou envoyer un numéro de version, et partager une seule
fonction de validation (voir A-03).

**A-27** · `api.rs:499-515` · Logique · **basse**
`stop_run` et `abort_run` ignorent l'`id` du chemin (`Path(_id)`) : un onglet
resté sur un ancien run qui envoie `POST /api/runs/41/stop` arrête le run
actif quel qu'il soit.
*Correctif* : renvoyer 409 si `id` n'est pas le run actif.

**A-28** · `api.rs:381-408` · Logique (intégrité du journal) · **basse**
`RunConfig.pump_addr` vient du client et n'est qu'enregistré dans la ligne
du run. Le moteur pilote l'adresse configurée (`Pump::set_address`). Le
journal peut donc indiquer une adresse que la pompe n'a jamais eue.
*Correctif* : ignorer la valeur du client et enregistrer l'adresse
réellement pilotée.

### crates/fermentool-core/src/notify.rs

**A-29** · `notify.rs:590-607` (`handle`) · Performance / disponibilité des alarmes · **moyenne**
Les nouvelles tentatives d'envoi sont faites **en ligne** sur le thread
`notifier` : jusqu'à 4 × 15 s de timeout plus 5 + 30 + 120 s d'attente, soit
~3,5 min **par cible** et par événement. Si ntfy ou Teams est injoignable,
chaque événement notable bloque la boucle aussi longtemps : les événements
suivants, **y compris les alarmes**, les rappels (`ring`) et la lecture
des acquittements attendent derrière. Avec quelques événements en file,
une alarme part avec des dizaines de minutes de retard.
*Correctif* : une file d'envoi par cible, avec ses propres échéances de
retry, consultée à chaque passe sans bloquer. Envoyer les alarmes avant les
infos.

**A-30** · `notify.rs:415-418` · Ressources · **basse**
`link_said` (deux clés par run) et `ack_polled` (une clé par topic)
grossissent pendant toute la vie du process sans jamais être purgés.
L'effet est négligeable (quelques octets par run), mais la croissance n'est
pas bornée.
*Correctif* : purger les entrées plus vieilles que `LINK_QUIET` ou
`ACK_POLL` à chaque passe.

### Cargo.toml (racine, core, curves, modbus), .cargo/config.toml, rust-toolchain.toml, config.example.toml

**A-31** · `rust-toolchain.toml:2` · Reproductibilité · **basse**
`channel = "stable"` n'est pas épinglé : deux exports du même commit
peuvent sortir de compilateurs différents.
*Correctif* : épingler une version (`channel = "1.xx.0"`).

**A-32** · `config.example.toml` · Documentation · **basse**
L'exemple est en retard sur `Config` : il manque `serial.allow_simulator`,
`scale.position`, `scale.trim_limit_pct` et la section `[notify]`. Il
présente aussi `resume.grace_minutes` comme "still resumable this long past
the planned end", alors qu'un run de dosage se reprend désormais à tout
moment (dans sa phase de maintien) et que ce délai ne vaut plus que pour
les bursts de calibration. Le commentaire de
`crates/fermentool-core/Cargo.toml:35` ("Later milestones add: tower-http,
rust-embed") est aussi périmé.
*Correctif* : régénérer l'exemple depuis `Config::default().to_toml()`
et le commenter.

### crates/fermentool-modbus/src/lib.rs

**A-33** · `fermentool-modbus/src/lib.rs:461-466` + `engine/mod.rs:2780-2782` · Logique (cas limite) · **moyenne**
`set_speed_rpm` refuse tout ce qui est < 0,1 rpm (`OutOfRange`). Or un run
en ml/min **piloté en rpm** (`drive_ml_per_rpm`) convertit sa consigne par
`drive_rpm`, qui arrondit au 0,1 rpm et borne à `[0, 350]` : toute consigne
inférieure à 0,05 rpm-équivalent donne **0 rpm**. Par exemple, avec un tube
à 2,4 mL/tr, tout ce qui est sous ~0,12 mL/min.
- Au départ, une courbe qui commence à 0 ou très bas est refusée avec
  "pump: value out of range: motor speed must be 0.1..=350 rpm", sans
  explication (ce n'est pas un `PumpRejectedSetpoint`, pas de `hint`).
- **En cours de run**, une courbe qui descend vers 0 (une marche `Step` à 0
  pour couper l'alimentation, une `Custom` qui finit à 0) fait échouer
  chaque écriture : la pompe **garde sa dernière vitesse non nulle** au lieu
  de ralentir ou de s'arrêter, et le journal se remplit de `write_fail`.
*Correctif* : en mode piloté, traiter une consigne < `RPM_MIN` comme
"pompe arrêtée" (registre start/stop) ou la borner à `RPM_MIN` avec un
avertissement. Le vérifier dès `start_run`, avec un message explicite.

**A-34** · `fermentool-modbus/src/lib.rs:419-424, 761-775` + `engine/mod.rs:984` · Performance (attentes lentes sur le thread de contrôle) · **moyenne**
Avec une pompe éteinte mais l'adaptateur USB branché (le port s'ouvre, rien
ne répond), chaque opération pompe coûte 2 × (1,5 s de timeout + 100 ms
d'écart) ≈ 3,2 s, à cause du retry de `transact_retrying`. `recover_serial`
enchaîne `confirm_read() || confirm_read()`, soit ~6,4 s, plus l'ouverture.
Pendant un run, `tick` et `apply_setpoint` continuent d'écrire vers ce port
muet (le `swap_strict` a réussi, puisque le port s'ouvre). Le thread de
contrôle passe donc l'essentiel de son temps en timeouts, et `/api/status`
comme **Stop** attendent plusieurs secondes derrière. C'est exactement le
problème corrigé pour la balance avec `PolledScale`, transposé à la pompe ;
`WatchdogTransport` ne protège que d'un syscall bloqué, pas d'un périphérique
lent.
*Correctif* : tant que `serial_lost` est vrai, ne plus écrire sur le port
(court-circuit dans `tick`/`apply_setpoint`, la pompe garde physiquement sa
consigne) et confirmer le lien avec une seule lecture sans retry. À terme,
un worker série qui sert une file de commandes et rend la main
immédiatement, comme `PolledScale`.

**A-35** · `fermentool-modbus/src/lib.rs:801-829` · Logique (désynchronisation) · **basse**
Une réponse `03H` ne rappelle pas le registre lu. Une réponse **tardive** à
une lecture précédente (arrivée après le `clear(Input)` mais avant la
nouvelle réponse) est acceptée comme la valeur du registre demandé
maintenant : vitesse lue comme débit, ou l'inverse. Les écritures sont
protégées par la vérification de l'écho, pas les lectures.
*Correctif* : après la réponse attendue, vider le tampon d'entrée et, sur
une relecture qui sort de la tolérance, relire une fois avant de compter un
`readback_fail`.

**A-36** · `api.rs:791-806` → `fermentool-modbus/src/lib.rs:694` · Performance · **basse**
`available_ports()` (énumération SetupAPI sous Windows, parfois des
centaines de ms) est appelé directement dans un handler `async`, sans
`spawn_blocking`, et occupe un worker tokio pendant ce temps.
*Correctif* : `tokio::task::spawn_blocking`.

### crates/fermentool-curves/src/lib.rs

**A-37** · `fermentool-curves/src/lib.rs:364-390` (`validate`) + `engine/mod.rs:1274`, `control.rs:478` · Entrées non validées / overflow · **basse**
`validate` ne borne pas `duration`. Une durée > `i64::MAX` secondes
(possible via l'API, `u64` en JSON) devient **négative** au cast
`as_secs() as i64`. Le run passe alors aussitôt en `curve_done`, et
`burst_end` fait `SignedDuration::from_secs(négatif) - elapsed`, qui peut
paniquer par dépassement **hors** `catch_unwind` (voir A-07), ce qui tue
le thread de contrôle. Ce n'est pas accessible depuis l'UI, seulement par
une requête forgée.
*Correctif* : refuser une `duration` au-delà d'une borne réaliste
(par exemple 30 jours).

### logos/preview.html

Lu en entier : page statique de prévisualisation des logos, avec un
`onclick` inline qui bascule le thème clair/sombre. Hors application.
Aucun problème.

### src-tauri/src/main.rs, src-tauri/src/daemon.rs

**A-38** · `src-tauri/src/daemon.rs:14-20` + `main.rs:29-39` · Concurrence / logique · **moyenne**
La fenêtre décide qu'il n'y a "pas de daemon" si `GET /api/status` ne
répond pas en **500 ms**. Or `/api/status` passe par le thread de contrôle,
qui peut être occupé plus longtemps (A-14, A-34, une reprise de port). Dans
ce cas, ouvrir la fenêtre lance un **second** `fermentool-core` détaché.
Celui-ci va jusqu'à A-01 (base ouverte, port série refusé, faux
`serial_lost` journalisé, notifier lancé) avant de mourir sur le port HTTP
occupé.
*Correctif* : une route de santé qui ne passe pas par le thread de
contrôle (`/api/health`, réponse immédiate dans axum), un délai plus long,
et, côté daemon, le verrou d'instance unique de A-01.

**A-39** · `src-tauri/src/daemon.rs:11`, `tauri.conf.json:24`, `installer/hooks.nsh:9`, `installer/remove-task.ps1:7` · Configuration · **basse**
Le port `8730` est écrit en dur dans la fenêtre, la CSP et les deux scripts
d'installation, alors que `config.toml` permet de le changer. Avec un autre
port, la fenêtre ne trouve plus le daemon et en relance un (voir A-38), et
l'installeur ne protège plus un run actif (il croit le daemon arrêté).
*Correctif* : documenter le port comme fixe pour l'application de bureau,
ou le lire depuis `%APPDATA%\Fermentool\config.toml`.

### src-tauri/src/tray.rs

**A-40** · `src-tauri/src/tray.rs:40-43` · Logique / sûreté · **moyenne**
"Shut down daemon & quit" envoie `POST /api/shutdown` sans vérifier
qu'un run est actif ni demander confirmation. L'arrêt du daemon ne stoppe
pas la pompe (le run reste `running` pour la reprise) : la pompe continue
à sa dernière consigne, **sans courbe, sans trim ni alarme**. La tâche
planifiée ne relance pas un daemon sorti proprement (elle ne relance qu'en
cas d'échec). Un clic de travers dans le menu du tray fige donc un run de
fed-batch jusqu'au prochain démarrage manuel.
*Correctif* : lire `/api/status` et, si un run est actif, demander une
confirmation explicite ("la pompe restera à X ml/min sans régulation"),
ou refuser.

### src-tauri/Cargo.toml, build.rs, tauri.conf.json, capabilities/default.json

**A-41** · `src-tauri/Cargo.toml:19-20`, `src-tauri/src/main.rs:19-20` · Sécurité (surface inutile) · **basse**
`tauri-plugin-shell` et `tauri-plugin-process` sont initialisés alors que
l'UI n'utilise aucune API JS Tauri (voir `capabilities/default.json`). Sans
permission ils sont inertes, mais ils élargissent la surface et la taille
du binaire pour rien.
*Correctif* : les retirer. Le spawn du sidecar passe par
`std::process::Command`, pas par le plugin.

CSP (`tauri.conf.json:24`) et capabilities (`core:default`) : corrects et
restrictifs.

### src-tauri/installer/hooks.nsh

**A-42** · `installer/hooks.nsh:9-13` · Gestion d'erreurs · **moyenne**
1. `try { status } catch { exit 0 }` : un daemon qui ne répond pas en 2 s
   (thread de contrôle occupé, voir A-34) est pris pour "pas de daemon",
   et l'installation continue **pendant un run actif**.
2. Le code de sortie `3` ("le daemon ne s'est pas arrêté en 10 s") n'est
   pas traité : seul `2` provoque un `Abort`. L'installeur poursuit donc
   avec un exe verrouillé, ce qui produit exactement la mise à jour
   partielle silencieuse que ce hook devait empêcher (cf. CLAUDE.md).
*Correctif* : traiter `3` (message + `Abort`). Sur échec de `status`,
regarder aussi `Get-Process fermentool-core` avant de conclure qu'aucun
daemon ne tourne.

### src-tauri/installer/remove-task.ps1

**A-43** · `installer/remove-task.ps1:5-8` · Logique / sûreté · **moyenne**
La désinstallation arrête le daemon **sans vérifier qu'un run est actif**,
contrairement au hook d'installation. Désinstaller (ou réparer) pendant un
run coupe la régulation, et la pompe reste à sa dernière consigne. Le
script n'attend pas non plus la fin du process : le désinstalleur peut
tomber sur l'exe encore verrouillé.
*Correctif* : même logique que `NSIS_HOOK_PREINSTALL` (refus si
`active`, attente de la sortie du process, code d'erreur traité par un
`NSIS_HOOK_PREUNINSTALL` qui fait `Abort`).

### src-tauri/installer/fermentool-task.xml, setup-task.ps1

**A-44** · `installer/fermentool-task.xml:13-15` · Sécurité (moindre privilège) · **basse**
`RunLevel = HighestAvailable` : pour un compte administrateur, le daemon
tourne **élevé**. Un port COM ne demande aucun droit d'administration. Un
process élevé qui expose une API HTTP locale, capable de réécrire sa propre
config (`storage.dir`, `log.dir`, donc des écritures de fichiers à des
chemins arbitraires au redémarrage), aggrave A-23 et A-24.
*Correctif* : `LeastPrivilege`.

**A-45** · `installer/fermentool-task.xml:8-15, 36-39` · Robustesse (reprise) · **basse** (limitation à connaître)
Déclencheur `LogonTrigger` + `InteractiveToken` : après un redémarrage
Windows (mise à jour nocturne), le daemon ne démarre qu'à l'ouverture de
session. La pompe, elle, reste alimentée à sa dernière consigne. La reprise
après crash n'a donc lieu que lorsque quelqu'un se connecte, et
`RestartOnFailure` est limité à 3 tentatives.
*Correctif* : documenter la contrainte (session ouverte, mises à jour
automatiques planifiées hors run), ou un service Windows / un déclencheur
au démarrage avec "exécuter même si l'utilisateur n'est pas connecté".

`setup-task.ps1` : correct (substitution du chemin, `schtasks /create /f`).

### src-tauri/scripts/copy-sidecar.ps1

Aucun problème. Les échecs passent par `$LASTEXITCODE`, le contrôle
`VCRUNTIME140` est présent et `npm ci` utilise le lockfile.

### ui/ : index.html, package.json, jsconfig.json, svelte.config.js, vite.config.js, src/main.js

**A-46** · `ui/package.json:4` · Cohérence de version · **basse**
`"version": "0.1.0"`, alors que l'application en est à 0.1.5 (`Cargo.toml`,
`tauri.conf.json`). Ce numéro n'est utilisé nulle part, mais il trompe qui
le lit.
*Correctif* : l'aligner, ou le retirer de la règle "bump à chaque export".

Les autres fichiers de configuration UI ne posent aucun problème (le proxy
Vite redirige `/api` et le WebSocket vers le daemon, les icônes de
`index.html` existent dans `public/`).

### ui/src/App.svelte

**A-47** · `ui/src/App.svelte:122-133` + `ui/src/lib/api.js:46-86` · Logique (état périmé affiché comme vivant) · **haute**
`app.connected` passe à `true` au premier statut et **ne repasse jamais à
`false`** : aucun code ne le remet à faux, et `connectWs` ne signale pas la
fermeture du socket. Si le daemon s'arrête ou plante, l'UI continue
d'afficher "Daemon online" avec le **dernier statut figé** : run "running",
poids de la balance, aucune alarme. La progression et la valeur "Now" de
l'Overview continuent même d'avancer, puisqu'elles sont calculées avec
l'horloge du navigateur. Pour un système surveillé de loin pendant 100 h,
l'opérateur croit que tout tourne.
*Correctif* : un callback `onDown` dans `connectWs` (sur `onclose`) qui met
`app.connected = false`. Considérer un statut sans mise à jour depuis plus
de 5 s comme périmé : bandeau rouge "Daemon injoignable", et arrêter les
animations "live".

**A-48** · `ui/src/routes/settings/RunSettings.svelte:171-174` + `crates/fermentool-core/src/config.rs:162-163` · Logique (réglage sans effet) · **haute**
"Ask before resuming an interrupted run" (`resume.prompt`) n'est lu
**nulle part**, ni par le daemon ni par l'UI : la reprise attend toujours
un clic dans `ResumeModal`. Un opérateur qui décoche la case pour obtenir
une reprise automatique après une coupure de courant nocturne n'obtient
rien : le run reste en suspens jusqu'à ce que quelqu'un ouvre l'UI, avec la
pompe à sa dernière consigne (ou arrêtée si elle a perdu le courant). Le
réglage voisin "Resume grace" laisse aussi croire qu'il s'applique à tous
les runs, alors qu'il ne concerne plus que les bursts de calibration (voir
A-32).
*Correctif* : implémenter la reprise automatique au démarrage du daemon
quand `prompt = false` (si le lien pompe est confirmé, sinon dès qu'il
l'est), ou retirer la case. Renommer la grâce "Calibration burst resume
grace".

**A-49** · `ui/src/lib/api.js:73-77` · Ressources · **basse**
Si on se désabonne (`live = false`) pendant l'attente de reconnexion, le
`setTimeout(open, retry)` déjà programmé rouvre quand même un socket.
`open()` ne vérifie pas `live`, ce socket reste ouvert et continue d'appeler
`onStatus`. Aujourd'hui `connectWs` n'est désabonné qu'à la destruction de
l'app, donc l'impact est nul, mais le piège reste.
*Correctif* : `if (!live) return;` en tête de `open()`, et mémoriser le
timer pour l'annuler.

**A-50** · `ui/src/lib/api.js:21-39` · Robustesse · **basse**
Aucun `fetch` n'a de délai maximum (`AbortSignal.timeout`). Un daemon dont
le thread de contrôle est bloqué (A-34) laisse les boutons en "Stopping…",
"Starting…" ou "Connecting…" aussi longtemps que le navigateur garde la
requête ouverte.
*Correctif* : `signal: AbortSignal.timeout(15000)` et un message
explicite.

### ui/src/app.css

**A-51** · `ui/src/app.css:184-192` · UI · **basse**
Les pastilles d'événements utilisent `class="pill {e.level}"`
(`info`/`warn`/`error`), mais seules `.pill.running/completed/stopped/aborted`
existent. Les erreurs et avertissements du journal (Overview, History)
s'affichent donc dans le même gris que les infos.
*Correctif* : `.pill.warn` (ambre) et `.pill.error` (rouge).

### ui/src/components/Chart.svelte

**A-52** · `ui/src/components/Chart.svelte:168` · UI · **basse**
`id="ftfill"` est le même pour chaque instance du graphique. La fiche d'un
run dans History en affiche deux (la courbe et le suivi pesé) : les ID
sont dupliqués dans le document, et le dégradé d'un graphique dépend de
l'autre.
*Correctif* : un ID unique par instance, comme `PumpHead` le fait pour son
masque.

### ui/src/components/ConnectControl.svelte

Rien de propre au composant. C'est lui qui envoie `POST /api/serial/reconnect` :
un clic sur **Connect** avec le port déjà ouvert déclenche le repli sur le
simulateur décrit en **A-04**.

### ui/src/components/PumpHead.svelte

**A-53** · `ui/src/components/PumpHead.svelte:282-305` · Performance · **basse**
Pendant un run, chaque `PumpHead` (celui du menu et celui de la bande
"Now") met à jour un `$state` à **chaque frame** (60 fps) pour faire
tourner le rotor : un rendu Svelte par frame et par instance, pendant
100 h. L'Overview, lui, limite déjà son horloge animée à 15 fps.
*Correctif* : appliquer la rotation par `style.transform` directement sur
l'élément (sans état réactif), ou limiter à ~20 fps.

### ui/src/components/TrackingPanel.svelte → api.rs

**A-54** · `crates/fermentool-core/src/api.rs:434-443` (`get_tracking`) · Logique (intégrité de l'historique) · **moyenne**
Le rapport de suivi convertit les grammes en mL avec la densité **actuelle**
de `config.toml`, pas celle en vigueur pendant le run. La densité n'est pas
enregistrée avec le run. Passer la densité de 1,18 à 1,00 pour un nouveau
milieu fausse donc **rétroactivement** de 18 % les volumes livrés, le %
livré, l'écart et l'export de tous les runs passés dans History.
*Correctif* : enregistrer `density_g_per_ml` (et la position de la
balance) dans la ligne du run au démarrage (nouvelle colonne), et l'utiliser
dans `tracking_report`.

### ui/src/components/FinishModal.svelte, ResumeModal.svelte, ErrorText.svelte, FigureBand.svelte, Icon.svelte, ConnBar.svelte

Aucun problème propre. `ResumeModal` affiche une `resume_target` calculée au
moment du `GET /api/recovery`, sans le trim ni la quantification du mode
piloté : c'est une valeur indicative. Elle peut aussi être ancienne si la
fenêtre reste ouverte longtemps, puisque la reprise recalcule au moment du
clic.

### ui/src/routes/Overview.svelte

**A-55** · `ui/src/routes/Overview.svelte:362` · Sûreté d'usage · **moyenne**
"Stop run" termine le run **au premier clic**, sans confirmation. L'arrêt
est définitif (un run `stopped` ne se reprend pas) et le bouton est voisin
de "Refill bottle". La même chose vaut pour "Shut down daemon" dans
Settings > Daemon (`DaemonSettings.svelte:324`), qui coupe la régulation
d'un run en cours au premier clic (voir A-40).
*Correctif* : une confirmation dans une petite fenêtre, comme pour
"Delete run" ("Arrêter « nom » ? La pompe s'arrête, le run ne pourra pas
reprendre."), et un refus ou un avertissement explicite pour l'arrêt du
daemon pendant un run.

### ui/src/routes/NewRun.svelte

**A-56** · `ui/src/routes/History.svelte` (`settingsOf`) → `NewRun.svelte:163-183` · Logique · **basse**
"Run again" copie `kind: c.params.kind`. Pour un run `step` ou `custom`
(créé par l'API, puisque le formulaire n'offre que 4 formes), le formulaire
ne connaît pas ce `kind` et `curveSpec()` tombe sur la branche `constant`.
Relancer un tel run crée silencieusement un run à débit constant.
*Correctif* : refuser "Run again" (bouton désactivé avec explication) pour
une forme que le formulaire ne sait pas rendre.

### ui/src/routes/History.svelte

**A-57** · `ui/src/routes/History.svelte:100-127` · Performance · **basse**
Ouvrir la fiche d'un run charge **tous** ses ticks (environ 360 000 lignes
JSON pour 100 h, en pages de 50 000) avant d'afficher quoi que ce soit, et
uniquement pour l'export CSV, puisque le graphique est décimé à 3 000
points. Ouvrir la fiche d'un long run prend plusieurs secondes et des
dizaines de Mo de mémoire. La liste est aussi limitée aux 1 000 derniers
runs, sans pagination.
*Correctif* : afficher la fiche avec un échantillon (`delivery_samples`
existe déjà côté store) et ne charger le détail qu'au clic sur "Export
CSV".

**A-58** · `ui/src/routes/History.svelte` (`exportCsv`), `TubingCalibration.svelte:491-496`, `ui/src/lib/runfile.js:47-52` · Robustesse · **basse**
`URL.revokeObjectURL` est appelé dans la foulée de `a.click()`. Ça marche dans
Chromium et WebView2, mais certains navigateurs annulent le téléchargement
si l'URL est révoquée avant son démarrage.
*Correctif* : `setTimeout(() => URL.revokeObjectURL(url), 0)` ou après
quelques secondes.

### ui/src/routes/TubingCalibration.svelte

**A-59** · `ui/src/routes/TubingCalibration.svelte:240-298` · Concurrence (plusieurs fenêtres) · **basse**
Le mode automatique tourne dans la page, donc dans **chaque** onglet ou
fenêtre où elle est ouverte (fenêtre Tauri + navigateur, par exemple).
Deux instances lancent chacune leur burst (le second est refusé, `Busy`)
et sauvegardent chacune leur copie du brouillon (`persist`). La dernière
écriture gagne, ce qui peut effacer un burst ou une pesée de l'autre copie.
*Correctif* : un verrou côté daemon (le brouillon porte un identifiant de
session ; une sauvegarde d'une session plus ancienne est refusée), ou
piloter le mode automatique depuis le daemon.

### ui/src/routes/Settings.svelte, settings/sections.js, settings.css, SettingsCard.svelte, PumpSettings.svelte, BalanceSettings.svelte

Aucun problème propre (validations côté client doublées par le serveur).

### ui/src/routes/settings/DaemonSettings.svelte

Voir A-55 (arrêt du daemon sans confirmation) et A-39 (le port modifié
ici casse l'application de bureau et les scripts d'installation).

### ui/src/routes/settings/RunSettings.svelte

Voir A-48.

### ui/src/routes/settings/NotificationSettings.svelte

**A-60** · `ui/src/routes/settings/NotificationSettings.svelte:249-282` · Logique · **basse**
La sauvegarde automatique n'envoie que les personnes "complètes". Vider
le topic d'une personne pour en coller un autre la **retire** de la
config 700 ms plus tard. Si elle est responsable du run en cours, ses
alarmes cessent (le notifier journalise seulement "has no channel") jusqu'à
ce que la ligne soit de nouveau complète. Même effet si on supprime par
erreur une personne pendant un run.
*Correctif* : garder la dernière version valide d'une personne
incomplète au lieu de la retirer, et avertir (ou refuser) avant de retirer
quelqu'un qui est responsable du run actif.
