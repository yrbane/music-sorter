# music-sorter — Spec de design

## Vue d'ensemble

Exécutable Rust CLI qui scanne un dossier source à la recherche de fichiers musicaux, identifie et enrichit leurs métadonnées via des bases de données en ligne (MusicBrainz, Discogs, AcoustID), puis copie les fichiers dans une arborescence organisée.

## Interface CLI

```
music-sorter [OPTIONS]

Options :
  --source <PATH>    Dossier source à scanner (défaut : ~/Téléchargements/)
  --target <PATH>    Dossier destination (défaut : ~/Music/)
  --workers <N>      Nombre de workers parallèles (défaut : 1, séquentiel)
  --move             Déplacer les fichiers au lieu de les copier
  -h, --help         Aide
  -V, --version      Version
```

## Configuration

Fichier : `~/.config/music-sorter/config.toml`

```toml
# Tokens API
discogs_token = "ton_token_ici"
acoustid_api_key = "ta_cle_acoustid"

# Paramètres par défaut (chacun overridable via CLI)
source = "~/Téléchargements"
target = "~/Music"
workers = 1
move = false

# Cache
cache_enabled = true
api_cache_ttl_days = 30
```

Le token Discogs est obtenu sur https://www.discogs.com/settings/developers.

## Formats audio supportés

| Format | Extensions | Tag format |
|--------|-----------|------------|
| MP3 | .mp3 | ID3v2 |
| FLAC | .flac | Vorbis Comments |
| OGG Vorbis | .ogg | Vorbis Comments |
| M4A/AAC | .m4a, .aac | MP4/iTunes atoms |
| Opus | .opus | Vorbis Comments |
| WMA | .wma | ASF |

WAV est explicitement exclu (pas de support métadonnées exploitable).

## Architecture

### Pipeline de traitement par fichier

```
Scan récursif
     │
     ▼
Lecture tags existants (lofty)
     │
     ▼
Tags suffisants ? ──non──▶ Fingerprint AcoustID (fpcalc)
     │                              │
     oui                            ▼
     │                     AcoustID → MusicBrainz Recording ID
     │                              │
     ▼                              ▼
Recherche MusicBrainz (texte ou Recording ID)
     │
     ▼
Résultat trouvé ? ──non──▶ Recherche Discogs (fallback)
     │                              │
     oui                            ▼
     │                     Résultat trouvé ? ──non──▶ _unsorted/
     │                              │
     ▼                              oui
Récupération pochette                │
(Cover Art Archive → Discogs)        ▼
     │                     Récupération pochette (Discogs)
     ▼                              │
Enrichissement des tags ◀───────────┘
     │
     ▼
Copie vers destination
     │
     ▼
(--move) Suppression source
```

### Modules

```
src/
├── main.rs              # Point d'entrée, parsing CLI (clap)
├── config.rs            # Lecture config TOML
├── scanner.rs           # Scan récursif du dossier source (walkdir)
├── tags.rs              # Lecture/écriture des tags audio (lofty)
├── fingerprint.rs       # Appel fpcalc + soumission AcoustID
├── musicbrainz.rs       # Client API MusicBrainz
├── discogs.rs           # Client API Discogs
├── coverart.rs          # Récupération pochettes (Cover Art Archive + Discogs)
├── enricher.rs          # Orchestration de l'enrichissement (pipeline)
├── organizer.rs         # Copie/déplacement + gestion conflits
├── retry.rs             # Retry avec backoff exponentiel
├── rate_limiter.rs      # Limiteur de débit partagé entre workers
├── models.rs            # Structures de données partagées (TrackInfo, etc.)
├── cache.rs             # Cache SQLite persistant (re-runs rapides)
├── cache_keys.rs        # Hashes et clés normalisées pour le cache
├── artist_registry.rs   # Registre canonique des noms d'artistes
└── album_group.rs       # Helper de regroupement par dossier source
```

### Structures de données principales

```rust
struct TrackInfo {
    artist: Option<String>,
    album: Option<String>,
    title: Option<String>,
    year: Option<u32>,
    track_number: Option<u32>,
    total_tracks: Option<u32>,
    genre: Option<String>,
    cover_art: Option<Vec<u8>>,  // Image brute (JPEG/PNG)
}
```

