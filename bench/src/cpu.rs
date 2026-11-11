//! Native process CPU accounting. Durations include all process threads, but not children.
//!
//! Output units do not imply measurement precision. Run separately from Valgrind and avoid
//! concurrent benchmark cases in the same process.

use serde::Serialize;
use std::{io, time::Instant};

/// One process-wide measurement interval.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Sample {
    /// Elapsed time in nanoseconds.
    pub wall_ns: u64,
    /// User CPU time in nanoseconds.
    pub user_cpu_ns: u64,
    /// Kernel CPU time in nanoseconds.
    pub system_cpu_ns: u64,
    /// Voluntary context switches.
    pub voluntary_context_switches: u64,
    /// Involuntary context switches.
    pub involuntary_context_switches: u64,
    /// Page faults not requiring I/O on Linux.
    pub minor_page_faults: u64,
    /// Page faults requiring I/O on Linux.
    pub major_page_faults: u64,
}

/// Values derived from the original integer measurements.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Normalized {
    /// Total user and kernel CPU nanoseconds.
    pub total_cpu_ns: u64,
    /// CPU nanoseconds per measured batch.
    pub cpu_ns_per_batch: f64,
    /// CPU nanoseconds per submitted write attempt, including duplicates.
    pub cpu_ns_per_write: f64,
    /// Mean number of occupied cores.
    pub average_cores_used: f64,
    /// CPU percentage; one occupied core is 100%.
    pub cpu_usage_percent: f64,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

