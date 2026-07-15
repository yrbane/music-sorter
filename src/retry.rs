use std::thread;
use std::time::Duration;

/// Plafond du délai d'attente imposé par un serveur (Retry-After) pour éviter
/// de bloquer un worker trop longtemps.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

/// Politique de retry : les délais de base appliqués entre deux tentatives.
/// Le nombre total de tentatives vaut `delays.len() + 1`.
pub struct RetryPolicy {
    delays: Vec<Duration>,
}

impl RetryPolicy {
    /// Politique réseau par défaut : 3 tentatives, backoff exponentiel 1s puis 2s.
    pub fn network() -> Self {
        Self {
            delays: vec![Duration::from_secs(1), Duration::from_secs(2)],
        }
    }

    /// Construit une politique à partir d'une liste de délais explicite.
    #[cfg(test)]
    pub fn from_delays(delays: Vec<Duration>) -> Self {
        Self { delays }
    }

    /// Politique sans aucun délai : `attempts` tentatives instantanées.
    #[cfg(test)]
    pub fn no_delay(attempts: usize) -> Self {
        Self {
            delays: vec![Duration::ZERO; attempts.saturating_sub(1)],
        }
    }
}

/// Issue d'une tentative du point de vue du pilote de retry.
pub enum Outcome<T> {
    /// Succès terminal : la valeur est renvoyée telle quelle.
    Success(T),
    /// Échec transitoire : réessayer, en respectant un éventuel délai serveur.
    Transient { retry_after: Option<Duration> },
    /// Échec définitif : abandonner immédiatement, sans réessayer.
    Permanent,
}

/// Décision de retry déduite d'un statut HTTP et d'un éventuel en-tête Retry-After.
#[derive(Debug, PartialEq)]
pub enum Disposition {
    Success,
    Transient(Option<Duration>),
    Permanent,
}

