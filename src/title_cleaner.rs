/// Nettoie un titre avant la recherche MusicBrainz en supprimant les suffixes
/// génériques qui font rater les matchs (« Original Mix », « FREE DOWNLOAD »,
/// IDs SoundCloud, etc.) sans toucher aux noms d'artistes/remixers.
pub fn clean_for_search(title: &str) -> String {
    let mut s = title.trim().to_string();
    loop {
        let stripped = strip_one_pass(&s);
        if stripped == s {
            break;
        }
        s = stripped;
    }
    let cleaned = s.trim().to_string();
    if cleaned.is_empty() {
        title.trim().to_string()
    } else {
        cleaned
    }
}

fn strip_one_pass(s: &str) -> String {
    let s = s.trim_end_matches([' ', '\t', '-', '_', '.', ',']).trim();

    if let Some(stripped) = strip_trailing_domain(s) {
        return stripped.to_string();
    }
    if let Some(stripped) = strip_trailing_provider_marker(s) {
        return stripped.to_string();
    }
    if let Some(stripped) = strip_trailing_bracket_group(s) {
        return stripped.to_string();
    }
    if let Some(stripped) = strip_trailing_paren_group(s) {
        return stripped.to_string();
    }
    s.to_string()
}

/// Strip un token final ressemblant à un domaine / URL parasite
/// (« music-team.net », « www.site.com »). Préserve les acronymes (« R.E.M. »)
/// et ne supprime jamais l'intégralité de la chaîne.
fn strip_trailing_domain(s: &str) -> Option<&str> {
    const TLDS: &[&str] = &[
        ".net", ".com", ".org", ".fr", ".io", ".me", ".to", ".ru", ".co", ".tv",
        ".fm", ".biz", ".info", ".uk", ".de", ".es", ".it", ".nl",
    ];
    let trimmed = s.trim_end();
    let last = trimmed.rsplit(char::is_whitespace).next()?;
    let lower = last.to_lowercase();
    let looks_like_domain = lower.starts_with("www.")
        || (last.contains('.') && TLDS.iter().any(|tld| lower.ends_with(tld)));
    if !looks_like_domain {
        return None;
    }
    let rest = trimmed[..trimmed.len() - last.len()].trim_end();
    // Ne pas manger toute la chaîne (ex. un titre qui EST un domaine).
    (!rest.is_empty()).then_some(rest)
}

/// Strip un suffixe de type `_<digits>` ou `_soundcloud` ou `_youtube` ou `_bandcamp`.
fn strip_trailing_provider_marker(s: &str) -> Option<&str> {
    for provider in ["_soundcloud", "_youtube", "_bandcamp", "_spotify"] {
        if let Some(rest) = s.strip_suffix(provider) {
            return Some(rest.trim_end());
        }
    }
    let trimmed = s.trim_end();
    let idx = trimmed.rfind('_')?;
    let after = &trimmed[idx + 1..];
    if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit()) {
        return Some(trimmed[..idx].trim_end());
    }
    None
}

/// Strip un groupe entre crochets `[...]` à la fin si son contenu est du bruit.
fn strip_trailing_bracket_group(s: &str) -> Option<&str> {
    let trimmed = s.trim_end();
    let last_close = trimmed.strip_suffix(']')?;
    let open_idx = last_close.rfind('[')?;
    let content = &last_close[open_idx + 1..];
    if is_noise_keyword(content) {
        Some(trimmed[..open_idx + ']'.len_utf8() - 1].trim_end())
    } else {
        None
    }
}

/// Strip un groupe entre parenthèses `(...)` à la fin si son contenu est du bruit.
/// Préserve les annotations utiles type « (X Remix) » ou « (Y Edit) ».
fn strip_trailing_paren_group(s: &str) -> Option<&str> {
    let trimmed = s.trim_end();
    let last_close = trimmed.strip_suffix(')')?;
    let open_idx = last_close.rfind('(')?;
    let content = &last_close[open_idx + 1..];
    if is_noise_keyword(content) {
        Some(trimmed[..open_idx].trim_end())
    } else {
        None
    }
}

