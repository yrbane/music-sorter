<div align="center">

```
  ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫
   __  __           _        ____             _
  |  \/  |_   _ ___(_) ___  / ___|  ___  _ __| |_ ___ _ __
  | |\/| | | | / __| |/ __| \___ \ / _ \| '__| __/ _ \ '__|
  | |  | | |_| \__ \ | (__   ___) | (_) | |  | ||  __/ |
  |_|  |_|\__,_|___/_|\___| |____/ \___/|_|   \__\___|_|
  ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪
```

# 🎵 Music Sorter

**Range et enrichit automatiquement ta bibliothèque musicale** grâce à MusicBrainz, AcoustID et Discogs.

[![Rust](https://img.shields.io/badge/Rust-1.85%2B-orange?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Edition](https://img.shields.io/badge/edition-2024-blue)](https://doc.rust-lang.org/edition-guide/)
[![Build](https://img.shields.io/badge/build-cargo-green?logo=rust)](https://doc.rust-lang.org/cargo/)
[![Version](https://img.shields.io/badge/version-0.1.0-lightgrey)](CHANGELOG.md)
[![License](https://img.shields.io/badge/license-à%20définir-red)](#-licence)

*Un binaire, zéro base de données externe, une bibliothèque impeccablement organisée.*

</div>

---

## 📖 Table des matières

- [Aperçu](#-aperçu)
- [Fonctionnalités](#-fonctionnalités)
- [Comment ça marche](#-comment-ça-marche)
- [Prérequis](#-prérequis)
- [Installation](#-installation)
- [Démarrage rapide](#-démarrage-rapide)
- [Configuration](#-configuration)
- [Utilisation détaillée](#-utilisation-détaillée)
- [Template de nommage](#-template-de-nommage)
- [Structure de sortie](#-structure-de-sortie)
- [Score de confiance & quarantaine](#-score-de-confiance--quarantaine)
- [Le cache](#-le-cache)
- [Architecture](#-architecture)
- [Développement](#-développement)
- [Feuille de route](#-feuille-de-route)
- [FAQ](#-faq)
- [Licence](#-licence)

---

## 🎯 Aperçu

**Music Sorter** parcourt un dossier source rempli de fichiers audio en vrac (téléchargements, rips, exports…), identifie chaque morceau, corrige ses métadonnées et le range dans une arborescence propre et cohérente dans un dossier destination.

L'identification s'appuie sur, dans l'ordre :

1. **Les tags embarqués** du fichier (ID3, Vorbis, MP4…) via [`lofty`](https://github.com/Serial-ATA/lofty-rs) ;
2. **L'empreinte acoustique** ([AcoustID](https://acoustid.org/) / Chromaprint) quand les tags sont insuffisants ;
3. **MusicBrainz** pour obtenir les métadonnées canoniques ;
4. **Cover Art Archive** pour la pochette ;
5. **Discogs** en repli, si un token est fourni.

Le traitement est **parallélisé** ([rayon](https://github.com/rayon-rs/rayon)), **mis en cache** (SQLite) pour être ré-exécutable sans coût, et entièrement **réversible** (`--rollback`).

---

## ✨ Fonctionnalités

| | Fonctionnalité | Détail |
|---|---|---|
| 🔎 | **Identification multi-sources** | Tags → AcoustID → MusicBrainz → Discogs, avec repli intelligent |
| 🏷️ | **Correction de tags** | Réécrit les métadonnées canoniques dans les fichiers (`--fix-tags`) |
| 🖼️ | **Pochettes** | Récupération automatique via Cover Art Archive |
| 📁 | **Nommage configurable** | Template de chemin personnalisable (`{artist}`, `{album}`, `{year}`…) |
| ⚡ | **Traitement parallèle** | `--workers N` grâce à rayon |
| 💾 | **Cache SQLite** | Ré-exécutions instantanées, cache API à TTL (30 j par défaut) |
| 🧬 | **Détection de doublons** | Par hash de contenu, inter-fichiers |
| 🥇 | **Résolution de conflits** | En cas de doublon logique, conserve le **meilleur bitrate** |
| 🚧 | **Quarantaine `_review/`** | Les matchs de faible confiance sont isolés au lieu d'être mal rangés |
| 📥 | **Zone `_unsorted/`** | Les fichiers non identifiés conservent leur arborescence relative |
| 🔄 | **Rollback complet** | Annule les opérations passées depuis le cache (`--rollback --apply`) |
| ⏯️ | **Reprise & Ctrl-C propre** | Interruption sûre + `--resume` |
| 🧪 | **Mode simulation** | `--dry-run` : rien n'est touché, rapport des destinations prévues |
| 🍎 | **Robuste** | Ignore les fichiers AppleDouble (`._foo.mp3`), insensible à la casse des extensions |

**Formats audio supportés :** `mp3`, `flac`, `ogg`, `m4a`, `aac`, `opus`, `wma`.

---

## ⚙️ Comment ça marche

```
                        ┌─────────────────────────────────────────────┐
   dossier source  ──▶  │  1. Scan récursif (extensions supportées)   │
   (~/Téléchargements)  │  2. Cache hit ? ──▶ skip instantané          │
                        │  3. Lecture des tags embarqués               │
                        │  4. Tags insuffisants ? ──▶ AcoustID/fpcalc  │
                        │  5. MusicBrainz (canonique) + Cover Art       │
                        │  6. Repli Discogs si incomplet                │
                        │  7. Score de confiance (High/Medium/Low)      │
                        │  8. Copie/déplacement selon le template       │
                        └───────────────────┬─────────────────────────┘
                                            ▼
   dossier destination  ──▶   Artiste - Année - Album/NN - Titre.ext
   (~/Music)                  _review/     (confiance faible)
                             _unsorted/    (non identifié)
```

Chaque décision est journalisée dans un cache SQLite : sources traitées, réponses API, empreintes, pochettes, index de contenu pour la déduplication.

---

## 📦 Prérequis

- **[Rust](https://rustup.rs/) ≥ 1.85** (edition 2024) et Cargo.
- Un **compilateur C** (SQLite est compilé en *bundled*, `reqwest` s'appuie sur la stack TLS système).
- **[Chromaprint](https://acoustid.org/chromaprint) (`fpcalc`)** — *optionnel mais recommandé* : active le fingerprinting AcoustID. Sans lui, l'identification se limite aux tags et à la recherche texte.

  ```bash
  # Arch Linux
  sudo pacman -S chromaprint
  # Debian / Ubuntu
  sudo apt install libchromaprint-tools
  # macOS
  brew install chromaprint
  ```

---

## 🚀 Installation

### Depuis les sources (recommandé)

```bash
git clone <url-du-repo> music-sorter
cd music-sorter
cargo install --path .
```

Le binaire `music-sorter` est placé dans `~/.cargo/bin/` (assure-toi que ce dossier est dans ton `PATH`).

### Compilation sans installation globale

```bash
cargo build --release
./target/release/music-sorter --help
```

### Vérification

```bash
music-sorter --help      # affiche la bannière, les options et des exemples
```

---

## ⏱️ Démarrage rapide

```bash
# 1. Simulation : voir ce qui serait rangé, sans rien toucher
music-sorter --source ~/Téléchargements --target ~/Music --dry-run

# 2. Tri réel (copie), 4 workers
music-sorter --source ~/Téléchargements --target ~/Music --workers 4

# 3. Déplacer + corriger les tags + reprendre après interruption
music-sorter --move --fix-tags --resume
```

> 💡 Commence **toujours** par un `--dry-run` sur une nouvelle bibliothèque.

---

## 🔧 Configuration

Les valeurs par défaut se placent dans **`~/.config/music-sorter/config.toml`**. Toute option CLI a la priorité sur la config, qui a la priorité sur les défauts.

```toml
# ~/.config/music-sorter/config.toml

# --- Chemins ---
source  = "~/Téléchargements"
target  = "~/Music"

# --- Traitement ---
workers = 4          # nb de workers parallèles (défaut : 1)
move    = false      # true = déplace au lieu de copier

# --- Clés API (optionnelles) ---
discogs_token    = "xxxxxxxx"   # active le repli Discogs
acoustid_api_key = "yyyyyyyy"   # active le fingerprinting AcoustID

# --- Cache ---
cache_enabled      = true       # défaut : true
api_cache_ttl_days = 30         # durée de vie du cache API (défaut : 30)

# --- Organisation ---
naming_template   = "{artist} - {year} - {album}/{track} - {title}"
unsorted_ttl_days = 30          # re-tente les _unsorted après ce délai
quarantine_enabled = true       # route les matchs faibles vers _review/
dedup_enabled      = true       # détecte les doublons par hash de contenu
fix_tags           = false      # réécrit les tags canoniques sur match sûr
```

### Où obtenir les clés ?

| Service | Clé | Lien |
|---|---|---|
| **AcoustID** | `acoustid_api_key` | <https://acoustid.org/api-key> |
| **Discogs** | `discogs_token` | <https://www.discogs.com/settings/developers> |

> MusicBrainz et Cover Art Archive ne nécessitent aucune clé (rate-limitée à ~1 req/s en interne).

---

## 🛠️ Utilisation détaillée

```
music-sorter [OPTIONS]
```

| Option | Description |
|---|---|
| `--source <DIR>` | Dossier source à scanner |
| `--target <DIR>` | Dossier destination |
| `--workers <N>` | Nombre de workers parallèles |
| `--move` | Déplace les fichiers au lieu de les copier (nettoie les dossiers vides) |
| `--fix-tags` | Réécrit les tags canoniques dans les fichiers sur match sûr |
| `--dry-run` | Simule sans rien copier/déplacer |
| `--resume` | Reprend après interruption (skip aussi les `_unsorted` déjà vus, ignore le TTL) |
| `--list-processed` | Liste les entrées du cache (source → destination) puis quitte |
| `--list-unsorted` | Liste les fichiers non rangés avec leur raison puis quitte |
| `--rollback` | Défait les opérations passées (dry-run par défaut) |
| `--rollback --apply` | Exécute réellement le rollback |
| `-h, --help` / `-V, --version` | Aide / version |

### Exemples

```bash
music-sorter --dry-run                                # simule, ne touche rien
music-sorter --workers 4 --move                       # déplace, 4 workers
music-sorter --move --fix-tags --resume               # range, corrige, reprend
music-sorter --target ~/Music --list-unsorted         # fichiers non rangés + raison
music-sorter --target ~/Music --list-processed        # audit du cache
music-sorter --target ~/Music --rollback              # aperçu du rollback
music-sorter --target ~/Music --rollback --apply      # exécute le rollback
```

---

## 🧩 Template de nommage

Le chemin de destination est piloté par un template. Séparateur `/` = sous-dossiers.

**Placeholders disponibles :** `{artist}` · `{album}` · `{title}` · `{year}` · `{track}` (zero-paddé sur 2) · `{genre}`.

**Défaut :**

```
{artist} - {year} - {album}/{track} - {title}
```

**Exemples de templates :**

```toml
# Par artiste puis album
naming_template = "{artist}/{year} - {album}/{track} - {title}"

# Par genre
naming_template = "{genre}/{artist}/{album}/{track} - {title}"
```

---

## 🗂️ Structure de sortie

```
~/Music/
├── Daft Punk - 2001 - Discovery/
│   ├── 01 - One More Time.mp3
│   └── 02 - Aerodynamic.mp3
├── _review/                      # 🚧 matchs de faible confiance à valider
│   └── Artiste incertain - 2019 - Album/…
├── _unsorted/                    # ⚠️ non identifiés (arborescence source préservée)
│   └── best of 2024/disc 1/unknown.mp3
└── …
```

En fin de traitement, un **récapitulatif coloré** s'affiche :

```
Traitement terminé :
  ✓ 128 fichiers organisés
  — 340 ignorés depuis le cache (instantané)
  ↑ 3 conflits résolus (meilleur bitrate conservé)
  ⚠ 12 fichiers non identifiés → _unsorted/
  ⧉ 7 doublons de contenu ignorés
  ✗ 1 erreurs
  ⏱ en 8.4s (58.1 fichiers/s)
```

---

## 🎚️ Score de confiance & quarantaine

Chaque enrichissement reçoit un niveau de **confiance** :

| Niveau | Origine | Action |
|---|---|---|
| **High** 🟢 | Confirmé par une API (AcoustID/MusicBrainz) | Rangé directement |
| **Medium** 🟡 | Tags embarqués complets, sans confirmation API | Rangé directement |
| **Low** 🔴 | Heuristiques (nom de fichier/dossier) uniquement | → `_review/` si `quarantine_enabled` |

La quarantaine `_review/` évite de polluer la bibliothèque avec des rangements douteux : tu valides à la main, puis relances.

---

## 💾 Le cache

Une base **SQLite** (dans le répertoire de données de l'app) mémorise :

- `processed_files` — sources déjà traitées (status, destination, note) → **skip instantané** ;
- `api_cache` — réponses MusicBrainz/Discogs avec TTL ;
- `fingerprint_cache` — empreintes AcoustID calculées ;
- `cover_cache` — pochettes téléchargées ;
- `content_index` — hash de contenu pour la déduplication ;
- `artists` — registre des noms canoniques.

➡️ Une deuxième exécution sur la même source est quasi instantanée. Désactivable via `cache_enabled = false`.

---

## 🏛️ Architecture

Projet Rust modulaire (~4 900 lignes) :

| Module | Rôle |
|---|---|
| `main.rs` | Orchestration, parallélisme, récapitulatif, signaux |
| `cli.rs` | Parsing des arguments (clap) + résolution CLI/config/défauts |
| `config.rs` | Chargement `config.toml` |
| `scanner.rs` | Scan récursif, filtrage des extensions & AppleDouble |
| `enricher.rs` | Pipeline d'enrichissement (tags → AcoustID → MB → Discogs) |
| `musicbrainz.rs` / `discogs.rs` / `coverart.rs` | Clients API |
| `fingerprint.rs` | Empreintes via `fpcalc` + lookup AcoustID |
| `tags.rs` | Lecture/écriture des métadonnées (lofty) |
| `organizer.rs` | Rendu du template, calcul du chemin, `_review`/`_unsorted` |
| `cache.rs` | Couche SQLite (rusqlite) |
| `rollback.rs` | Listing & annulation |
| `title_cleaner.rs` | Nettoyage des titres pour améliorer le taux de match |
| `rate_limiter.rs` / `retry.rs` | Respect des quotas API + retries |
| `models.rs` | `TrackInfo`, `Confidence`, `ProcessResult` |

**Principes :** SOLID, DRY, KISS. Erreurs propagées via `anyhow`, rate-limiting par service, retries avec back-off.

---

## 🧑‍💻 Développement

```bash
cargo build                 # build debug
cargo build --release       # build optimisé
cargo test                  # suite de tests (unitaires + intégration)
cargo clippy --all-targets  # lint
cargo fmt                   # formatage
```

Tests unitaires embarqués dans chaque module + `tests/integration_test.rs`.

---

## 🗺️ Feuille de route

**Robustesse (prioritaire)**

- [ ] **Retries réseau réels** : câbler le back-off exponentiel sur tous les appels API + respect de `Retry-After` (429).
- [ ] **Rapport `--report out.json`** : export JSON/CSV du plan de déplacements avant application.

**Fonctionnalités**

- [ ] **Pochette embarquée** dans les tags (pas seulement déposée sur disque).
- [ ] **Détection de doublons acoustiques** (même morceau ré-encodé) via l'empreinte AcoustID, au-delà du hash exact.
- [ ] **`--watch`** : surveille la source et range à la volée (tri incrémental).
- [ ] **Sous-commandes** (`music-sorter sort | rollback | list | doctor`) plutôt que des flags booléens.
- [ ] **Interface interactive** pour valider la file `_review/` au clavier.
- [ ] **Playlists & smart folders** (liens symboliques par genre/année) en plus du tri physique.
- [ ] **Normalisation ReplayGain / analyse BPM-clé** (usage DJ).
- [ ] **Sources de repli supplémentaires** (Beatport, Bandcamp) pour l'électro mal couverte par MusicBrainz.

---

## ❓ FAQ

**« fpcalc non trouvé » à l'exécution ?**
Installe Chromaprint (voir [Prérequis](#-prérequis)). Sans lui, le fingerprinting est désactivé mais le tri fonctionne quand même via les tags.

**Mes fichiers sont-ils en sécurité ?**
Par défaut, Music Sorter **copie** (ne déplace pas). Utilise `--dry-run` d'abord, et `--rollback` pour tout annuler.

**Puis-je relancer sans tout re-traiter ?**
Oui — le cache skippe instantanément ce qui a déjà été traité.

---

## 📄 Licence

À définir. *(Aucune licence n'est actuellement déclarée dans le dépôt — pensez à ajouter un fichier `LICENSE`.)*

---

<div align="center">

Fait avec ❤️ et 🦀 — *pour une bibliothèque musicale enfin en ordre.*

</div>
