use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Tracks vault lock state and inactivity timeout.
pub struct LockManager {
    locked: AtomicBool,
    last_activity: Mutex<Instant>,
    timeout: Mutex<Duration>,
}

impl LockManager {
    pub fn new(timeout_minutes: u16) -> Self {
        Self {
            locked: AtomicBool::new(true),
            last_activity: Mutex::new(Instant::now()),
            timeout: Mutex::new(Duration::from_secs(timeout_minutes as u64 * 60)),
        }
    }

    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Acquire)
    }

    pub fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
        self.touch();
    }

    pub fn lock(&self) {
        self.locked.store(true, Ordering::Release);
    }

    /// Record activity to reset the inactivity timer.
    pub fn touch(&self) {
        *self.last_activity.lock() = Instant::now();
    }

    /// Check if the inactivity timeout has elapsed. If so, lock and return true.
    pub fn check_timeout(&self) -> bool {
        if self.is_locked() {
            return false;
        }
        let elapsed = self.last_activity.lock().elapsed();
        if elapsed >= *self.timeout.lock() {
            self.lock();
            true
        } else {
            false
        }
    }

    /// Returns seconds remaining before auto-lock, or 0 if locked.
    pub fn seconds_remaining(&self) -> u64 {
        if self.is_locked() {
            return 0;
        }
        let elapsed = self.last_activity.lock().elapsed();
        let timeout = *self.timeout.lock();
        timeout.saturating_sub(elapsed).as_secs()
    }

    pub fn set_timeout_minutes(&self, minutes: u16) {
        *self.timeout.lock() = Duration::from_secs(minutes as u64 * 60);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_locked() {
        let lm = LockManager::new(15);
        assert!(lm.is_locked());
    }

    #[test]
    fn unlock_and_lock() {
        let lm = LockManager::new(15);
        lm.unlock();
        assert!(!lm.is_locked());
        lm.lock();
        assert!(lm.is_locked());
    }

    #[test]
    fn check_timeout_does_nothing_when_locked() {
        let lm = LockManager::new(0); // 0 min timeout
        assert!(!lm.check_timeout()); // already locked, returns false
    }

    #[test]
    fn seconds_remaining_zero_when_locked() {
        let lm = LockManager::new(15);
        assert_eq!(lm.seconds_remaining(), 0);
    }

    #[test]
    fn seconds_remaining_positive_when_unlocked() {
        let lm = LockManager::new(15);
        lm.unlock();
        assert!(lm.seconds_remaining() > 0);
    }

    #[test]
    fn set_timeout_minutes_updates_duration() {
        let lm = LockManager::new(15);
        lm.unlock();
        lm.set_timeout_minutes(30);
        assert!(lm.seconds_remaining() > 15 * 60);
    }
}
