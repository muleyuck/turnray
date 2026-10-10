mod backend;
mod herdr;
mod images;
mod menu;
mod panel;
mod process;
mod store;
mod ui;

use std::sync::Arc;

use agent_core::DataSource;
use backend::{spawn_backend, UserEvent};
use menu::App;
use objc2::MainThreadMarker;
use panel::Panel;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use ui::TrayImage;

/// Builds the menu bar image for `image`, ready to hand to tray-icon.
pub fn tray_icon(image: &TrayImage) -> Icon {
    let img = match image {
        TrayImage::Status(status) => images::status_icon(*status),
        TrayImage::Full(counts) => images::full(counts),
        TrayImage::Standby => images::standby_icon(),
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

    let tray_proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        // Hover events and the middle button do nothing.
        if let TrayIconEvent::Click {
            button: MouseButton::Left | MouseButton::Right,
            button_state,
            ..
        } = e
        {
            let pressed = button_state == MouseButtonState::Down;
            let _ = tray_proxy.send_event(UserEvent::TrayClick { pressed });
        }
    }));
    let panel_proxy = event_loop.create_proxy();

    let source: Arc<dyn DataSource> = Arc::new(herdr::HerdrSource::default());
    let mut app = App::new(store::load(), source.emitted_statuses());
    spawn_backend(source, event_loop.create_proxy());

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                // On macOS, the tray must be created after the event loop starts. It starts
                // hidden: nothing is shown until the first fetch says what to show. Clicks
                // open the panel, so neither button opens a menu.
                let tray = TrayIconBuilder::new()
                    .with_menu_on_left_click(false)
                    .with_menu_on_right_click(false)
                    .build()
                    .expect("tray build");
                tray.set_visible(false).expect("tray visibility");
                let mtm = MainThreadMarker::new().expect("event loop runs on the main thread");
                app.attach(tray, Panel::new(panel_proxy.clone(), mtm));
            }
            Event::UserEvent(UserEvent::Update(res)) => app.on_update(res),
            Event::UserEvent(UserEvent::TrayClick { pressed }) => app.on_tray_click(pressed),
            Event::UserEvent(UserEvent::Settings) => app.on_settings(),
            Event::UserEvent(UserEvent::SetStyle(style)) => app.set_style(style),
            Event::UserEvent(UserEvent::Move {
                status,
                dir,
                keyboard,
            }) => app.move_status(status, dir, keyboard),
            Event::UserEvent(UserEvent::ClosePanel) => app.close_panel(),
            Event::UserEvent(UserEvent::AnchorMoved) => app.follow_anchor(),
            Event::UserEvent(UserEvent::Quit) => {
                app.close_panel();
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}