fn is_noise_keyword(content: &str) -> bool {
    let lower = content.trim().to_lowercase();
    matches!(
        lower.as_str(),
        "original mix"
            | "original version"
            | "original"
            | "album version"
            | "album mix"
            | "single version"
            | "single mix"
            | "radio edit"
            | "radio version"
            | "extended"
            | "extended mix"
            | "extended version"
            | "extended edit"
            | "club mix"
            | "club edit"
            | "remastered"
            | "remaster"
            | "out now"
            | "free download"
            | "free dl"
            | "free"
            | "premiere"
            | "bonus track"
            | "bonus"
            | "videoclip"
            | "video clip"
            | "official"
            | "official video"
            | "official audio"
            | "lyrics"
            | "lyric video"
            | "audio"
            | "hq"
            | "hd"
            | "320kbps"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_original_mix_paren() {
        assert_eq!(clean_for_search("Eso Es (Original Mix)"), "Eso Es");
    }

    #[test]
    fn test_strip_extended_mix_paren() {
        assert_eq!(clean_for_search("Track Name (Extended Mix)"), "Track Name");
    }

    #[test]
    fn test_strip_out_now_brackets() {
        assert_eq!(clean_for_search("Loco [OUT NOW]"), "Loco");
    }

    #[test]
    fn test_strip_free_download_paren() {
        assert_eq!(
            clean_for_search("Bidolibido (FREE DOWNLOAD)"),
            "Bidolibido"
        );
    }

    #[test]
    fn test_strip_soundcloud_id_suffix() {
        assert_eq!(
            clean_for_search("Wee Kid_260625501_soundcloud"),
            "Wee Kid"
        );
    }

    #[test]
    fn test_strip_combined_brackets_and_id() {
        assert_eq!(
            clean_for_search("Loco [OUT NOW]_290941905_soundcloud"),
            "Loco"
        );
    }

    #[test]
    fn test_preserves_remixer_name() {
        // « (Der Joe Remix) » contient un nom d'artiste : ne PAS strip
        assert_eq!(
            clean_for_search("Hey Joe (Der Joe Remix)"),
            "Hey Joe (Der Joe Remix)"
        );
    }

    #[test]
    fn test_preserves_named_edit() {
        // « (Gary Dalhson Edit 2014) » contient un nom : ne PAS strip
        assert_eq!(
            clean_for_search("Hideway (Gary Dalhson Edit 2014)"),
            "Hideway (Gary Dalhson Edit 2014)"
        );
    }

    #[test]
    fn test_strip_remastered_paren() {
        assert_eq!(clean_for_search("Song (Remastered)"), "Song");
    }

    #[test]
    fn test_strip_does_not_eat_everything() {
        // Si tout serait strippé, on garde l'original
        assert_eq!(clean_for_search("(Original Mix)"), "(Original Mix)");
    }

    #[test]
    fn test_already_clean_unchanged() {
        assert_eq!(
            clean_for_search("Music Has the Right to Children"),
            "Music Has the Right to Children"
        );
    }

    #[test]
    fn test_multiple_passes() {
        // Plusieurs passes nécessaires : strip de l'ID puis du bracket
        assert_eq!(
            clean_for_search("Track Name (Original Mix)_123456_soundcloud"),
            "Track Name"
        );
    }

    #[test]
    fn test_strip_trailing_domain() {
        assert_eq!(clean_for_search("Arabe music-team.net"), "Arabe");
        assert_eq!(clean_for_search("Some Track www.example.com"), "Some Track");
        assert_eq!(clean_for_search("Nom - dl.mp3blog.io"), "Nom");
    }

    #[test]
    fn test_domain_strip_preserves_non_domain_dots() {
        // « R.E.M. » n'est pas un domaine (pas de TLD final) : ne pas toucher.
        assert_eq!(
            clean_for_search("R.E.M. Losing My Religion"),
            "R.E.M. Losing My Religion"
        );
    }
}
