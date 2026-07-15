use crate::models::TrackInfo;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Résultat d'une copie de fichier
#[derive(Debug)]
pub enum CopyResult {
    Copied,
    Replaced { bitrate: u32 },
    Skipped { existing_bitrate: u32 },
}

/// Remplace les caractères invalides dans un nom de fichier par `_`
/// Longueur max d'une composante de chemin, en octets. La plupart des systèmes de
/// fichiers plafonnent à 255 ; on garde une marge pour l'extension et l'UTF-8.
const MAX_COMPONENT_BYTES: usize = 200;

/// Tronque une composante à `max_bytes` sans couper un caractère UTF-8.
fn truncate_component(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].trim_end().to_string()
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Cherche un dossier existant dont le nom correspond (case-insensitive)
/// et retourne son nom exact. Sinon retourne le nom proposé.
fn resolve_existing_folder(target: &Path, proposed: &str) -> String {
    let proposed_lower = proposed.to_lowercase();
    if let Ok(entries) = std::fs::read_dir(target) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.to_lowercase() == proposed_lower {
                    return name.to_string();
                }
            }
        }
    }
    proposed.to_string()
}

/// Template de nommage par défaut. Placeholders : {artist} {album} {title}
/// {year} {track} {genre}. Les segments sont séparés par `/` (dossiers) ;
/// un segment dont un token est vide voit ses ` - ` superflus collapsés.
pub const DEFAULT_TEMPLATE: &str = "{artist} - {year} - {album}/{track} - {title}";

/// Substitue un token par sa valeur dans `info`. {track} est zero-paddé sur 2.
fn token_value(token: &str, info: &TrackInfo) -> String {
    match token {
        "artist" => info.artist.clone().unwrap_or_default(),
        "album" => info.album.clone().unwrap_or_default(),
        "title" => info.title.clone().unwrap_or_default(),
        "genre" => info.genre.clone().unwrap_or_default(),
        "year" => info.year.map(|y| y.to_string()).unwrap_or_default(),
        "track" => info.track_number.map(|n| format!("{:02}", n)).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Remplace tous les `{token}` d'un segment par leurs valeurs.
fn substitute_segment(segment: &str, info: &TrackInfo) -> String {
    let mut out = String::new();
    let mut rest = segment;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find('}') {
            let token = &rest[start + 1..start + end];
            out.push_str(&token_value(token, info));
            rest = &rest[start + end + 1..];
        } else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

/// Collapse les séparateurs ` - ` superflus issus d'un token vide
/// (ex. « Artist -  - Album » → « Artist - Album »).
fn collapse_separators(s: &str) -> String {
    s.split(" - ")
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" - ")
}

/// Rend le chemin relatif (sous la racine cible) à partir d'un template.
/// Chaque segment séparé par `/` devient un composant de chemin sanitizé.
/// L'extension est ajoutée au dernier segment (le nom de fichier).
pub fn render_relative_path(template: &str, info: &TrackInfo, ext: &str) -> PathBuf {
    let segments: Vec<&str> = template.split('/').collect();
    let last = segments.len().saturating_sub(1);
    let mut path = PathBuf::new();
    for (i, segment) in segments.iter().enumerate() {
        let substituted = substitute_segment(segment, info);
        let collapsed = collapse_separators(&substituted);
        // Tronque à la limite du système de fichiers (évite « File name too long »).
        let mut component =
            truncate_component(&sanitize_filename(&collapsed), MAX_COMPONENT_BYTES);
        if i == last && !ext.is_empty() {
            // Un nom de fichier vide donnerait un chemin = dossier → « Is a directory ».
            if component.is_empty() {
                component = "track".to_string();
            }
            component = format!("{}.{}", component, ext);
        } else if component.is_empty() {
            component = "unknown".to_string();
        }
        path.push(component);
    }
    path
}

/// Construit le chemin de destination d'un fichier audio (template par défaut).
/// `source_root` permet de reconstruire la structure relative dans `_unsorted/`
#[allow(dead_code)]
pub fn build_destination_path(
    target: &Path,
    info: &TrackInfo,
    original_path: &Path,
    source_root: &Path,
) -> PathBuf {
    build_destination_path_with_template(target, info, original_path, source_root, DEFAULT_TEMPLATE)
}

/// Variante paramétrée par un template de nommage (cf. config.toml).
pub fn build_destination_path_with_template(
    target: &Path,
    info: &TrackInfo,
    original_path: &Path,
    source_root: &Path,
    template: &str,
) -> PathBuf {
    // Récupère l'extension du fichier original
    let ext = original_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    // Pas assez d'info → dossier _unsorted en conservant la structure relative
    if !info.has_minimum_for_organization() {
        return target
            .join("_unsorted")
            .join(relative_to_source(original_path, source_root));
    }

    // Rend le chemin relatif via le template, puis réutilise un dossier
    // existant si seule la casse du premier composant (dossier) diffère.
    let relative = render_relative_path(template, info, ext);
    let mut components = relative.components();
    if let Some(first) = components.next() {
        let first_str = first.as_os_str().to_string_lossy();
        let resolved_first = resolve_existing_folder(target, &first_str);
        let rest: PathBuf = components.collect();
        return target.join(resolved_first).join(rest);
    }

    target.join(relative)
}

/// Chemin relatif d'un fichier par rapport à la racine source ; à défaut (fichier
/// hors de cette racine), son simple nom. Sert à reconstruire la structure sous
/// `_unsorted/` et `_errors/`.
fn relative_to_source<'a>(original_path: &'a Path, source_root: &Path) -> &'a Path {
    original_path.strip_prefix(source_root).unwrap_or_else(|_| {
        Path::new(
            original_path
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("unknown")),
        )
    })
}

