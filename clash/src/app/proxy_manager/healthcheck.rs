use std::sync::Arc;

use tokio::{
    sync::{Mutex, RwLock},
    time::Instant,
};

use super::{ProxyManager, ThreadSafeProxy};

pub type ThreadSafeHealthCheck = Arc<RwLock<HealthCheck>>;

struct HealCheckInner {
    last_check: Instant,
}

pub struct HealthCheck {
    proxies: Vec<ThreadSafeProxy>,
    url: String,
    interval: u64,
    lazy: bool,
    latency_manager: Arc<Mutex<ProxyManager>>,
    task_handle: Option<tokio::task::JoinHandle<()>>,
    inner: Arc<tokio::sync::Mutex<HealCheckInner>>,
}

impl HealthCheck {
    pub fn new(
        proxies: Vec<ThreadSafeProxy>,
        url: String,
        interval: u64,
        lazy: bool,
        latency_manager: Arc<Mutex<ProxyManager>>,
    ) -> anyhow::Result<Self> {
        let mut health_check = Self {
            proxies,
            url,
            interval,
            lazy,
            latency_manager,
            task_handle: None,
            inner: Arc::new(tokio::sync::Mutex::new(HealCheckInner {
                last_check: tokio::time::Instant::now(),
            })),
        };
        Ok(health_check)
    }

    fn kick_off(&mut self) {
        let latency_manager = self.latency_manager.clone();
        let interval = self.interval;
        let lazy = self.lazy;
        let proxies = self.proxies.clone();

        tokio::spawn(async move {
            latency_manager.blocking_lock().check(&proxies).await;
        });

        let inner = self.inner.clone();
        let proxies = self.proxies.clone();
        let latency_manager = self.latency_manager.clone();
        let task_handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(tokio::time::Duration::from_secs(interval));
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let now = tokio::time::Instant::now();
                        if !lazy || now.duration_since(inner.blocking_lock().last_check).as_secs() >= interval {
                            latency_manager.blocking_lock().check(&proxies).await;
                            inner.blocking_lock().last_check = now;
                        }
                    },
                }
            }
        });

        self.task_handle = Some(task_handle);
    }

    fn touch(&mut self) {
        self.inner.blocking_lock().last_check = tokio::time::Instant::now();
    }

    fn stop(&mut self) {
        if let Some(task_handle) = self.task_handle.take() {
            task_handle.abort();
        }
    }

    fn update(&mut self, proxies: Vec<ThreadSafeProxy>) {
        self.proxies = proxies;
    }

    fn auto(&self) -> bool {
        self.interval != 0
    }
}
