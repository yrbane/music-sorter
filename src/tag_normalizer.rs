//! Normalisation des tags avant rangement : corrige les métadonnées bancales
//! héritées des rips/exports (titre portant l'extension, artiste caché dans le
//! titre d'une compilation, album préfixé du catalogue, groupes répétés).

use crate::models::{SUPPORTED_EXTENSIONS, TrackInfo, is_various_artists};

/// Applique toutes les normalisations à `info` (idempotent) et lève
/// `info.normalized` si artiste, titre ou album ont changé.
pub fn normalize(info: &mut TrackInfo) {
    let before = (info.artist.clone(), info.title.clone(), info.album.clone());
    if let Some(title) = info.title.take() {
        info.title = Some(dedupe_trailing_groups(&strip_audio_extension(&title)));
    }
    // Piste de compilation dont l'artiste réel est caché dans le titre
    // (« 100 Soda303 - Red World » avec artist = Various Artists).
    let artist_is_placeholder = info
        .artist
        .as_deref()
        .map(is_various_artists)
        .unwrap_or(true);
    if artist_is_placeholder
        && let Some((artist, title)) = info
            .title
            .as_deref()
            .and_then(|t| split_artist_from_title(t, info.track_number))
    {
        info.artist = Some(artist);
        info.title = Some(title);
    }
    // Numéro de piste redondant en tête de titre (« 90 Titre » sur la piste 90).
    if let Some(title) = info.title.as_deref() {
        let stripped = strip_leading_track_number(title, info.track_number);
        if stripped.len() != title.len() {
            info.title = Some(stripped.to_string());
        }
    }
    if let (Some(album), Some(artist)) = (info.album.as_deref(), info.artist.as_deref()) {
        info.album = Some(clean_album_title(album, artist));
    }
    if (info.artist.clone(), info.title.clone(), info.album.clone()) != before {
        info.normalized = true;
    }
}

/// Retire une extension audio collée au titre (« Red World.wav »).
pub fn strip_audio_extension(title: &str) -> String {
    let lower = title.to_lowercase();
    for ext in SUPPORTED_EXTENSIONS.iter().chain(["aiff", "aif"].iter()) {
        let suffix = format!(".{ext}");
        if lower.ends_with(&suffix) && lower.len() > suffix.len() {
            return title[..title.len() - suffix.len()].trim_end().to_string();
        }
    }
    title.to_string()
}

/// Sépare « [N ]Artiste - Titre » en (artiste, titre). Le numéro initial n'est
/// retiré que s'il correspond au numéro de piste (« 3 Chords » reste intact).
pub fn split_artist_from_title(title: &str, track_number: Option<u32>) -> Option<(String, String)> {
    // « A - B » d'abord ; sinon le séparateur bancal « A -B » (espace avant le
    // tiret seulement). « A-B » sans espace est un nom, jamais coupé.
    let (artist, rest) = title.split_once(" - ").or_else(|| {
        title
            .split_once(" -")
            .filter(|(_, r)| !r.starts_with(['-', ' ']))
    })?;
    let (artist, rest) = (artist.trim(), rest.trim());
    if artist.is_empty() || rest.is_empty() {
        return None;
    }
    let artist = strip_leading_track_number(artist, track_number);
    if artist.is_empty() {
        return None;
    }
    Some((artist.to_string(), rest.to_string()))
}

fn strip_leading_track_number(s: &str, track_number: Option<u32>) -> &str {
    let Some(track) = track_number else { return s };
    let digits_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if digits_end == 0 || digits_end == s.len() {
        return s;
    }
    let Ok(n) = s[..digits_end].parse::<u32>() else {
        return s;
    };
    let rest = s[digits_end..].trim_start_matches([' ', '.', '-', '_']);
    if n == track && rest.len() < s.len() - digits_end && !rest.is_empty() {
        rest
    } else {
        s
    }
}

/// Retire un préfixe « Artiste - » ou « CATNO Artiste - » du nom d'album
/// (« ANJDEE234 Cubicolor - Down The Wall EP » → « Down The Wall EP »).
pub fn clean_album_title(album: &str, artist: &str) -> String {
    let artist = artist.trim();
    if artist.is_empty() {
        return album.to_string();
    }
    let needle = format!("{artist} - ");
    let stripped = strip_prefix_ci(album, &needle).or_else(|| {
        let (first, rest) = album.split_once(' ')?;
        if is_catalog_token(first) {
            strip_prefix_ci(rest, &needle)
        } else {
            None
        }
    });
    match stripped.map(str::trim) {
        Some(rest) if !rest.is_empty() => rest.to_string(),
        _ => album.to_string(),
    }
}

