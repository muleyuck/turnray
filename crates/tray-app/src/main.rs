mod backend;
mod herdr;
mod images;
mod menu;
mod store;
mod ui;

use std::sync::Arc;

use agent_core::DataSource;
use backend::{spawn_backend, UserEvent};
use menu::App;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::MenuEvent;
use tray_icon::{Icon, TrayIconBuilder};
use ui::TrayImage;

/// Builds the menu bar image for `image`, ready to hand to tray-icon.
pub fn tray_icon(image: &TrayImage) -> Icon {
    let img = match image {
        TrayImage::Status(status) => images::status_icon(*status),
        TrayImage::Full(counts) => images::full(counts),
        TrayImage::Error => images::error_icon(),
    };
    Icon::from_rgba(img.rgba, img.width, img.height).expect("tray icon")
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }

    let menu_proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let _ = menu_proxy.send_event(UserEvent::Menu(e));
    }));

    let source: Arc<dyn DataSource> = Arc::new(herdr::HerdrSource::default());
    let mut app = App::new(store::load(), source.emitted_statuses());
    spawn_backend(source, event_loop.create_proxy());

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                // On macOS, the tray must be created after the event loop starts. It starts
                // hidden: nothing is shown until there is an agent to show.
                let tray = TrayIconBuilder::new().build().expect("tray build");
                tray.set_visible(false).expect("tray visibility");
                app.tray = Some(tray);
                app.refresh();
            }
            Event::UserEvent(UserEvent::Update(res)) => app.on_update(res),
            Event::UserEvent(UserEvent::Menu(e)) => {
                if let Some(action) = app.actions.get(&e.id).copied() {
                    if app.on_action(action) {
                        *control_flow = ControlFlow::Exit;
                    }
                }
            }
            _ => {}
        }
    });
}
