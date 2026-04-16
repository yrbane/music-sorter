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
discogs_token = "ton_token_ici"
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
└── models.rs            # Structures de données partagées (TrackInfo, etc.)
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

Appel de `fpcalc` via `std::process::Command` (pas de binding C).
