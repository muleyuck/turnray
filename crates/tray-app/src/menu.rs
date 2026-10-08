use std::collections::HashMap;

use agent_core::{Agent, Settings, SourceError, Status, Style};
use tray_icon::menu::{CheckMenuItem, Menu, MenuId, MenuItem, Submenu};
use tray_icon::TrayIcon;

use crate::panel::Panel;
use crate::ui::{self, DisplayState, PanelContent, PriorityRow, SettingsMenu, TrayImage, TrayView};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    SetStyle(Style),
    MoveUp(Status),
    MoveDown(Status),
}

fn style_submenu(current: Style, actions: &mut HashMap<MenuId, MenuAction>) -> Submenu {
    let submenu = Submenu::new("Style", true);
    for (style, label) in [(Style::Simple, "Simple"), (Style::Full, "Full")] {
        let item = CheckMenuItem::new(label, true, style == current, None);
        actions.insert(item.id().clone(), MenuAction::SetStyle(style));
        submenu.append(&item).expect("menu append");
    }
    submenu
}

fn priority_submenu(rows: &[PriorityRow], actions: &mut HashMap<MenuId, MenuAction>) -> Submenu {
    let submenu = Submenu::new("Priority", true);
    for row in rows {
        let entry = Submenu::new(&row.label, true);
        for (label, enabled, action) in [
            ("Move Up", row.can_move_up, MenuAction::MoveUp(row.status)),
            (
                "Move Down",
                row.can_move_down,
                MenuAction::MoveDown(row.status),
            ),
        ] {
            let item = MenuItem::new(label, enabled, None);
            actions.insert(item.id().clone(), action);
            entry.append(&item).expect("menu append");
        }
        submenu.append(&entry).expect("menu append");
    }
    submenu
}

/// The ⚙ button's menu.
pub fn build_settings_menu(model: &SettingsMenu) -> (Menu, HashMap<MenuId, MenuAction>) {
    let menu = Menu::new();
    let mut actions = HashMap::new();
    menu.append(&style_submenu(model.style, &mut actions))
        .expect("menu append");
    menu.append(&priority_submenu(&model.priority, &mut actions))
        .expect("menu append");
    (menu, actions)
}

pub struct App {
    tray: Option<TrayIcon>,
    state: DisplayState,
    settings: Settings,
    emitted: &'static [Status],
    /// What the tray was last given, so nothing is re-sent every second.
    last_view: Option<TrayView>,
    panel: Option<Panel>,
    /// What the panel was last given.
    last_content: Option<PanelContent>,
    /// The ⚙ menu last shown, kept until the next replaces it: muda's items point into it.
    settings_menu: Option<Menu>,
    actions: HashMap<MenuId, MenuAction>,
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
            settings_menu: None,
            actions: HashMap::new(),
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
        // tray-icon keeps icon, title and menu while hidden and applies them when the item
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
        let content = ui::panel_model(&self.state, &self.settings);
        if self.last_content.as_ref() != Some(&content) {
            if let Some(panel) = self.panel.as_mut() {
                panel.set_content(&content);
            }
            self.last_content = Some(content);
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

    /// An id from an older menu does nothing.
    pub fn on_menu_event(&mut self, id: &MenuId) {
        let Some(action) = self.actions.get(id).copied() else {
            return;
        };
        match action {
            MenuAction::SetStyle(style) => self.settings.style = style,
            MenuAction::MoveUp(status) => self.settings.priority.move_up(status),
            MenuAction::MoveDown(status) => self.settings.priority.move_down(status),
        }
        crate::store::save(&self.settings);
        self.refresh();
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

    /// Built afresh each time, so a check item macOS flipped on its last click is redrawn
    /// from the settings.
    pub fn on_settings(&mut self) {
        let Some(panel) = self.panel.as_ref() else {
            return;
        };
        if !panel.is_shown() {
            return;
        }
        let (menu, actions) =
            build_settings_menu(&ui::settings_model(&self.settings, self.emitted));
        self.actions = actions;
        panel.show_settings_menu(&menu);
        self.settings_menu = Some(menu);
    }

    pub fn close_panel(&mut self) {
        if let Some(panel) = self.panel.as_mut() {
            panel.close();
        }
    }
}
