#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
pub use std::time::*;

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
mod wasm_time {
    use core::ops::{Add, AddAssign, Sub, SubAssign};
    use std::time::Duration;

    #[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub struct Instant(Duration);

    #[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub struct SystemTime(Duration);

    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub struct SystemTimeError(Duration);

    pub const UNIX_EPOCH: SystemTime = SystemTime(Duration::from_secs(0));

    impl Instant {
        pub fn now() -> Self {
            Self(duration_since_epoch())
        }

        pub fn duration_since(&self, earlier: Self) -> Duration {
            self.0.checked_sub(earlier.0).unwrap_or_default()
        }

        pub fn elapsed(&self) -> Duration {
            Self::now().duration_since(*self)
        }
    }

    impl SystemTime {
        pub const UNIX_EPOCH: SystemTime = UNIX_EPOCH;

        pub fn now() -> Self {
            Self(duration_since_epoch())
        }

        pub fn duration_since(&self, earlier: Self) -> Result<Duration, SystemTimeError> {
            self.0
                .checked_sub(earlier.0)
                .ok_or(SystemTimeError(
                    earlier.0.checked_sub(self.0).unwrap_or(Duration::ZERO),
                ))
        }
    }

    impl SystemTimeError {
        pub fn duration(&self) -> Duration {
            self.0
        }
    }

    impl Add<Duration> for Instant {
        type Output = Instant;

        fn add(self, rhs: Duration) -> Self::Output {
            Instant(self.0 + rhs)
        }
    }

    impl AddAssign<Duration> for Instant {
        fn add_assign(&mut self, rhs: Duration) {
            self.0 += rhs;
        }
    }

    impl Sub<Duration> for Instant {
        type Output = Instant;

        fn sub(self, rhs: Duration) -> Self::Output {
            Instant(self.0.checked_sub(rhs).unwrap_or_default())
        }
    }

    impl SubAssign<Duration> for Instant {
        fn sub_assign(&mut self, rhs: Duration) {
            self.0 = self.0.checked_sub(rhs).unwrap_or_default();
        }
    }

    impl Sub<Instant> for Instant {
        type Output = Duration;

        fn sub(self, rhs: Instant) -> Self::Output {
            self.duration_since(rhs)
        }
    }

    impl Add<Duration> for SystemTime {
        type Output = SystemTime;

        fn add(self, rhs: Duration) -> Self::Output {
            SystemTime(self.0 + rhs)
        }
    }

    impl Sub<Duration> for SystemTime {
        type Output = SystemTime;

        fn sub(self, rhs: Duration) -> Self::Output {
            SystemTime(self.0.checked_sub(rhs).unwrap_or_default())
        }
    }

    fn duration_since_epoch() -> Duration {
        let seconds = miniquad::date::now();
        if seconds <= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(seconds)
        }
    }
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
pub use wasm_time::*;
