use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::domain::config::KeyStrategy;

const EXHAUST_TTL: Duration = Duration::from_secs(5 * 60);

pub struct KeyPool {
    states: Vec<KeyState>,
    cursor: AtomicUsize,
    strategy: KeyStrategy,
}

struct KeyState {
    key: String,
    exhausted_until: Mutex<Option<Instant>>,
}

impl KeyPool {
    pub fn new(keys: Vec<String>, strategy: KeyStrategy) -> Self {
        let states = keys
            .into_iter()
            .map(|key| KeyState {
                key,
                exhausted_until: Mutex::new(None),
            })
            .collect();
        Self {
            states,
            cursor: AtomicUsize::new(0),
            strategy,
        }
    }

    pub fn len(&self) -> usize {
        self.states.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    pub async fn acquire(&self) -> Option<String> {
        if self.states.is_empty() {
            return None;
        }

        let start = self.cursor.load(Ordering::Acquire);
        let n = self.states.len();

        for offset in 0..n {
            let idx = (start + offset) % n;
            let state = &self.states[idx];

            let mut guard = state.exhausted_until.lock().await;
            if let Some(until) = *guard {
                if Instant::now() < until {
                    continue;
                }
                *guard = None;
            }

            match self.strategy {
                KeyStrategy::RoundRobin => {
                    let _ = self.cursor.compare_exchange_weak(
                        start,
                        (idx + 1) % n,
                        Ordering::Release,
                        Ordering::Relaxed,
                    );
                }
                KeyStrategy::Failover => {}
            }

            return Some(state.key.clone());
        }

        None
    }

    pub async fn mark_exhausted(&self, key: &str) {
        for state in &self.states {
            if state.key == key {
                let mut guard = state.exhausted_until.lock().await;
                *guard = Some(Instant::now() + EXHAUST_TTL);
                return;
            }
        }
    }

    #[allow(dead_code)]
    pub async fn reset(&self) {
        for state in &self.states {
            let mut guard = state.exhausted_until.lock().await;
            *guard = None;
        }
    }
}
