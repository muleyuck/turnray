//! User settings, stored as `key=value` lines.

use crate::priority::Priority;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Style {
    /// The most urgent status's icon and its count
    #[default]
    Simple,
    /// Icon and count for every status that has an agent
    Full,
}

impl Style {
    pub fn as_str(self) -> &'static str {
        match self {
            Style::Simple => "simple",
            Style::Full => "full",
        }
    }

    fn parse(s: &str) -> Option<Style> {
        [Style::Simple, Style::Full]
            .into_iter()
            .find(|st| st.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Settings {
    pub style: Style,
    pub priority: Priority,
}

impl Settings {
    /// Lenient: a line it can't read leaves that setting at its default rather than
    /// discarding the rest.
    pub fn parse(text: &str) -> Settings {
        let mut settings = Settings::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "style" => {
                    if let Some(style) = Style::parse(value) {
                        settings.style = style;
                    }
                }
                "priority" => {
                    if let Some(priority) = Priority::parse(value) {
                        settings.priority = priority;
                    }
                }
                _ => {}
            }
        }
        settings
    }

    pub fn serialize(&self) -> String {
        format!(
            "style={}\npriority={}\n",
            self.style.as_str(),
            self.priority.serialize()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip() {
        let mut s = Settings {
            style: Style::Full,
            ..Settings::default()
        };
        s.priority.move_up(crate::Status::Working);
        assert_eq!(Settings::parse(&s.serialize()), s);
    }

    #[test]
    fn an_empty_file_gives_the_defaults() {
        let s = Settings::parse("");
        assert_eq!(s.style, Style::Simple);
        assert_eq!(s.priority, Priority::default());
    }

    #[test]
    fn a_bad_line_only_resets_its_own_setting() {
        let s = Settings::parse("style=full\npriority=blocked,done\ngarbage\nfoo=bar\n");
        assert_eq!(s.style, Style::Full);
        assert_eq!(s.priority, Priority::default());
    }
}
