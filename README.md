# Fermentool

Pilote une pompe péristaltique par MODBUS-RTU (Baoding Shenchen **LabQ**,
**LabV**, Innofluid **PDS II**) pour que sa consigne (**rpm** ou **ml/min**)
suive un **profil dans le temps** (linéaire, exponentiel fed-batch, sigmoïde,
paliers, constant, personnalisé) pendant des runs d'environ 100 h, avec un
journal qui survit aux plantages, une reprise au bon point de la courbe, une
régulation par balance, le remplissage automatique du flacon, des timelapses
à la webcam et des notifications sur le téléphone.

Version **0.1.8**, en service sur le banc. Application Windows (fenêtre Tauri +
daemon), interface sur `http://127.0.0.1:8730`.

## Documentation

- **[`docs/FERMENTOOL.md`](docs/FERMENTOOL.md)** : le document de référence.
  Fonctions, pages, régulation, remplissage, calibration, caméra,
  notifications, reprise, pompes et câblage, calculateur F₀, architecture,
  API, `config.toml`, règles de l'interface, installation et export,
  développement, reste à faire.
- [`CHANGELOG.md`](CHANGELOG.md) : ce qui change pour l'utilisateur, version
  par version.
- [`docs/protocoles/`](docs/protocoles/) : les protocoles MODBUS des pompes
  (fabricants).
- [`CLAUDE.md`](CLAUDE.md) : les consignes pour Claude Code.

## Démarrage rapide

Prérequis : Rust stable (rustup), Node.js 18 ou plus, et sous Windows Visual
Studio (outils C++).

```sh
cargo test                       # les crates Rust
cargo run -p fermentool-core     # daemon + interface sur :8730 (simulateur par défaut)

cd ui && npm install
npm run dev                      # interface sur :5173, /api renvoyé vers :8730
npm run build                    # ui/dist, embarqué par le daemon
```

Produire l'installateur Windows : [`docs/FERMENTOOL.md`](docs/FERMENTOOL.md)
§14.

## Licence

MIT.
