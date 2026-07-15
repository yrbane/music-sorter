# Registre d'album : casse et année canoniques

Date : 2026-07-15

## Problème

Un même album peut être rangé dans plusieurs dossiers distincts parce que ses
métadonnées varient selon la source (tags, heuristiques, MusicBrainz, Discogs) :

- **Année différente** : sortie originale vs remaster/réédition
  (`… - 1998 - Music…` et `… - 2013 - Music…`).
- **Casse du nom d'album différente** (`Geogaddi` vs `geogaddi`).

Aucun mécanisme n'unifie ces valeurs (contrairement à l'artiste, qui dispose de
`artist_registry`).

## Solution

Un **registre d'album** analogue à `artist_registry`, appliqué au point de sortie
unique `Enricher::finalize` (couvre donc tous les chemins d'`enrich`).

### Stockage — table cache `albums`

```sql
CREATE TABLE IF NOT EXISTS albums (
    key   TEXT PRIMARY KEY,   -- lower(artist) || '\u{1}' || lower(album)
    album TEXT NOT NULL,      -- casse canonique = première vue
    year  INTEGER             -- année canonique = la plus ancienne connue
);
```

- `upsert_album(artist, album, year)` :
  `INSERT(key, album, year) ON CONFLICT(key) DO UPDATE SET year = <plus petite
  année non nulle entre l'existante et la nouvelle>`.
  Le **nom d'album n'est jamais écrasé** (première casse gagne). L'opération est
  atomique (connexion sérialisée par mutex).
- `lookup_album(artist, album) -> Option<(String, Option<u32>)>`.

### `album_group::canonicalize(cache, artist, album, year) -> (String, Option<u32>)`

1. `upsert_album(artist, album, year)`.
2. Relit le gagnant via `lookup_album` et le renvoie (pattern « relire le gagnant »
   → convergence entre workers, comme le correctif de casse d'artiste).
3. Un fichier sans année (`year = None`) hérite de l'année enregistrée pour l'album.

### Application dans `finalize()`

Après canonicalisation de l'artiste, si `info.album` est renseigné :
`(info.album, info.year) = album_group::canonicalize(cache, artist, album, info.year)`.

## Clé de regroupement

`lower(artist) ‧ lower(album)` avec l'artiste **déjà canonicalisé** (finalize
traite l'artiste avant l'album). Deux albums identiques à la casse/année près
partagent donc la même clé.

## Tests (TDD)

- Cache : upsert/lookup, année min conservée, nom d'album première-casse conservé.
- `album_group::canonicalize` : convergence sous concurrence (N workers, casses et
  années mélangées → tous obtiennent la même casse + la plus ancienne année) ;
  héritage d'année pour un fichier sans année.
- `finalize` : casse d'album + année unifiées sur un chemin sans match API.

## Convergence : casse vs année

- **Casse du nom d'album** : première-vue, immuable après le premier enregistrement
  → converge dès le passage en cours (tous les workers relisent le gagnant).
- **Année la plus ancienne** : le minimum n'est connu qu'une fois **tous** les
  fichiers de l'album vus. Pendant un passage concurrent, un worker qui lit tôt
  obtient un minimum partiel. Le **registre final** tient la bonne valeur ; le
  regroupement effectif des dossiers par année est **garanti par la passe de
  consolidation** (ci-dessous), pas par le seul passage d'enrichissement.

## Hors périmètre (YAGNI)

- Consolidation des dossiers **déjà** rangés : proposée séparément (dry-run +
  exécution), après le correctif — les fichiers en cache `organized` ne sont pas
  reprocessés.
- Détection d'albums « proches » (fautes de frappe, ponctuation) : non traité, on
  se limite à l'égalité insensible à la casse.
```
