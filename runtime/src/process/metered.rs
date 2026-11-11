//! Process metrics collection.

use crate::telemetry::metrics::{Gauge, GaugeExt, GaugeValue, Register, Scope, raw};
use std::{
    future::Future,
    time::{Duration, Instant},
};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

/// The interval at which to update process metrics.
const TICK_INTERVAL: Duration = Duration::from_secs(10);

/// Process metrics collector.
pub struct Metrics {
    /// Resident set size in bytes.
    pub rss: Gauge,
    /// Virtual memory size in bytes.
    pub virtual_memory: Gauge,

    /// Process ID.
    pid: sysinfo::Pid,
    /// System information handle.
    system: System,
    cpu_scope: Scope,
    cpu_usage: Option<Gauge>,
    cpu_monitoring: bool,
    last_cpu_refresh: Option<Instant>,
}

impl Metrics {
    /// Initialize process metrics and register them with the given registry.
    pub fn init(registry: &mut impl Register) -> Self {
        Self {
            pid: sysinfo::Pid::from_u32(std::process::id()),
            rss: registry.register(
                "process_rss",
                "Resident set size of the current process",
                raw::Gauge::default(),
            ),
            virtual_memory: registry.register(
                "process_virtual_memory",
                "Virtual memory size of the current process",
                raw::Gauge::default(),
            ),

            system: System::new(),
            cpu_scope: registry.sub_registry("process"),
            cpu_usage: None,
            cpu_monitoring: true,
            last_cpu_refresh: None,
        }
    }

    /// Enable or disable CPU sampling independently of memory metrics. Enabled by default.
    pub fn with_cpu_monitoring(mut self, enabled: bool) -> Self {
        self.cpu_monitoring = enabled;
        self.cpu_usage = None;
        self.last_cpu_refresh = None;
        self
    }

    fn record_cpu(&mut self, usage: Option<f32>, now: Instant) {
        let Some(basis_points) = usage
            .and_then(cpu_basis_points)
            .filter(|_| self.cpu_monitoring)
        else {
            self.cpu_usage = None;
            self.last_cpu_refresh = None;
            return;
        };
        let previous = self.last_cpu_refresh.replace(now);
        if previous.is_none() {
            return;
        }
        let gauge = self.cpu_usage.get_or_insert_with(|| {
            self.cpu_scope.register(
                "cpu_usage_basis_points",
                "Process CPU usage in basis points (10000 is one occupied core)",
                raw::Gauge::default(),
            )
        });
        let _ = gauge.try_set(basis_points);
    }

    fn cpu_refresh_due(&self, now: Instant) -> bool {
        self.cpu_monitoring
            && self
                .last_cpu_refresh
                .is_none_or(|last| now.duration_since(last) >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL)
    }

    /// Update all process metrics.
    fn update(&mut self) {
        // Refresh process information
        let now = Instant::now();
        let refresh_cpu = self.cpu_refresh_due(now);
        let mut refresh = ProcessRefreshKind::nothing().with_memory();
        if refresh_cpu {
            refresh = refresh.with_cpu();
        }
        let refreshed = self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            refresh,
        );

        if refreshed == 0 {
            self.record_cpu(None, now);
            return;
        }

        // If the process exists, update the metrics
        if let Some(process) = self.system.process(self.pid) {
            let _ = self.rss.try_set(process.memory());
            let _ = self.virtual_memory.try_set(process.virtual_memory());
            let usage = process.cpu_usage();
            if refresh_cpu {
                self.record_cpu(Some(usage), now);
            }
        } else {
            self.record_cpu(None, now);
        }
    }

    /// Update process metrics periodically.
    ///
    /// This function takes a sleep function as a parameter to allow different runtimes
    /// to provide their own implementation.
    pub async fn collect<F, Fut>(mut self, sleep_fn: F)
    where
        F: Fn(Duration) -> Fut,
        Fut: Future<Output = ()>,
    {
        loop {
            self.update();
            sleep_fn(TICK_INTERVAL).await;
        }
    }
}

fn cpu_basis_points(usage: f32) -> Option<GaugeValue> {
    let scaled = (f64::from(usage) * 100.0).round();
    if !usage.is_finite() || usage < 0.0 || scaled >= GaugeValue::MAX as f64 {
        return None;
    }
    Some(scaled as GaugeValue)
}

#[cfg(test)]
#[cfg(not(target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn cpu_registration_lifecycle() {
        let mut registry = crate::telemetry::metrics::Registry::default();
        let mut metrics = Metrics::init(&mut registry);
        let now = Instant::now();
        assert!(metrics.cpu_refresh_due(now));
        metrics.record_cpu(Some(0.0), now);
        assert!(!metrics.cpu_refresh_due(now));
        assert!(metrics.cpu_refresh_due(now + TICK_INTERVAL));
        assert!(!registry.encode().contains("process_cpu_usage_basis_points"));
        metrics.record_cpu(Some(0.125), now + TICK_INTERVAL);
        assert_eq!(metrics.cpu_usage.as_ref().unwrap().get(), 13);
        metrics.record_cpu(Some(450.25), now + TICK_INTERVAL * 2);
        assert_eq!(metrics.cpu_usage.as_ref().unwrap().get(), 45025);
        metrics.record_cpu(None, now + TICK_INTERVAL * 3);
        assert!(!registry.encode().contains("process_cpu_usage_basis_points"));
        metrics.record_cpu(Some(20.0), now + TICK_INTERVAL * 4);
        assert!(metrics.cpu_usage.is_none());
        metrics.record_cpu(Some(20.0), now + TICK_INTERVAL * 5);
        assert_eq!(metrics.cpu_usage.as_ref().unwrap().get(), 2000);
        metrics.record_cpu(Some(f32::NAN), now + TICK_INTERVAL * 6);
        assert!(!registry.encode().contains("process_cpu_usage_basis_points"));
        metrics = metrics.with_cpu_monitoring(false);
        assert!(!metrics.cpu_refresh_due(now + TICK_INTERVAL * 5));
        metrics.record_cpu(Some(10.0), now + TICK_INTERVAL * 5);
        assert!(metrics.cpu_usage.is_none());
        for invalid in [f32::NAN, f32::INFINITY, -1.0, f32::MAX] {
            assert!(cpu_basis_points(invalid).is_none());
        }
    }

    #[test]
    fn test_process_metrics_init() {
        let mut registry = crate::telemetry::metrics::Registry::default();
        let mut metrics = Metrics::init(&mut registry).with_cpu_monitoring(false);

        // Update metrics
        metrics.update();
        let rss = metrics.rss.get();
        assert!(rss > 1024 * 1024); // 1MB
        let virt = metrics.virtual_memory.get();
        assert!(
            virt >= rss,
            "Expected virtual memory ({virt}) to be >= RSS ({rss})"
        );

        // Update metrics
        metrics.update();
        let new_rss = metrics.rss.get();
        assert!(new_rss > 1024 * 1024); // 1MB
        let new_virt = metrics.virtual_memory.get();
        assert!(
            new_virt >= new_rss,
            "Expected virtual memory ({new_virt}) to be >= RSS ({new_rss})"
        );

        // Because tests may be run in parallel, we can't assert anything about the value of the metrics.
    }
}
