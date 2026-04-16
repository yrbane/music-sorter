use std::thread;
use std::time::Duration;

/// Exécute une closure avec retry et backoff exponentiel
/// 3 tentatives : 1s, 2s, 4s
pub fn with_retry<T, E, F>(operation: F) -> Result<T, E>
where
    F: Fn() -> Result<T, E>,
    E: std::fmt::Display,
{
    let delays = [
        Duration::from_secs(1),
        Duration::from_secs(2),
        Duration::from_secs(4),
    ];

    let mut last_err = None;

    for (i, delay) in delays.iter().enumerate() {
        match operation() {
            Ok(val) => return Ok(val),
            Err(e) => {
                eprintln!(
                    "  Tentative {}/3 échouée : {}. Retry dans {}s...",
                    i + 1, e, delay.as_secs()
                );
                last_err = Some(e);
                thread::sleep(*delay);
            }
        }
    }

    // Dernière tentative
    match operation() {
        Ok(val) => Ok(val),
        Err(e) => {
            last_err = Some(e);
            Err(last_err.unwrap())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn test_retry_succeeds_first_try() {
        let result = with_retry(|| Ok::<_, String>(42));
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_retry_succeeds_after_failures() {
        let attempts = AtomicU32::new(0);
        let result = with_retry(|| {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(format!("tentative {}", n))
            } else {
                Ok(42)
            }
        });
        assert_eq!(result.unwrap(), 42);
    }
}