/// Destination de quarantaine pour un fichier en **erreur de contenu** (illisible,
/// tags corrompus) : sous `target/_errors/`, structure relative préservée.
pub fn error_destination(target: &Path, original_path: &Path, source_root: &Path) -> PathBuf {
    target
        .join("_errors")
        .join(relative_to_source(original_path, source_root))
}

/// Rebase une destination organisée sous `target/_review/` (zone de quarantaine
/// pour les matchs de faible confiance). Si `dest` n'est pas sous `target`, no-op.
pub fn redirect_to_review(target: &Path, dest: &Path) -> PathBuf {
    match dest.strip_prefix(target) {
        Ok(rel) => target.join("_review").join(rel),
        Err(_) => dest.to_path_buf(),
    }
}

/// Copie via reflink (CoW, instantané sur btrfs/xfs/zfs).
/// En cas d'échec (filesystem non-supporté, cross-fs, etc.), fallback std::fs::copy.
fn copy_with_reflink(source: &Path, destination: &Path) -> Result<()> {
    match reflink_copy::reflink(source, destination) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(source, destination)?;
            Ok(())
        }
    }
}

/// Vrai si les deux chemins désignent le même fichier réel (après résolution).
/// Protège les retris « sur place » (source déjà à sa destination).
fn is_same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(pa), Ok(pb)) => pa == pb,
        _ => false,
    }
}

/// Copie un fichier vers sa destination en gérant les conflits par bitrate
pub fn copy_to_destination(source: &Path, destination: &Path) -> Result<CopyResult> {
    // Source déjà à sa destination (retri sur place) : ne rien faire.
    if is_same_file(source, destination) {
        return Ok(CopyResult::Skipped {
            existing_bitrate: crate::tags::get_bitrate(destination).unwrap_or(0),
        });
    }
    // Crée les dossiers parents si nécessaire
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // La destination existe déjà → comparaison de bitrate
    if destination.exists() {
        let src_bitrate = crate::tags::get_bitrate(source).unwrap_or(0);
        let dst_bitrate = crate::tags::get_bitrate(destination).unwrap_or(0);

        if src_bitrate > dst_bitrate {
            // La source est de meilleure qualité → on remplace
            copy_with_reflink(source, destination)?;
            return Ok(CopyResult::Replaced { bitrate: src_bitrate });
        } else {
            // La destination est au moins aussi bonne → on garde
            return Ok(CopyResult::Skipped {
                existing_bitrate: dst_bitrate,
            });
        }
    }

    // Pas de conflit → copie simple
    copy_with_reflink(source, destination)?;
    Ok(CopyResult::Copied)
}

