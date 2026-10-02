use std::collections::HashMap;

use agent_core::{Agent, Settings, SourceError, Status, Style};
use tray_icon::menu::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::TrayIcon;

use crate::ui::{self, DisplayState, MenuEntry, PriorityRow, TrayImage, TrayView};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    SetStyle(Style),
    MoveUp(Status),
    MoveDown(Status),
    Quit,
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

/// muda strips a lone `&` from item text as a mnemonic marker; `&&` renders as one `&`.
fn escape_mnemonic(text: &str) -> String {
    text.replace('&', "&&")
}

/// The agent lines are display-only: nothing in them is clickable.
pub fn build_menu(model: &[MenuEntry]) -> (Menu, HashMap<MenuId, MenuAction>) {
    let menu = Menu::new();
    let mut actions = HashMap::new();
    for entry in model {
        match entry {
            MenuEntry::Disabled(text) => {
                menu.append(&MenuItem::new(escape_mnemonic(text), false, None))
            }
            MenuEntry::Separator => menu.append(&PredefinedMenuItem::separator()),
            MenuEntry::Style(current) => menu.append(&style_submenu(*current, &mut actions)),
            MenuEntry::Priority(rows) => menu.append(&priority_submenu(rows, &mut actions)),
            MenuEntry::Quit => {
                let item = MenuItem::new("Quit", true, None);
                actions.insert(item.id().clone(), MenuAction::Quit);
                menu.append(&item)
            }
        }
        .expect("menu append");
    }
    (menu, actions)
}

fn error_line(e: &SourceError) -> String {
    match e {
        SourceError::NotFound(msg) => format!("{msg} — install herdr and start it"),
        SourceError::Failed(msg) => msg.clone(),
    }
}

pub struct App {
    pub tray: Option<TrayIcon>,
    pub state: DisplayState,
    pub settings: Settings,
    pub emitted: &'static [Status],
    /// What the tray was last given, so nothing is re-sent every second.
    pub last_view: Option<TrayView>,
    /// What `set_menu` was last given.
    pub last_model: Option<Vec<MenuEntry>>,
    pub actions: HashMap<MenuId, MenuAction>,
}

impl App {
    pub fn new(settings: Settings, emitted: &'static [Status]) -> Self {
        App {
            tray: None,
            state: DisplayState::BeforeFirstFetch,
            settings,
            emitted,
            last_view: None,
            last_model: None,
            actions: HashMap::new(),
        }
    }

    pub fn refresh(&mut self) {
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
        // `set_menu` closes the menu if it is open, so it runs only on a real change.
        let model = ui::menu_model(&self.state, &self.settings, self.emitted);
        if self.last_model.as_ref() != Some(&model) {
            let (menu, actions) = build_menu(&model);
            tray.set_menu(Some(Box::new(menu)));
            self.actions = actions;
            self.last_model = Some(model);
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
                line: error_line(&e),
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

    /// Returns true when the app should quit.
    pub fn on_action(&mut self, action: MenuAction) -> bool {
        match action {
            MenuAction::Quit => return true,
            MenuAction::SetStyle(style) => {
                self.settings.style = style;
                // macOS flips a check item's mark on click, so re-picking the current style
                // unchecks it while the model stays equal; force a rebuild to restore it.
                self.last_model = None;
            }
            MenuAction::MoveUp(status) => self.settings.priority.move_up(status),
            MenuAction::MoveDown(status) => self.settings.priority.move_down(status),
        }
        crate::store::save(&self.settings);
        self.refresh();
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_herdr_says_what_to_do() {
        assert_eq!(
            error_line(&SourceError::NotFound("herdr not found".into())),
            "herdr not found — install herdr and start it"
        );
        assert_eq!(error_line(&SourceError::Failed("boom".into())), "boom");
    }

    #[test]
    fn an_ampersand_in_a_line_survives_mnemonic_stripping() {
        assert_eq!(escape_mnemonic("R&D · a && b"), "R&&D · a &&&& b");
    }
}
