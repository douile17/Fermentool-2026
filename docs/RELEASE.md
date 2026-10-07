# Exporter Fermentool (installateur Windows)

Ce document décrit comment produire l'installateur à copier sur un autre PC, et
comment l'installer. Règle de base : **l'installateur doit suffire pour une
première installation sur un Windows 10 ou 11 neuf**. Rien d'autre à installer
à côté.

## Ce que contient l'installateur

`Fermentool_<version>_x64-setup.exe` (environ 5 Mo) installe, pour
l'utilisateur courant, dans `%LOCALAPPDATA%\Fermentool` :

- `fermentool.exe` : la fenêtre (Tauri) avec l'interface intégrée ;
- `fermentool-core.exe` : le daemon qui pilote la pompe et lit la balance ;
- une tâche planifiée `Fermentool` qui lance le daemon à l'ouverture de
  session et le relance s'il plante (sans droits administrateur : un port
  COM n'en demande pas).

Aucune dépendance externe :

- **Runtime Visual C++** : les exe sont liés en statique (`.cargo/config.toml`,
  `+crt-static`), `VCRUNTIME140.dll` n'est pas nécessaire.
- **WebView2** (moteur d'affichage de la fenêtre) : déjà présent sur Windows 10
  et 11. S'il manque, l'installateur le télécharge (`"webviewInstallMode":
  { "type": "downloadBootstrapper" }` dans `src-tauri/tauri.conf.json`). Pour
  un PC sans internet et sans WebView2 seulement, passer à `offlineInstaller`
  (installateur d'environ 220 Mo).

Mise à jour d'une installation existante : l'installateur arrête proprement
le daemon avant de copier les fichiers, et **refuse de s'installer si un run
est en cours** (message « A run is in progress »). Arrêter le run d'abord. Il
refuse aussi si le daemon ne s'arrête pas ou ne répond pas, plutôt que de
laisser l'ancienne version en place. La désinstallation applique les mêmes
règles.

## Limites à connaître

- **Le daemon démarre à l'ouverture de session**, pas au démarrage du PC. Après
  un redémarrage (mise à jour Windows la nuit, coupure de courant), la reprise
  du run n'a lieu qu'une fois quelqu'un connecté sur ce PC. Pendant ce temps la
  pompe, si elle est restée alimentée, garde sa dernière consigne. Sur le PC
  de manip : régler les heures d'activité de Windows Update pour qu'il ne
  redémarre pas pendant un run, et, pour une reprise sans clic, décocher
  « Ask before resuming » (Settings, Crash resume).
- **Le port 8730 est fixe** pour la fenêtre Fermentool et l'installateur. Le
  changer dans Settings, Daemon ne sert qu'à un usage dans le navigateur.
- Un adaptateur USB-série débranché en pleine transaction peut bloquer le
  pilote Windows indéfiniment. Le daemon abandonne alors ce fil d'exécution et
  rouvre le port sur un neuf, ce qui coûte un fil par débranchement de ce
  type, jusqu'au redémarrage du daemon.

## Produire l'installateur

Sur le PC de développement (Rust, Node, `cargo tauri` installés).

1. **Partir de `main` à jour**, arbre propre (`git status`).
2. **Monter la version**, toujours, pour distinguer le nouveau build de
   l'ancien (sinon rien ne permet de savoir quelle version est installée) :
   - `Cargo.toml` : `[workspace.package] version = "x.y.z"`
   - `src-tauri/tauri.conf.json` : `"version": "x.y.z"`
3. **Tests** :
   ```sh
   cargo test --workspace --exclude fermentool-tauri
   ```
4. **Interface + daemon** (PowerShell, depuis la racine du dépôt) :
   ```powershell
   .\src-tauri\scripts\copy-sidecar.ps1
   ```
   Le script construit `ui/dist`, compile le daemon en release et le place dans
   `src-tauri/binaries/`. Il s'arrête si le daemon dépend encore de
   `VCRUNTIME140.dll`.
5. **Installateur** :
   ```sh
   cargo tauri build
   ```
   Résultat : `target\release\bundle\nsis\Fermentool_<version>_x64-setup.exe`.
6. **Contrôle avant de livrer** (obligatoire) : lancer
   `target\release\fermentool.exe`, vérifier que
   `http://127.0.0.1:8730/api/status` répond avec la bonne `app_version`, tuer
   `fermentool.exe` dans le Gestionnaire des tâches, vérifier que
   `fermentool-core.exe` et `/api/status` sont toujours là, puis arrêter le
   daemon (`POST /api/shutdown`). Pour ne pas toucher à la configuration réelle,
   faire ce contrôle avec `APPDATA` pointé vers un dossier de test.
7. **Commiter** le changement de version et **taguer** (`git tag vx.y.z`).

## Installer sur un autre PC

1. Copier `Fermentool_<version>_x64-setup.exe` (clé USB) et le lancer. Si
   SmartScreen s'affiche (installateur non signé) : « Informations
   complémentaires », puis « Exécuter quand même ».
2. **Pompe** : brancher l'adaptateur USB-RS485, choisir son port dans la barre
   de connexion en haut de l'interface.
3. **Balance** : Settings, carte « Balance » : choisir le port de la balance
   (pas celui de la pompe), baud 9600, densité du liquide, « Save balance ».
   Appliqué tout de suite, sans redémarrer. (Enregistré dans la section
   `[scale]` de `%APPDATA%\Fermentool\config.toml`.)
4. Vérifier en bas de la barre latérale : la pastille de la pompe, et
   « Balance · 0.0 g · stable ». La version s'affiche dans `/api/status`.

## Dépannage

| Symptôme | Cause probable | Correctif |
|---|---|---|
| Pas de pastille pompe, « reconnecting… » en bas à gauche | Le daemon ne tourne pas | Gestionnaire des tâches : `fermentool-core.exe` absent ? Double-cliquer `%LOCALAPPDATA%\Fermentool\fermentool-core.exe` pour voir l'erreur. Journaux : `%APPDATA%\Fermentool\logs`. |
| L'ancienne version est toujours là après une mise à jour | Le daemon tournait pendant l'installation | Quitter via « Shut down daemon & quit », relancer l'installateur. |
| Pas de case « Enable gravimetric trim », pas d'indicateur Balance | Aucune balance configurée | Settings, carte « Balance », choisir le port, « Save balance ». |
| « Balance disconnected » | Mauvais port COM, câble, vitesse | Vérifier le port dans le Gestionnaire de périphériques et le baud (9600 par défaut sur la Ranger 7000), dans Settings, carte « Balance ». |