## Stratégie d'identification

Par ordre de priorité :

1. **Tags existants** — lecture via lofty des tags ID3v2, Vorbis Comments, MP4 atoms
2. **Fingerprint AcoustID** — si tags insuffisants (artiste OU titre manquant), appel `fpcalc` en sous-processus pour générer l'empreinte Chromaprint, puis soumission à l'API AcoustID pour obtenir un MusicBrainz Recording ID
3. **MusicBrainz** — recherche par Recording ID (fingerprint) ou par texte (artiste + titre). Récupère : artiste, album, titre, année, numéro de piste, total pistes, genre
4. **Cover Art Archive** — pochette liée au Release ID MusicBrainz
5. **Discogs (fallback)** — si MusicBrainz n'a pas trouvé ou si la pochette manque. Recherche textuelle artiste + album

## Enrichissement des tags

L'enrichissement est **systématique** : même si les tags existants suffisent au classement, on cherche toujours à compléter les métadonnées manquantes dans le fichier destination.

Règles :
- On ne remplace jamais un tag existant non vide par une valeur vide
- On complète les champs manquants avec les données MusicBrainz/Discogs
- La pochette est embarquée dans le fichier destination (APIC pour ID3, METADATA_BLOCK_PICTURE pour Vorbis/FLAC, covr pour MP4)
- Les tags sont écrits sur le fichier **destination** (jamais sur la source)

## Organisation des fichiers

### Arborescence destination

```
~/Music/
├── Boards of Canada - 2002 - Geogaddi/
│   ├── 01 - Ready Lets Go.flac
│   ├── 02 - Music Is Math.flac
│   └── ...
├── Portishead - 1994 - Dummy/
│   ├── 01 - Mysterons.flac
│   └── ...
└── _unsorted/
    └── fichier_non_identifie.mp3
```

Format : `[artist] - [year] - [album]/[trackNb] - [trackName].[ext]`

- `trackNb` : zéro-paddé sur 2 chiffres (01, 02...)
- Caractères invalides pour le filesystem sont remplacés par `_`
- Si `year` est inconnu : omis → `[artist] - [album]/`
- Si `artist` est inconnu mais `album` connu : `Unknown Artist - [album]/`
- Si rien n'est identifiable : `_unsorted/`

### Gestion des conflits

Quand un fichier destination existe déjà :
1. Comparer les empreintes Chromaprint des deux fichiers (source et destination existante)
2. Si même empreinte (même morceau) → comparer les bitrates, garder le fichier avec le meilleur bitrate
3. Si empreinte différente → les fichiers sont différents, suffixer le nom (`trackName (2).[ext]`)

## Parallélisme

- **Par défaut** (`--workers 1`) : traitement séquentiel fichier par fichier
- **`--workers N`** : pool de N workers via rayon, chaque worker traite un fichier indépendamment
- Un **rate limiter partagé** entre les workers pour respecter :
  - MusicBrainz : 1 requête/seconde
  - Discogs : 60 requêtes/minute
  - AcoustID : 3 requêtes/seconde

## Gestion des erreurs

### Erreurs réseau
- Retry avec backoff exponentiel (3 tentatives : 1s, 2s, 4s) sur les appels API
- Si toutes les APIs échouent → traitement avec les tags existants uniquement
- Si tags insuffisants → `_unsorted/`

### Erreur fichier
- Fichier corrompu / illisible → log d'erreur, fichier laissé en place dans la source
- Erreur d'écriture des tags → log d'erreur, fichier laissé en place

### Dépendance fpcalc
- Au lancement, vérification de la présence de `fpcalc` dans le PATH
- Si absent → avertissement affiché, fingerprinting désactivé pour toute la session
- L'outil fonctionne quand même (tags existants + recherche textuelle uniquement)
- Installation sur Arch Linux : `sudo pacman -S chromaprint`

## Logs et sortie console

Sortie colorée (via colored) :
- `✓` vert : fichier traité et déplacé avec succès
- `⚠` jaune : fichier envoyé dans `_unsorted/`
- `✗` rouge : erreur (fichier laissé en place)
- `↑` cyan : conflit résolu (meilleur bitrate conservé)

