//! Commands accepted by the daemon (tray menu, CLI forwarding) — pure logic.

use jepret_core::{Action, CaptureKind, Config};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Capture(CaptureKind),
    Reload,
}

pub const KIND_NAMES: &[(&str, CaptureKind)] = &[
    ("fullscreen", CaptureKind::Fullscreen),
    ("active_monitor", CaptureKind::ActiveMonitor),
    ("active_window", CaptureKind::ActiveWindow),
    ("region", CaptureKind::Region),
    ("last_region", CaptureKind::LastRegion),
    ("scroll", CaptureKind::Scroll),
    ("record_region", CaptureKind::RecordRegion),
    ("record_monitor", CaptureKind::RecordMonitor),
];

/// Parse a capture kind from the CLI; accepts `snake_case` or `kebab-case`.
pub fn parse_kind(s: &str) -> anyhow::Result<CaptureKind> {
    let norm = s.trim().to_ascii_lowercase().replace('-', "_");
    KIND_NAMES
        .iter()
        .find(|(n, _)| *n == norm)
        .map(|(_, k)| *k)
        .ok_or_else(|| {
            let all: Vec<&str> = KIND_NAMES.iter().map(|(n, _)| *n).collect();
            anyhow::anyhow!("unknown capture kind {s:?}; expected one of: {}", all.join(", "))
        })
}

pub fn kind_name(kind: CaptureKind) -> &'static str {
    KIND_NAMES.iter().find(|(_, k)| *k == kind).map(|(n, _)| *n).unwrap_or("unknown")
}

impl Command {
    /// Wire format for the single-instance channel.
    pub fn encode(self) -> String {
        match self {
            Command::Capture(k) => format!("capture {}", kind_name(k)),
            Command::Reload => "reload".to_string(),
        }
    }

    pub fn decode(s: &str) -> anyhow::Result<Self> {
        let mut it = s.split_whitespace();
        match (it.next(), it.next()) {
            (Some("capture"), Some(k)) => Ok(Command::Capture(parse_kind(k)?)),
            (Some("reload"), None) => Ok(Command::Reload),
            _ => anyhow::bail!("unrecognised command {s:?}"),
        }
    }
}

/// Actions to run for a capture started without a hotkey (tray menu / CLI):
/// the first hotkey binding with the same kind, else sensible defaults.
pub fn actions_for(config: &Config, kind: CaptureKind) -> Vec<Action> {
    if kind.is_recording() {
        return config.recording.actions.clone();
    }
    config
        .hotkeys
        .iter()
        .find(|h| h.capture == kind && !h.actions.is_empty())
        .map(|h| h.actions.clone())
        .unwrap_or_else(|| vec![Action::Save, Action::Clipboard, Action::Notify])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kinds() {
        assert_eq!(parse_kind("region").unwrap(), CaptureKind::Region);
        assert_eq!(parse_kind("Last-Region").unwrap(), CaptureKind::LastRegion);
        assert_eq!(parse_kind("record_monitor").unwrap(), CaptureKind::RecordMonitor);
        assert!(parse_kind("bogus").is_err());
    }

    #[test]
    fn every_kind_round_trips() {
        for (_, k) in KIND_NAMES {
            assert_eq!(Command::decode(&Command::Capture(*k).encode()).unwrap(), Command::Capture(*k));
        }
        assert_eq!(Command::decode("reload").unwrap(), Command::Reload);
        assert!(Command::decode("capture").is_err());
        assert!(Command::decode("rm -rf").is_err());
    }

    #[test]
    fn actions_lookup() {
        let c = Config::default();
        assert!(actions_for(&c, CaptureKind::Region).contains(&Action::OpenEditor));
        assert_eq!(actions_for(&c, CaptureKind::RecordRegion), c.recording.actions);
        assert_eq!(actions_for(&c, CaptureKind::ActiveMonitor).len(), 3);
    }
}
