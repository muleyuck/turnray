use std::sync::Arc;
use std::time::Duration;

use agent_core::{Agent, DataSource, SourceError};
use tao::event_loop::EventLoopProxy;
use tray_icon::menu::MenuEvent;

/// herdr's own tab bar polls at the same rate.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

pub enum UserEvent {
    Update(Result<Vec<Agent>, SourceError>),
    Menu(MenuEvent),
}

fn build_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

async fn backend_loop(source: Arc<dyn DataSource>, proxy: EventLoopProxy<UserEvent>) {
    let mut interval = tokio::time::interval(POLL_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        // fetch starts processes and this runtime is single-threaded, so running it
        // inline would stop the timer with it.
        let source = Arc::clone(&source);
        let res = match tokio::task::spawn_blocking(move || source.fetch()).await {
            Ok(res) => res,
            Err(e) => Err(SourceError::Failed(format!("fetch task failed: {e}"))),
        };
        if proxy.send_event(UserEvent::Update(res)).is_err() {
            return; // event loop already ended
        }
    }
}

pub fn spawn_backend(source: Arc<dyn DataSource>, proxy: EventLoopProxy<UserEvent>) {
    std::thread::spawn(move || build_runtime().block_on(backend_loop(source, proxy)));
}
