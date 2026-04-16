use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Limite le rythme des appels aux APIs externes (ex: AcoustID, MusicBrainz)
pub struct RateLimiter {
    min_interval: Duration,
    last_request: Mutex<Instant>,
}

impl RateLimiter {
    /// Crée un nouveau limiteur avec l'intervalle minimum entre chaque requête
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            // Initialise dans le passé pour que le premier appel soit immédiat
            last_request: Mutex::new(Instant::now() - min_interval),
        }
    }

    /// Attend si nécessaire pour respecter l'intervalle minimum, puis met à jour l'horodatage
    pub fn wait(&self) {
        let mut last = self.last_request.lock().unwrap();
        let elapsed = last.elapsed();
        if elapsed < self.min_interval {
            std::thread::sleep(self.min_interval - elapsed);
        }
        *last = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_first_call_no_wait() {
        let limiter = RateLimiter::new(Duration::from_secs(1));
        let start = Instant::now();
        limiter.wait();
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn test_rate_limiter_enforces_delay() {
        let limiter = RateLimiter::new(Duration::from_millis(100));
        limiter.wait();
        let start = Instant::now();
        limiter.wait();
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(80));
    }
}
