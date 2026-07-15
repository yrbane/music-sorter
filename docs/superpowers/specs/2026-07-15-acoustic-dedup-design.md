# Dédup acoustique : même enregistrement → garder la meilleure qualité

Date : 2026-07-15

## Problème

La dédup actuelle ne détecte que les fichiers **byte-identiques** (hash de contenu).
Le même morceau ré-encodé (bitrate/format différent) crée des doublons non détectés.

## Solution

Deux fichiers renvoyant le **même enregistrement AcoustID** (MBID) sont le même
morceau. On conserve la **meilleure qualité** (bitrate) et on envoie l'autre à la
**corbeille système** (récupérable). Activé par défaut ; nécessite `fpcalc` + clé
AcoustID.

> **Note de conception (vérifiée à l'E2E)** : l'égalité *exacte* des empreintes
> Chromaprint ne fonctionne pas — deux ré-encodages du même morceau produisent des
> chaînes différentes (~94 %). AcoustID compare les empreintes par similarité et
> renvoie l'identité MusicBrainz du morceau : c'est cette **identité (MBID
> d'enregistrement)** qui sert de clé, pas l'empreinte brute.

## Composants

### Résolution d'identité — `Enricher::resolve_recording_id(path) -> Option<String>`
Empreinte (`generate_or_cached`, cachée par hash de contenu) → AcoustID
(`lookup_acoustid_cached`, résultat mémorisé pour éviter les doubles appels).
Renvoie le MBID d'enregistrement, ou `None` sans fpcalc/clé/match.

### Index acoustique — table cache `acoustic_index`
```sql
CREATE TABLE IF NOT EXISTS acoustic_index (
    fp_key    TEXT PRIMARY KEY,  -- MBID d'enregistrement AcoustID
    dest_path TEXT NOT NULL,
    quality   INTEGER NOT NULL   -- bitrate kbps
);
```
- `lookup_acoustic(rec_id) -> Option<(String, u32)>` (dest, qualité).
- `upsert_acoustic(rec_id, dest, quality)` : `INSERT OR REPLACE` (l'index pointe
  toujours vers le meilleur exemplaire connu).

### Flux (main.rs, avant l'organisation, après la dédup par hash)
Sous garde `opts.audio_dedup` (config `audio_dedup`, défaut `true`) et fpcalc dispo,
hors `--dry-run` :
1. `recording_id = enricher.resolve_recording_id(file)` (None → pas de dédup).
2. `cur_q = get_bitrate(file)`.
3. `lookup_acoustic(recording_id)` :
   - existant présent sur disque :
     - `cur_q > existing_q` → **corbeille l'ancien**, on range le courant, on met
       l'index à jour après organisation.
     - sinon → **corbeille le fichier source courant** (perdant), résultat
       `AcousticDuplicate { from, of }`, aucun rangement.
   - absent → rangement normal.
4. Après un rangement réussi (`Copied`/`Replaced`) : `upsert_acoustic(fp_key, dest, cur_q)`.

### Corbeille
Crate `trash` (FreeDesktop ; gère `.Trash-<uid>` sur disque externe). Jamais de
suppression définitive.

### Résultat & résumé
`ProcessResult::AcousticDuplicate { from, of }` + ligne « ⧉ N doublons acoustiques
(meilleure qualité conservée, autre en corbeille) ».

## Tests (TDD)
- Cache : upsert/lookup acoustique, remplacement (INSERT OR REPLACE).
- (Intégration) vérif E2E : deux encodages d'un morceau connu d'AcoustID → un seul
  gardé (meilleur bitrate), l'autre en corbeille.

## Hors périmètre (YAGNI)
- Similarité d'empreinte locale (décodage Chromaprint + BER) et comparaison de
  format (FLAC > MP3 à bitrate égal). On s'appuie sur l'identité AcoustID + bitrate.
- Fichiers non reconnus par AcoustID : pas de dédup acoustique (comportement sûr,
  aucun faux positif / aucune suppression incertaine).