/// `strip_prefix` insensible à la casse, sûr vis-à-vis des frontières UTF-8.
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let mut chars = s.char_indices();
    for pc in prefix.chars() {
        let (_, sc) = chars.next()?;
        if !sc.to_lowercase().eq(pc.to_lowercase()) {
            return None;
        }
    }
    let end = chars.next().map(|(i, _)| i).unwrap_or(s.len());
    Some(&s[end..])
}

/// Numéro de catalogue : token alphanumérique (tirets admis) contenant un chiffre.
fn is_catalog_token(token: &str) -> bool {
    token.len() >= 3
        && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && token.chars().any(|c| c.is_ascii_digit())
}

/// Supprime les groupes finaux « (…) » / « […] » dont le contenu apparaît déjà
/// plus tôt dans le titre (« Song (Live) (Live) » → « Song (Live) »).
pub fn dedupe_trailing_groups(title: &str) -> String {
    let mut s = title.trim().to_string();
    while let Some((head, content)) = split_trailing_group(&s) {
        if head.is_empty() || !head.to_lowercase().contains(&content.to_lowercase()) {
            break;
        }
        s = head.to_string();
    }
    s
}

fn split_trailing_group(s: &str) -> Option<(&str, &str)> {
    let (open, close) = match s.chars().last()? {
        ')' => ('(', ')'),
        ']' => ('[', ']'),
        _ => return None,
    };
    let start = s.rfind(open)?;
    let content = s[start + 1..s.len() - close.len_utf8()].trim();
    if content.is_empty() {
        return None;
    }
    Some((s[..start].trim_end(), content))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TrackInfo;

    #[test]
    fn test_strip_audio_extension_from_title() {
        assert_eq!(strip_audio_extension("Red World.wav"), "Red World");
        assert_eq!(strip_audio_extension("Round1.WAV"), "Round1");
        assert_eq!(strip_audio_extension("Song.flac"), "Song");
        assert_eq!(strip_audio_extension("Mr. Wav"), "Mr. Wav");
        assert_eq!(strip_audio_extension("Piano..."), "Piano...");
    }

    #[test]
    fn test_split_artist_from_title_strips_matching_track_number() {
        let r = split_artist_from_title("1 1qlp - U lil' punk", Some(1));
        assert_eq!(r, Some(("1qlp".into(), "U lil' punk".into())));
    }

    #[test]
    fn test_split_artist_from_title_keeps_number_when_not_track_number() {
        let r = split_artist_from_title("3 Chords - Live", Some(6));
        assert_eq!(r, Some(("3 Chords".into(), "Live".into())));
    }

    #[test]
    fn test_split_artist_from_title_without_separator() {
        assert_eq!(split_artist_from_title("Something", Some(1)), None);
        assert_eq!(split_artist_from_title(" - Empty", Some(1)), None);
    }

    #[test]
    fn test_clean_album_title_strips_catalog_and_artist_prefix() {
        assert_eq!(
            clean_album_title("ANJDEE234 Cubicolor - Down The Wall EP", "Cubicolor"),
            "Down The Wall EP"
        );
        assert_eq!(
            clean_album_title("Cubicolor - Down The Wall EP", "cubicolor"),
            "Down The Wall EP"
        );
        assert_eq!(
            clean_album_title("Down The Wall EP", "Cubicolor"),
            "Down The Wall EP"
        );
        assert_eq!(
            clean_album_title("Odyssey / Sonne", "Rival Consoles"),
            "Odyssey / Sonne"
        );
        // Le préfixe doit être le nom d'artiste entier, pas une sous-chaîne.
        assert_eq!(
            clean_album_title("Cubicolors - Live", "Cubicolor"),
            "Cubicolors - Live"
        );
    }

    #[test]
    fn test_dedupe_trailing_groups() {
        assert_eq!(
            dedupe_trailing_groups("Recovery (Vessels Remix) [Bonus Track] (Vessels Remix)"),
            "Recovery (Vessels Remix) [Bonus Track]"
        );
        assert_eq!(
            dedupe_trailing_groups("Soul (Bonus Track) [feat. Peter Broderick] (Bonus Track)"),
            "Soul (Bonus Track) [feat. Peter Broderick]"
        );
        assert_eq!(dedupe_trailing_groups("Song (Live) (Live)"), "Song (Live)");
        assert_eq!(dedupe_trailing_groups("Song (Remix)"), "Song (Remix)");
        assert_eq!(dedupe_trailing_groups("Song"), "Song");
    }

    /// normalize() : piste de compilation « Various Artists » dont le titre
    /// embarque numéro, artiste et extension.
    #[test]
    fn test_normalize_va_track_with_embedded_artist() {
        let mut info = TrackInfo {
            artist: Some("Various Artists".into()),
            album_artist: Some("Various Artists".into()),
            album: Some("Tekaid Chocó".into()),
            title: Some("100 Soda303 - Red World.wav".into()),
            track_number: Some(100),
            ..Default::default()
        };
        normalize(&mut info);
        assert_eq!(info.artist.as_deref(), Some("Soda303"));
        assert_eq!(info.title.as_deref(), Some("Red World"));
        assert_eq!(info.album.as_deref(), Some("Tekaid Chocó"));
    }

    /// normalize() ne touche pas au titre d'un artiste réel, même avec « - ».
    #[test]
    fn test_normalize_leaves_real_artist_title_alone() {
        let mut info = TrackInfo {
            artist: Some("Rival Consoles".into()),
            title: Some("Recovery - Vessels remix".into()),
            album: Some("ANJDEE234 Rival Consoles - Howl".into()),
            ..Default::default()
        };
        normalize(&mut info);
        assert_eq!(info.artist.as_deref(), Some("Rival Consoles"));
        assert_eq!(info.title.as_deref(), Some("Recovery - Vessels remix"));
        assert_eq!(info.album.as_deref(), Some("Howl"));
    }

    /// normalize() signale s'il a modifié quelque chose (pour réécrire les tags).
    #[test]
    fn test_normalize_reports_changes() {
        let mut clean = TrackInfo {
            artist: Some("Rival Consoles".into()),
            title: Some("Howl".into()),
            album: Some("Howl".into()),
            ..Default::default()
        };
        normalize(&mut clean);
        assert!(!clean.normalized);

        let mut dirty = TrackInfo {
            artist: Some("Rival Consoles".into()),
            title: Some("Howl.wav".into()),
            ..Default::default()
        };
        normalize(&mut dirty);
        assert!(dirty.normalized);
    }

    /// Séparateur bancal « Artiste -Titre » (espace avant le tiret seulement).
    #[test]
    fn test_split_artist_from_title_loose_separator() {
        assert_eq!(
            split_artist_from_title("113 Tosam -Skarsnik", Some(113)),
            Some(("Tosam".into(), "Skarsnik".into()))
        );
        assert_eq!(
            split_artist_from_title("97 Shindo -Aeterna", Some(97)),
            Some(("Shindo".into(), "Aeterna".into()))
        );
        // Un tiret sans espace est un nom (« Sane-Trudge », « Pan_demi_CK »).
        assert_eq!(split_artist_from_title("94 Sane-Trudge", Some(94)), None);
    }

    /// Sans séparateur, normalize() retire au moins le numéro de piste
    /// redondant en tête de titre (« 90 RohmInHood … », piste 90).
    #[test]
    fn test_normalize_strips_redundant_track_number_without_split() {
        let mut info = TrackInfo {
            artist: Some("Various Artists".into()),
            title: Some("90 RohmInHood F...ing Bitch".into()),
            track_number: Some(90),
            ..Default::default()
        };
        normalize(&mut info);
        assert_eq!(info.artist.as_deref(), Some("Various Artists"));
        assert_eq!(info.title.as_deref(), Some("RohmInHood F...ing Bitch"));
        assert!(info.normalized);

        // Numéro différent du numéro de piste : intact (« 2 Become 1 »).
        let mut keep = TrackInfo {
            title: Some("2 Become 1".into()),
            track_number: Some(7),
            ..Default::default()
        };
        normalize(&mut keep);
        assert_eq!(keep.title.as_deref(), Some("2 Become 1"));
    }
}