Récapitulatif final :
```
Traitement terminé :
  ✓ 42 fichiers organisés
  ↑  3 conflits résolus (meilleur bitrate conservé)
  ⚠  5 fichiers non identifiés → _unsorted/
  ✗  1 erreur
```

## Cache et performances

### Emplacement

Une base SQLite est créée à la racine du dossier destination : `<target>/.music-sorter.db`. Elle agit comme cache persistant pour accélérer les ré-exécutions et réduire la charge sur les APIs externes.

Ouverte en mode WAL pour permettre lectures concurrentes par plusieurs workers, avec `synchronous = NORMAL` et `temp_store = MEMORY` pour les performances.

### Tables

| Table | Clé | Contenu | Usage |
|-------|-----|---------|-------|
| `processed_files` | `source_path` | mtime, size, dest_path, status, last_seen | Index des fichiers déjà traités. Skip total au prochain run si mtime/size inchangés. |
| `fingerprint_cache` | SHA-256 du fichier | chromaprint, duration | Évite de relancer fpcalc (coût en secondes par fichier). |
| `api_cache` | (endpoint, cache_key) | response JSON, fetched_at | Cache des appels MusicBrainz, AcoustID, Discogs. TTL configurable. |
| `cover_cache` | release_id | image binaire, fetched_at | Cache des pochettes téléchargées. |
| `artists` | canonical_lower | canonical, mbid | Registre canonique des noms d'artistes (résout les variations de casse). |

### TTL par défaut

- MusicBrainz : 30 jours
- Discogs : 90 jours (les releases changent rarement)
- Cover art binaire : pas d'expiration (taille bornée par le nombre de releases uniques)
- Fingerprints : pas d'expiration (déterministe par fichier)

### Configuration

```toml
# ~/.config/music-sorter/config.toml
cache_enabled = true        # défaut true. Si false, le cache est en mémoire (perdu à chaque run).
api_cache_ttl_days = 30     # exposé mais non actif pour l'instant (TTL hardcodés par client)
```

### Réinitialiser le cache

```bash
rm -f ~/Music/.music-sorter.db ~/Music/.music-sorter.db-wal ~/Music/.music-sorter.db-shm
```

### Optimisations supplémentaires

- **Copie via reflink** (CoW) : sur btrfs/xfs/zfs, la copie est quasi-instantanée et ne consomme pas d'espace disque tant que le fichier n'est pas modifié. Fallback automatique sur `std::fs::copy` pour les autres filesystems.
- **HTTP** : reqwest configuré avec gzip + HTTP/2 (négocié via TLS ALPN) + connection pool (4 connexions persistantes par hôte).
- **Court-circuit tags complets** : si un fichier source a déjà tous les champs essentiels + une pochette, aucun appel API n'est effectué.

### Procédure de bench (manuelle)

Pour mesurer le gain du cache sur ton dossier réel :

```bash
# Premier run (cache froid) — on supprime la DB pour repartir de zéro
rm -f ~/Music/.music-sorter.db*
time ./target/release/music-sorter --source ~/Téléchargements --target ~/Music

# Deuxième run (cache chaud)
time ./target/release/music-sorter --source ~/Téléchargements --target ~/Music
```

Sur un dossier de 200 fichiers déjà organisés, le second run doit passer de plusieurs minutes (rate-limit MB+Discogs) à moins d'une seconde (juste un `SELECT` par fichier).

## Crates Rust

| Crate | Usage |
|-------|-------|
| clap | Parsing CLI |
| toml + serde | Lecture config TOML |
| lofty | Lecture/écriture tags audio (tous formats) |
| reqwest (blocking) | Appels HTTP (MusicBrainz, Discogs, AcoustID) |
| serde + serde_json | Sérialisation/désérialisation JSON |
| walkdir | Scan récursif |
| colored | Sortie console colorée |
| rayon | Parallélisme workers |
| rusqlite | Cache SQLite (bundled) |
| sha2, hex | Content-hash des fichiers pour le cache fingerprint |
| reflink-copy | Copie CoW sur btrfs/xfs/zfs |

Appel de `fpcalc` via `std::process::Command` (pas de binding C).
