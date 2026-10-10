use agent_core::{Agent, Settings, SourceError, Status, Style};
use tray_icon::TrayIcon;

use crate::panel::Panel;
use crate::ui::{self, Dir, DisplayState, PanelContent, SettingsView, TrayImage, TrayView};

pub struct App {
    tray: Option<TrayIcon>,
    state: DisplayState,
    settings: Settings,
    emitted: &'static [Status],
    /// What the tray was last given, so nothing is re-sent every second.
    last_view: Option<TrayView>,
    panel: Option<Panel>,
    /// What the panel's list and settings view were last given.
    last_content: Option<PanelContent>,
    last_settings: Option<SettingsView>,
}

impl App {
    pub fn new(settings: Settings, emitted: &'static [Status]) -> Self {
        App {
            tray: None,
            state: DisplayState::BeforeFirstFetch,
            settings,
            emitted,
            last_view: None,
            panel: None,
            last_content: None,
            last_settings: None,
        }
    }

    /// The tray and the panel can only be built once the event loop runs, so they arrive
    /// after `new`.
    pub fn attach(&mut self, tray: TrayIcon, panel: Panel) {
        self.tray = Some(tray);
        self.panel = Some(panel);
        self.refresh();
    }

    fn refresh(&mut self) {
        let Some(tray) = self.tray.as_ref() else {
            return;
        };
        let view = ui::tray_view(&self.state, &self.settings);
        let last = self.last_view.as_ref();
        // tray-icon keeps icon and title while hidden and applies them when the item
        // comes back, so they are updated before visibility. A hidden view carries no image
        // worth drawing (the full style's would be zero pixels wide), so it is skipped.
        if view.visible && last.map(|l| &l.image) != Some(&view.image) {
            // set_icon alone drops the template flag that lets macOS tint the image.
            tray.set_icon_with_as_template(Some(crate::tray_icon(&view.image)), true)
                .expect("tray icon update");
        }
        if last.map(|l| &l.title) != Some(&view.title) {
            tray.set_title(Some(&view.title));
        }
        // Each view is rebuilt only on a change, and the panel refitted once after.
        let mut changed = false;
        let content = ui::panel_model(&self.state, &self.settings);
        if self.last_content.as_ref() != Some(&content) {
            if let Some(panel) = self.panel.as_mut() {
                panel.set_content(&content);
            }
            self.last_content = Some(content);
            changed = true;
        }
        let settings = ui::settings_model(&self.settings, self.emitted);
        if self.last_settings.as_ref() != Some(&settings) {
            if let Some(panel) = self.panel.as_mut() {
                panel.set_settings(&settings);
            }
            self.last_settings = Some(settings);
            changed = true;
        }
        if changed {
            if let Some(panel) = self.panel.as_ref() {
                panel.fit();
            }
        }
        if last.map(|l| l.visible) != Some(view.visible) {
            tray.set_visible(view.visible).expect("tray visibility");
        }
        self.last_view = Some(TrayView {
            // Keep the last image actually drawn, so a later visible view compares to it.
            // Before anything is drawn, an empty Full stands in: no visible view carries it,
            // so the first visible view always sets its icon.
            image: if view.visible {
                view.image.clone()
            } else {
                last.map_or(TrayImage::Full(Vec::new()), |l| l.image.clone())
            },
            ..view
        });
    }

    pub fn on_update(&mut self, res: Result<Vec<Agent>, SourceError>) {
        let state = match res {
            Ok(agents) => DisplayState::Ok { agents },
            Err(e) => DisplayState::Error {
                line: ui::error_line(&e),
            },
        };
        // Polled every second, so only a change of failure is worth a log line.
        if let DisplayState::Error { line } = &state {
            if self.state != state {
                tracing::warn!("fetch failed: {line}");
            }
        }
        self.state = state;
        self.refresh();
    }

    fn save_and_refresh(&mut self) {
        crate::store::save(&self.settings);
        self.refresh();
    }

    pub fn set_style(&mut self, style: Style) {
        // Picking the selected segment again sends it too; nothing to save.
        if self.settings.style == style {
            return;
        }
        self.settings.style = style;
        self.save_and_refresh();
    }

    /// `keyboard`: the button had the keyboard focus, which then follows the status so
    /// pressing on keeps moving it.
    pub fn move_status(&mut self, status: Status, dir: Dir, keyboard: bool) {
        match dir {
            Dir::Up => self.settings.priority.move_up(status),
            Dir::Down => self.settings.priority.move_down(status),
        }
        self.save_and_refresh();
        if !keyboard {
            return;
        }
        let rows = ui::priority_rows(&self.settings, self.emitted);
        let (row, dir) = ui::focus_after_move(&rows, status, dir);
        if let Some(panel) = self.panel.as_ref() {
            panel.focus_move_button(row, dir);
        }
    }

    /// Opens or closes the panel on the press; the release only keeps the icon pressed.
    pub fn on_tray_click(&mut self, pressed: bool) {
        let (Some(tray), Some(panel)) = (self.tray.as_ref(), self.panel.as_mut()) else {
            return;
        };
        if !pressed {
            if panel.is_shown() {
                panel.keep_highlight();
            }
        } else if panel.is_shown() {
            panel.close();
        } else if let Some(item) = tray.ns_status_item() {
            panel.show(&item);
        }
    }

    /// Switches the panel between the list and the settings view.
    pub fn on_settings(&mut self) {
        if let Some(panel) = self.panel.as_mut() {
            panel.toggle_settings();
        }
    }

    pub fn follow_anchor(&self) {
        if let Some(panel) = self.panel.as_ref() {
            panel.follow_anchor();
        }
    }

    pub fn close_panel(&mut self) {
        if let Some(panel) = self.panel.as_mut() {
            panel.close();
        }
    }
}