/// Supprime le fichier source
#[allow(dead_code)]
pub fn remove_source(source: &Path) -> Result<()> {
    std::fs::remove_file(source)?;
    Ok(())
}

/// Déplace un fichier vers sa destination en consommant la source.
/// Privilégie `rename(2)` (atomique, zéro espace, instantané sur même FS),
/// sinon fallback copy + delete pour les déplacements cross-filesystem.
fn move_or_copy_delete(source: &Path, destination: &Path) -> Result<()> {
    if std::fs::rename(source, destination).is_ok() {
        return Ok(());
    }
    copy_with_reflink(source, destination)?;
    std::fs::remove_file(source)?;
    Ok(())
}

/// Déplace un fichier vers sa destination en gérant les conflits par bitrate.
/// La source est TOUJOURS consommée en cas de succès (sémantique --move).
pub fn move_to_destination(source: &Path, destination: &Path) -> Result<CopyResult> {
    // Source déjà à sa destination (retri sur place) : ne rien faire, surtout ne
    // JAMAIS supprimer le fichier.
    if is_same_file(source, destination) {
        return Ok(CopyResult::Skipped {
            existing_bitrate: crate::tags::get_bitrate(destination).unwrap_or(0),
        });
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if destination.exists() {
        let src_bitrate = crate::tags::get_bitrate(source).unwrap_or(0);
        let dst_bitrate = crate::tags::get_bitrate(destination).unwrap_or(0);

        if src_bitrate > dst_bitrate {
            // Source meilleure → on remplace dest et on consomme source
            std::fs::remove_file(destination)?;
            move_or_copy_delete(source, destination)?;
            return Ok(CopyResult::Replaced { bitrate: src_bitrate });
        } else {
            // Dest au moins aussi bonne → on supprime juste source (consolidation)
            std::fs::remove_file(source)?;
            return Ok(CopyResult::Skipped {
                existing_bitrate: dst_bitrate,
            });
        }
    }

    move_or_copy_delete(source, destination)?;
    Ok(CopyResult::Copied)
}

/// Supprime récursivement les sous-dossiers vides sous `root` (root inclus exclu).
/// Utilise `remove_dir` (non-récursif) qui n'efface que les dossiers réellement vides.
pub fn cleanup_empty_dirs(root: &Path) -> usize {
    use walkdir::WalkDir;
    let mut removed = 0;
    for entry in WalkDir::new(root)
        .contents_first(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_dir()
            && entry.path() != root
            && std::fs::remove_dir(entry.path()).is_ok()
        {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TrackInfo;
    use std::io::Write;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn test_build_destination_full_info() {
        let info = TrackInfo {
            artist: Some("Boards of Canada".into()),
            album: Some("Geogaddi".into()),
            title: Some("Music Is Math".into()),
            year: Some(2002),
            track_number: Some(2),
            ..Default::default()
        };
        let original = Path::new("song.flac");
        let target = Path::new("/home/user/Music");
        let source = Path::new("/home/user/Downloads");

        let result = build_destination_path(target, &info, original, source);

        assert_eq!(
            result,
            Path::new("/home/user/Music/Boards of Canada - 2002 - Geogaddi/02 - Music Is Math.flac")
        );
    }

    #[test]
    fn test_build_destination_no_year() {
        let info = TrackInfo {
            artist: Some("BoC".into()),
            album: Some("Geogaddi".into()),
            title: Some("Track".into()),
            year: None,
            track_number: None,
            ..Default::default()
        };
        let original = Path::new("file.mp3");
        let target = Path::new("/music");
        let source = Path::new("/downloads");

        let result = build_destination_path(target, &info, original, source);

        assert_eq!(result, Path::new("/music/BoC - Geogaddi/Track.mp3"));
    }

    #[test]
    fn test_build_destination_no_track_number() {
        let info = TrackInfo {
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            title: Some("Title".into()),
            year: Some(2020),
            track_number: None,
            ..Default::default()
        };
        let original = Path::new("track.ogg");
        let target = Path::new("/music");
        let source = Path::new("/downloads");

        let result = build_destination_path(target, &info, original, source);

        assert_eq!(result, Path::new("/music/Artist - 2020 - Album/Title.ogg"));
    }

    #[test]
    fn test_build_destination_unsorted_preserves_relative_structure() {
        let info = TrackInfo::default();
        let original = Path::new("/downloads/best of 2024/disc 1/unknown.mp3");
        let target = Path::new("/music");
        let source = Path::new("/downloads");

        let result = build_destination_path(target, &info, original, source);

        assert_eq!(
            result,
            Path::new("/music/_unsorted/best of 2024/disc 1/unknown.mp3")
        );
    }

    #[test]
    fn test_build_destination_unsorted_at_source_root() {
        let info = TrackInfo::default();
        let original = Path::new("/downloads/unknown.mp3");
        let target = Path::new("/music");
        let source = Path::new("/downloads");

        let result = build_destination_path(target, &info, original, source);

        assert_eq!(result, Path::new("/music/_unsorted/unknown.mp3"));
    }

    #[test]
    fn test_build_destination_unsorted_falls_back_when_not_under_source() {
        let info = TrackInfo::default();
        let original = Path::new("/elsewhere/unknown.mp3");
        let target = Path::new("/music");
        let source = Path::new("/downloads");

        let result = build_destination_path(target, &info, original, source);

        assert_eq!(result, Path::new("/music/_unsorted/unknown.mp3"));
    }

    #[test]
    fn test_build_destination_reuses_existing_folder_case_insensitive() {
        let dir = tempdir().unwrap();
        let target = dir.path();

        // Créer un dossier existant avec "Boards Of Canada"
        std::fs::create_dir_all(target.join("Boards Of Canada - 2002 - Geogaddi")).unwrap();

        // Un fichier avec "Boards of Canada" (o minuscule) doit réutiliser le dossier existant
        let info = TrackInfo {
            artist: Some("Boards of Canada".into()),
            album: Some("Geogaddi".into()),
            title: Some("Music Is Math".into()),
            year: Some(2002),
            track_number: Some(2),
            ..Default::default()
        };

        let result = build_destination_path(
            target,
            &info,
            Path::new("song.flac"),
            Path::new("/downloads"),
        );

        assert_eq!(
            result,
            target.join("Boards Of Canada - 2002 - Geogaddi/02 - Music Is Math.flac")
        );
    }

    #[test]
    fn test_move_to_destination_same_path_is_noop() {
        // Retri sur place : source == destination ne doit JAMAIS supprimer le fichier.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("track.mp3");
        std::fs::write(&f, b"audio-bytes").unwrap();
        let result = move_to_destination(&f, &f).unwrap();
        assert!(f.exists(), "le fichier a été supprimé lors d'un move sur soi-même !");
        assert!(matches!(result, CopyResult::Skipped { .. }));
    }

    #[test]
    fn test_copy_to_destination_same_path_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("track.mp3");
        std::fs::write(&f, b"audio-bytes").unwrap();
        let result = copy_to_destination(&f, &f).unwrap();
        assert!(f.exists());
        assert!(matches!(result, CopyResult::Skipped { .. }));
    }

    #[test]
    fn test_truncate_component_ascii() {
        let long = "a".repeat(300);
        let t = truncate_component(&long, 200);
        assert_eq!(t.len(), 200);
    }

    #[test]
    fn test_truncate_component_utf8_boundary() {
        let s = "é".repeat(150); // 300 octets (é = 2 octets)
        let t = truncate_component(&s, 201);
        assert!(t.len() <= 201, "len={}", t.len());
        assert!(t.chars().all(|c| c == 'é'), "caractère coupé");
    }

    #[test]
    fn test_truncate_component_short_unchanged() {
        assert_eq!(truncate_component("short", 200), "short");
    }

    #[test]
    fn test_render_truncates_overlong_filename() {
        // Un titre démesuré ne doit pas produire un nom de fichier > limite FS.
        let info = TrackInfo {
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            year: Some(2020),
            track_number: Some(1),
            title: Some("T".repeat(400)),
            ..Default::default()
        };
        let rel = render_relative_path(DEFAULT_TEMPLATE, &info, "mp3");
        let fname = rel.file_name().unwrap().to_str().unwrap();
        assert!(fname.len() <= MAX_COMPONENT_BYTES + 5, "nom trop long : {}", fname.len());
        assert!(fname.ends_with(".mp3"));
    }

    #[test]
    fn test_render_empty_last_segment_uses_fallback_name() {
        // Si track+title manquent (segment vide), on évite un dest = dossier (EISDIR).
        let info = TrackInfo {
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            year: Some(2020),
            ..Default::default()
        };
        let rel = render_relative_path(DEFAULT_TEMPLATE, &info, "mp3");
        assert_eq!(rel.file_name().unwrap().to_str().unwrap(), "track.mp3");
    }

    #[test]
    fn test_error_destination_preserves_relative_structure() {
        let dest = error_destination(
            Path::new("/music"),
            Path::new("/src/badfolder/corrupt.mp3"),
            Path::new("/src"),
        );
        assert_eq!(dest, Path::new("/music/_errors/badfolder/corrupt.mp3"));
    }

    #[test]
    fn test_error_destination_falls_back_to_filename() {
        // Fichier hors de la racine source → simple nom sous _errors/.
        let dest = error_destination(
            Path::new("/music"),
            Path::new("/other/x.mp3"),
            Path::new("/src"),
        );
        assert_eq!(dest, Path::new("/music/_errors/x.mp3"));
    }

    #[test]
    fn test_redirect_to_review_rebases_under_review() {
        let target = Path::new("/music");
        let dest = Path::new("/music/Artist - 2020 - Album/01 - Title.mp3");
        let review = redirect_to_review(target, dest);
        assert_eq!(
            review,
            Path::new("/music/_review/Artist - 2020 - Album/01 - Title.mp3")
        );
    }

    #[test]
    fn test_redirect_to_review_noop_when_not_under_target() {
        let target = Path::new("/music");
        let dest = Path::new("/elsewhere/x.mp3");
        assert_eq!(redirect_to_review(target, dest), dest.to_path_buf());
    }

    #[test]
    fn test_render_template_full() {
        let info = TrackInfo {
            artist: Some("Boards of Canada".into()),
            album: Some("Geogaddi".into()),
            title: Some("Music Is Math".into()),
            year: Some(2002),
            track_number: Some(2),
            ..Default::default()
        };
        let rel = render_relative_path(DEFAULT_TEMPLATE, &info, "flac");
        assert_eq!(
            rel,
            Path::new("Boards of Canada - 2002 - Geogaddi/02 - Music Is Math.flac")
        );
    }

    #[test]
    fn test_render_template_no_year_collapses_separator() {
        let info = TrackInfo {
            artist: Some("BoC".into()),
            album: Some("Geogaddi".into()),
            title: Some("Track".into()),
            year: None,
            track_number: None,
            ..Default::default()
        };
        let rel = render_relative_path(DEFAULT_TEMPLATE, &info, "mp3");
        assert_eq!(rel, Path::new("BoC - Geogaddi/Track.mp3"));
    }

    #[test]
    fn test_render_template_no_track_collapses_separator() {
        let info = TrackInfo {
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            title: Some("Title".into()),
            year: Some(2020),
            track_number: None,
            ..Default::default()
        };
        let rel = render_relative_path(DEFAULT_TEMPLATE, &info, "ogg");
        assert_eq!(rel, Path::new("Artist - 2020 - Album/Title.ogg"));
    }

    #[test]
    fn test_render_template_custom_nested_artist_folder() {
        let info = TrackInfo {
            artist: Some("Aphex Twin".into()),
            album: Some("Drukqs".into()),
            title: Some("Avril 14th".into()),
            year: Some(2001),
            track_number: Some(8),
            ..Default::default()
        };
        let rel = render_relative_path(
            "{artist}/{year} - {album}/{track} - {title}",
            &info,
            "flac",
        );
        assert_eq!(
            rel,
            Path::new("Aphex Twin/2001 - Drukqs/08 - Avril 14th.flac")
        );
    }

    #[test]
    fn test_render_template_sanitizes_each_component() {
        let info = TrackInfo {
            artist: Some("AC/DC".into()),
            album: Some("Back: In Black".into()),
            title: Some("T*N*T".into()),
            year: Some(1980),
            track_number: Some(1),
            ..Default::default()
        };
        let rel = render_relative_path(DEFAULT_TEMPLATE, &info, "mp3");
        assert_eq!(
            rel,
            Path::new("AC_DC - 1980 - Back_ In Black/01 - T_N_T.mp3")
        );
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("AC/DC"), "AC_DC");
        assert_eq!(sanitize_filename("a:b*c?d"), "a_b_c_d");
        assert_eq!(sanitize_filename("normal name"), "normal name");
    }

    #[test]
    fn test_copy_with_reflink_creates_destination() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("a.mp3");
        std::fs::write(&src, b"data").unwrap();
        let dst = dir.path().join("b.mp3");
        copy_with_reflink(&src, &dst).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"data");
    }

    #[test]
    fn test_copy_to_destination_new_file() {
        let dir = tempdir().unwrap();

        // Crée le fichier source
        let source = dir.path().join("source.txt");
        let mut f = std::fs::File::create(&source).unwrap();
        f.write_all(b"hello world").unwrap();

        // Destination dans un sous-dossier qui n'existe pas encore
        let destination = dir.path().join("sub/dest.txt");

        let result = copy_to_destination(&source, &destination).unwrap();

        // Vérifie que le fichier existe avec le bon contenu
        assert!(destination.exists());
        let content = std::fs::read(&destination).unwrap();
        assert_eq!(content, b"hello world");

        // Vérifie que le résultat est Copied
        assert!(matches!(result, CopyResult::Copied));
    }

    #[test]
    fn test_move_to_destination_consumes_source() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("a.txt");
        std::fs::write(&source, b"data").unwrap();
        let dest = dir.path().join("sub/b.txt");

        let result = move_to_destination(&source, &dest).unwrap();

        assert!(matches!(result, CopyResult::Copied));
        assert!(dest.exists(), "dest doit exister");
        assert!(!source.exists(), "source doit avoir été consommée");
        assert_eq!(std::fs::read(&dest).unwrap(), b"data");
    }

    #[test]
    fn test_move_to_destination_skipped_still_consumes_source() {
        // Même si la dest existe avec un meilleur ou égal bitrate, le mode --move
        // doit consommer la source (sémantique de consolidation).
        let dir = tempdir().unwrap();
        let source = dir.path().join("a.txt");
        let dest = dir.path().join("b.txt");
        std::fs::write(&source, b"src").unwrap();
        std::fs::write(&dest, b"dst").unwrap();

        let result = move_to_destination(&source, &dest).unwrap();

        assert!(matches!(result, CopyResult::Skipped { .. }));
        assert!(!source.exists(), "source doit être supprimée même si Skipped");
        assert!(dest.exists(), "dest intacte");
        assert_eq!(std::fs::read(&dest).unwrap(), b"dst");
    }

    #[test]
    fn test_cleanup_empty_dirs_removes_only_empty() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // Arborescence : root/empty1/empty1a/, root/keep/file.txt, root/empty2/
        std::fs::create_dir_all(root.join("empty1/empty1a")).unwrap();
        std::fs::create_dir_all(root.join("keep")).unwrap();
        std::fs::write(root.join("keep/file.txt"), b"x").unwrap();
        std::fs::create_dir_all(root.join("empty2")).unwrap();

        let removed = cleanup_empty_dirs(root);

        // 3 dossiers vides supprimés (empty1, empty1a, empty2)
        assert_eq!(removed, 3);
        assert!(!root.join("empty1").exists());
        assert!(!root.join("empty2").exists());
        assert!(root.join("keep").exists(), "dossier non-vide préservé");
        assert!(root.exists(), "root jamais supprimé");
    }
}