impl Sample {
    /// Normalize a sample without truncating ratios. Zero denominators are rejected.
    pub fn normalize(&self, batches: u64, writes: u64) -> io::Result<Normalized> {
        if self.wall_ns == 0 || batches == 0 || writes == 0 {
            return Err(invalid(
                "normalization requires positive wall time, batches and writes",
            ));
        }
        let total_cpu_ns = self
            .user_cpu_ns
            .checked_add(self.system_cpu_ns)
            .ok_or_else(|| invalid("CPU time overflow"))?;
        let average_cores_used = total_cpu_ns as f64 / self.wall_ns as f64;
        Ok(Normalized {
            total_cpu_ns,
            cpu_ns_per_batch: total_cpu_ns as f64 / batches as f64,
            cpu_ns_per_write: total_cpu_ns as f64 / writes as f64,
            average_cores_used,
            cpu_usage_percent: 100.0 * average_cores_used,
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Counters {
    user: u64,
    system: u64,
    voluntary: u64,
    involuntary: u64,
    minor: u64,
    major: u64,
}

/// Explicit start/finish accounting; dropping it does not sample or emit metrics.
pub struct Measurement {
    counters: Counters,
    instant: Instant,
}

impl Measurement {
    /// Snapshot process accounting, then start the wall timer.
    pub fn start() -> io::Result<Self> {
        let counters = read_counters()?;
        Ok(Self {
            counters,
            instant: Instant::now(),
        })
    }

    /// Stop the wall timer, then snapshot accounting. Snapshot overhead is not subtracted.
    pub fn finish(self) -> io::Result<Sample> {
        let elapsed = self.instant.elapsed();
        let end = read_counters()?;
        difference(
            self.counters,
            end,
            u64::try_from(elapsed.as_nanos()).map_err(|_| invalid("wall time overflow"))?,
        )
    }
}

fn difference(start: Counters, end: Counters, wall_ns: u64) -> io::Result<Sample> {
    let delta = |a: u64, b: u64| {
        b.checked_sub(a)
            .ok_or_else(|| invalid("process counter decreased"))
    };
    Ok(Sample {
        wall_ns,
        user_cpu_ns: delta(start.user, end.user)?,
        system_cpu_ns: delta(start.system, end.system)?,
        voluntary_context_switches: delta(start.voluntary, end.voluntary)?,
        involuntary_context_switches: delta(start.involuntary, end.involuntary)?,
        minor_page_faults: delta(start.minor, end.minor)?,
        major_page_faults: delta(start.major, end.major)?,
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn timeval_ns(seconds: i64, micros: i64) -> io::Result<u64> {
    if seconds < 0 || !(0..1_000_000).contains(&micros) {
        return Err(invalid("invalid process timeval"));
    }
    (seconds as u64)
        .checked_mul(1_000_000_000)
        .and_then(|s| s.checked_add(micros as u64 * 1_000))
        .ok_or_else(|| invalid("CPU time overflow"))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn read_counters() -> io::Result<Counters> {
    read_with(|usage| {
        // SAFETY: read_with supplies a valid, writable rusage pointer for this call.
        unsafe { libc::getrusage(libc::RUSAGE_SELF, usage) }
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn read_with(call: impl FnOnce(*mut libc::rusage) -> libc::c_int) -> io::Result<Counters> {
    // Zero initialization also keeps injected syscall-failure tests safe.
    // SAFETY: rusage contains only integers and timevals, for which zero is valid.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if call(&mut usage) != 0 {
        return Err(io::Error::last_os_error());
    }
    let counter =
        |value: libc::c_long| u64::try_from(value).map_err(|_| invalid("negative process counter"));
    Ok(Counters {
        user: timeval_ns(usage.ru_utime.tv_sec as i64, usage.ru_utime.tv_usec as i64)?,
        system: timeval_ns(usage.ru_stime.tv_sec as i64, usage.ru_stime.tv_usec as i64)?,
        voluntary: counter(usage.ru_nvcsw)?,
        involuntary: counter(usage.ru_nivcsw)?,
        minor: counter(usage.ru_minflt)?,
        major: counter(usage.ru_majflt)?,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_counters() -> io::Result<Counters> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "native CPU accounting supports Linux and macOS",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_deltas_and_normalization() {
        let sample = difference(
            Counters {
                user: 900,
                ..Counters::default()
            },
            Counters {
                user: 1_300,
                system: 400,
                ..Counters::default()
            },
            200,
        )
        .unwrap();
        let normalized = sample.normalize(2, 10).unwrap();
        assert_eq!(normalized.total_cpu_ns, 800);
        assert_eq!(normalized.average_cores_used, 4.0);
        assert_eq!(normalized.cpu_usage_percent, 400.0);
        assert_eq!(normalized.cpu_ns_per_batch, 400.0);
        assert_eq!(normalized.cpu_ns_per_write, 80.0);
        assert!(sample.normalize(0, 1).is_err());
        assert!(sample.normalize(1, 0).is_err());
        assert!(Sample::default().normalize(1, 1).is_err());
        assert!(
            Sample {
                user_cpu_ns: u64::MAX,
                system_cpu_ns: 1,
                wall_ns: 1,
                ..Sample::default()
            }
            .normalize(1, 1)
            .is_err()
        );
        assert!(
            difference(
                Counters {
                    minor: 1,
                    ..Counters::default()
                },
                Counters::default(),
                1
            )
            .is_err()
        );
        assert_eq!(
            Sample {
                wall_ns: 1,
                ..Sample::default()
            }
            .normalize(1, 1)
            .unwrap()
            .total_cpu_ns,
            0
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn timeval_borrow_and_syscall_failure() {
        assert_eq!(
            timeval_ns(2, 100).unwrap() - timeval_ns(1, 999_900).unwrap(),
            200_000
        );
        assert!(timeval_ns(-1, 0).is_err());
        assert!(timeval_ns(0, 1_000_000).is_err());
        assert!(timeval_ns(i64::MAX, 0).is_err());
        assert!(read_with(|_| -1).is_err());
    }

    #[cfg(all(not(miri), any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn native_worker_smoke() {
        let measurement = Measurement::start().unwrap();
        std::thread::spawn(|| {
            for n in 0..5_000_000u64 {
                std::hint::black_box(n.wrapping_mul(n));
            }
        })
        .join()
        .unwrap();
        let sample = measurement.finish().unwrap();
        assert!(sample.wall_ns > 0);
        assert!(sample.user_cpu_ns + sample.system_cpu_ns > 0);
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn unsupported_platform() {
        assert!(matches!(Measurement::start(), Err(e) if e.kind() == io::ErrorKind::Unsupported));
    }
}
