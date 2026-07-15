# Changelog

Toutes les modifications notables de ce projet sont documentées dans ce fichier.

Le format s'inspire de [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/)
et le projet suit le [versionnage sémantique](https://semver.org/lang/fr/).

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
