# Changelog

Toutes les modifications notables de ce projet sont documentées dans ce fichier.

Le format s'inspire de [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/)
et le projet suit le [versionnage sémantique](https://semver.org/lang/fr/).

## 0.5.0 — 2026-07-15 · « Quarantaine des erreurs »

### Ajouté
- **Dossier `_errors/`** : les fichiers en erreur (contenu illisible, ou nom de
  destination invalide sur le système de fichiers) sont déplacés en quarantaine
  sous `target/_errors/` (structure d'origine préservée) au lieu de rester
  indéfiniment dans la source. La raison est enregistrée (via `--list-unsorted`).
- Quarantaine **auto-classifiante** : si le déplacement vers `_errors/` échoue
  aussi (cible en lecture seule, disque plein → erreur environnementale), le
  fichier reste en source pour re-tentative au prochain run. Les erreurs
  d'enrichissement (JSON/DB, transitoires) restent également en source.

### Corrigé
- Les erreurs d'I/O au rangement enregistrent désormais leur raison (auparavant
  note vide, non diagnosticable).

## 0.4.0 — 2026-07-15 · « Dédup acoustique »

### Ajouté
- **Dédup acoustique** : deux fichiers reconnus comme le même enregistrement par
  AcoustID (MBID d'enregistrement) sont dédupliqués — on conserve le meilleur
  bitrate, l'autre part à la **corbeille système** (FreeDesktop, récupérable).
  Détecte les doublons que le hash de contenu manque (même morceau ré-encodé).
  Activée par défaut (config `audio_dedup` pour désactiver) ; nécessite fpcalc +
  clé AcoustID.
- Table cache `acoustic_index` ; lookup AcoustID mémorisé (pas de double appel).

### Note de conception
- L'identité repose sur le **MBID d'enregistrement AcoustID**, pas sur l'égalité
  exacte de l'empreinte Chromaprint (qui ne survit pas au ré-encodage — vérifié).
  Les fichiers non reconnus par AcoustID ne sont **pas** dédupliqués : aucun faux
  positif, aucune suppression incertaine.

## 0.3.0 — 2026-07-15 · « Moins de non-identifiés »

### Ajouté
- **Repli AcoustID** : quand la recherche texte MusicBrainz échoue et que le
  fichier resterait non identifié, l'empreinte acoustique est désormais tentée
  (auparavant AcoustID n'était sollicité que lorsque artiste+titre manquaient).
  Sollicite la base acoustique précisément sur les cas durs (noms/tags pourris).
- **`--retry-unsorted`** : force la re-tentative des fichiers déjà marqués
  `_unsorted` dans le cache, pour profiter des améliorations d'identification
  sans re-scanner toute la source.

### Amélioré
- Parseur de noms de fichiers : reconnaît les séparateurs en-dash « – » et
  em-dash « — » en plus du tiret ASCII (fréquents dans les téléchargements).
- Nettoyage de titre avant recherche : suppression des suffixes de domaine
  parasites (« music-team.net », « www.… »).

## 0.2.0 — 2026-07-15 · « Regroupement d'albums »

### Ajouté
- **Registre d'album** : pour un même couple artiste/album, unifie la casse du
  nom d'album (première casse rencontrée) et retient l'**année de sortie la plus
  ancienne** (une réédition ne l'emporte plus sur l'original). Un fichier sans
  année hérite de celle enregistrée pour l'album. Appliqué sur **tous** les
  chemins d'enrichissement via le point de sortie unique `finalize`, convergent
  entre workers (`--workers N`).
- Nouvelle table cache `albums` et fonctions `upsert_album` / `lookup_album`.

### Note
- L'unification de la **casse** d'album prend effet dès le passage courant.
  L'**année la plus ancienne** n'est connue qu'une fois tous les fichiers vus :
  le regroupement des dossiers déjà existants se fait via une passe de
  consolidation dédiée.

## 0.1.3 — 2026-07-15 · « Casse d'artiste unifiée »

### Corrigé
- **Dossiers d'artiste dupliqués selon la casse** (ex. `Boards Of Canada` vs
  `Boards of Canada`). Deux défauts se combinaient :
  - Le registre de casse n'était appliqué que sur les matchs API : les fichiers
    déjà bien taggés ou rangés par heuristique gardaient leur casse brute. La
    canonicalisation se fait désormais sur **tous** les chemins de sortie, via un
    point unique `finalize`.
  - En parallèle (`--workers N`), `canonicalize` renvoyait sa propre casse après
    enregistrement au lieu de relire le gagnant : deux workers découvrant le même
    artiste simultanément divergeaient. Il relit maintenant le nom stocké
    (INSERT OR IGNORE → premier writer gagnant), garantissant la convergence.

## 0.1.2 — 2026-07-15 · « Intégration continue »

### Ajouté
- **Workflow CI GitHub Actions** (`.github/workflows/ci.yml`) : build release,
  suite de tests complète et `clippy` sur chaque push `main` et chaque pull request.
  Clippy est pour l'instant informatif ; le passage en `-D warnings` suivra le
  nettoyage des avertissements existants.

## 0.1.1 — 2026-07-15 · « Fiabilité réseau »

### Corrigé
- **Retries réseau réels** : tous les appels API (MusicBrainz, Discogs, AcoustID,
  Cover Art Archive) réessaient désormais sur erreur transitoire — 3 tentatives,
  backoff exponentiel 1 s → 2 s, avec respect de l'en-tête `Retry-After` (plafonné
  à 30 s) sur les réponses `429`. Les erreurs définitives (`4xx`) échouent sans
  réessai inutile.
- **Sécurité — token Discogs** : le token n'est plus jamais transmis dans l'URL de
  recherche (surface de fuite dans les logs) ; il passe exclusivement par l'en-tête
  `Authorization`, de façon cohérente avec les autres appels Discogs.

### Modifié
- Le `User-Agent` HTTP est aligné automatiquement sur la version du manifeste
  (`CARGO_PKG_VERSION`) pour éviter toute dérive.
- Suppression de l'ancien utilitaire `with_retry` (code mort, jamais câblé) au
  profit du nouveau module `retry` entièrement testé.

### Ajouté
- `README.md` complet : installation, configuration, template de nommage,
  architecture, feuille de route.
- Module `retry` : politique de retry, classification des statuts HTTP,
  calcul du backoff et pilote générique, couverts par des tests unitaires.