/// Parse un en-tête `Retry-After` exprimé en secondes (delta-seconds).
/// Le format HTTP-date n'est pas géré et retourne `None`.
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Un statut HTTP est transitoire (donc justifie un retry) s'il vaut 429 ou 5xx.
pub fn is_transient_status(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

/// Classe un statut HTTP (et son éventuel Retry-After brut) en une décision de retry.
pub fn classify(status: u16, retry_after_header: Option<&str>) -> Disposition {
    if (200..300).contains(&status) {
        Disposition::Success
    } else if is_transient_status(status) {
        Disposition::Transient(retry_after_header.and_then(parse_retry_after))
    } else {
        Disposition::Permanent
    }
}

/// Calcule le délai avant la prochaine tentative : le Retry-After serveur prime
/// (plafonné à `MAX_RETRY_AFTER`), sinon on applique le backoff de la politique.
pub fn backoff_delay(
    policy: &RetryPolicy,
    attempt: usize,
    retry_after: Option<Duration>,
) -> Duration {
    if let Some(ra) = retry_after {
        return ra.min(MAX_RETRY_AFTER);
    }
    policy.delays.get(attempt).copied().unwrap_or(Duration::ZERO)
}

/// Pilote une opération avec retry : renvoie `Some(T)` au premier succès,
/// `None` en cas d'échec définitif ou après épuisement des tentatives.
pub fn run<T, F>(policy: &RetryPolicy, mut op: F) -> Option<T>
where
    F: FnMut() -> Outcome<T>,
{
    let max_attempts = policy.delays.len() + 1;
    for attempt in 0..max_attempts {
        match op() {
            Outcome::Success(value) => return Some(value),
            Outcome::Permanent => return None,
            Outcome::Transient { retry_after } => {
                if attempt == max_attempts - 1 {
                    return None; // dernière tentative épuisée
                }
                let delay = backoff_delay(policy, attempt, retry_after);
                if !delay.is_zero() {
                    eprintln!(
                        "  Tentative {}/{} échouée. Nouvel essai dans {}s…",
                        attempt + 1,
                        max_attempts,
                        delay.as_secs()
                    );
                }
                sleep(delay);
            }
        }
    }
    None
}

/// Endort le thread courant, en ignorant les délais nuls (utile en test).
fn sleep(delay: Duration) {
    if !delay.is_zero() {
        thread::sleep(delay);
    }
}

/// Exécute un GET HTTP avec rate-limiting optionnel et retry sur erreurs
/// transitoires (réseau, 429, 5xx). Renvoie la réponse en cas de succès,
/// `None` sur échec définitif ou après épuisement des tentatives.
///
/// `make_request` reconstruit la requête à chaque tentative (le corps d'une
/// `RequestBuilder` n'étant pas rejouable).
pub fn get_with_retry(
    rate_limiter: Option<&crate::rate_limiter::RateLimiter>,
    make_request: impl Fn() -> reqwest::blocking::RequestBuilder,
) -> Option<reqwest::blocking::Response> {
    run(&RetryPolicy::network(), || {
        if let Some(rl) = rate_limiter {
            rl.wait();
        }
        match make_request().send() {
            Ok(response) => classify_response(response),
            // Erreur réseau (timeout, connexion) : transitoire, on réessaie.
            Err(_) => Outcome::Transient { retry_after: None },
        }
    })
}

/// Convertit une réponse HTTP en `Outcome` en s'appuyant sur [`classify`].
fn classify_response(response: reqwest::blocking::Response) -> Outcome<reqwest::blocking::Response> {
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok());
    match classify(status, retry_after) {
        Disposition::Success => Outcome::Success(response),
        Disposition::Transient(after) => Outcome::Transient { retry_after: after },
        Disposition::Permanent => Outcome::Permanent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn test_parse_retry_after_seconds() {
        assert_eq!(parse_retry_after("5"), Some(Duration::from_secs(5)));
        assert_eq!(parse_retry_after("  10 "), Some(Duration::from_secs(10)));
    }

    #[test]
    fn test_parse_retry_after_http_date_unsupported() {
        assert_eq!(parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after(""), None);
    }

    #[test]
    fn test_is_transient_status() {
        assert!(is_transient_status(429));
        assert!(is_transient_status(500));
        assert!(is_transient_status(503));
        assert!(!is_transient_status(200));
        assert!(!is_transient_status(404));
        assert!(!is_transient_status(401));
    }

    #[test]
    fn test_classify_success() {
        assert_eq!(classify(200, None), Disposition::Success);
    }

    #[test]
    fn test_classify_429_with_retry_after() {
        assert_eq!(
            classify(429, Some("7")),
            Disposition::Transient(Some(Duration::from_secs(7)))
        );
    }

    #[test]
    fn test_classify_503_transient_no_header() {
        assert_eq!(classify(503, None), Disposition::Transient(None));
    }

    #[test]
    fn test_classify_404_permanent() {
        assert_eq!(classify(404, None), Disposition::Permanent);
    }

    #[test]
    fn test_backoff_uses_policy_delay() {
        let policy =
            RetryPolicy::from_delays(vec![Duration::from_secs(1), Duration::from_secs(2)]);
        assert_eq!(backoff_delay(&policy, 0, None), Duration::from_secs(1));
        assert_eq!(backoff_delay(&policy, 1, None), Duration::from_secs(2));
    }

    #[test]
    fn test_backoff_retry_after_wins_and_caps() {
        let policy = RetryPolicy::from_delays(vec![Duration::from_secs(1)]);
        assert_eq!(
            backoff_delay(&policy, 0, Some(Duration::from_secs(5))),
            Duration::from_secs(5)
        );
        // Plafonné à 30s
        assert_eq!(
            backoff_delay(&policy, 0, Some(Duration::from_secs(120))),
            MAX_RETRY_AFTER
        );
    }

    #[test]
    fn test_run_success_first_try() {
        let policy = RetryPolicy::no_delay(3);
        let result = run(&policy, || Outcome::Success(42));
        assert_eq!(result, Some(42));
    }

    #[test]
    fn test_run_succeeds_after_transient() {
        let policy = RetryPolicy::no_delay(3);
        let attempts = Cell::new(0);
        let result = run(&policy, || {
            let n = attempts.get();
            attempts.set(n + 1);
            if n < 2 {
                Outcome::Transient { retry_after: None }
            } else {
                Outcome::Success(n)
            }
        });
        assert_eq!(result, Some(2));
        assert_eq!(attempts.get(), 3);
    }

    #[test]
    fn test_run_permanent_stops_immediately() {
        let policy = RetryPolicy::no_delay(3);
        let attempts = Cell::new(0);
        let result: Option<i32> = run(&policy, || {
            attempts.set(attempts.get() + 1);
            Outcome::Permanent
        });
        assert_eq!(result, None);
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn test_run_exhausts_and_gives_up() {
        let policy = RetryPolicy::no_delay(3);
        let attempts = Cell::new(0);
        let result: Option<i32> = run(&policy, || {
            attempts.set(attempts.get() + 1);
            Outcome::Transient { retry_after: None }
        });
        assert_eq!(result, None);
        assert_eq!(attempts.get(), 3);
    }
}
