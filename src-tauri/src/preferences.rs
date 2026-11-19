use crate::caffeinate::CaffeinateSettings;
use chrono::NaiveTime;
use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Language {
    #[default]
    System,
    En,
    Es,
}

#[cfg(target_os = "macos")]
pub fn system_language() -> Language {
    use objc2_foundation::NSLocale;

    preferred_language(
        NSLocale::preferredLanguages()
            .iter()
            .map(|tag| tag.to_string()),
    )
}

#[cfg(any(target_os = "macos", test))]
fn preferred_language(tags: impl IntoIterator<Item = impl AsRef<str>>) -> Language {
    tags.into_iter()
        .find_map(|tag| match tag.as_ref().split(['-', '_']).next()? {
            code if code.eq_ignore_ascii_case("es") => Some(Language::Es),
            code if code.eq_ignore_ascii_case("en") => Some(Language::En),
            _ => None,
        })
        .unwrap_or(Language::En)
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ManualDuration {
    #[default]
    Minutes15,
    Minutes30,
    Hour,
    UntilStopped,
}

impl ManualDuration {
    pub fn seconds(self) -> Option<u64> {
        match self {
            Self::Minutes15 => Some(900),
            Self::Minutes30 => Some(1800),
            Self::Hour => Some(3600),
            Self::UntilStopped => None,
        }
    }
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct Schedule {
    pub enabled: bool,
    pub start: String,
    pub end: String,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            enabled: false,
            start: "09:00".into(),
            end: "17:00".into(),
        }
    }
}

impl Schedule {
    fn bounds(&self) -> Option<(NaiveTime, NaiveTime)> {
        let start = NaiveTime::parse_from_str(&self.start, "%H:%M").ok()?;
        let end = NaiveTime::parse_from_str(&self.end, "%H:%M").ok()?;
        if start == end {
            return None;
        }
        Some((start, end))
    }

    pub fn contains(&self, now: NaiveTime) -> bool {
        if !self.enabled {
            return false;
        }
        let Some((start, end)) = self.bounds() else {
            return false;
        };
        if start < end {
            now >= start && now < end
        } else {
            now >= start || now < end
        }
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct QuickStart {
    pub activity: bool,
    pub caffeinate: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    pub language: Language,
    pub theme: Theme,
    pub duration: ManualDuration,
    pub schedule: Schedule,
    pub quick_start: QuickStart,
    pub caffeinate: CaffeinateSettings,
    pub hide_while_sharing: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            language: Language::default(),
            theme: Theme::default(),
            duration: ManualDuration::default(),
            schedule: Schedule::default(),
            quick_start: QuickStart::default(),
            caffeinate: CaffeinateSettings::default(),
            hide_while_sharing: true,
        }
    }
}

pub fn load(path: &Path) -> Result<Preferences, Box<dyn std::error::Error>> {
    match fs::read(path) {
        Ok(data) => Ok(serde_json::from_slice(&data)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Preferences::default()),
        Err(error) => Err(error.into()),
    }
}

pub fn save(path: &Path, preferences: &Preferences) -> Result<(), &'static str> {
    if preferences.schedule.bounds().is_none() {
        return Err("invalidSchedule");
    }

    let data = serde_json::to_vec_pretty(preferences).map_err(|_| "saveFailed")?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, data).map_err(|_| "saveFailed")?;
    fs::rename(temporary, path).map_err(|_| "saveFailed")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time(hour: u32, minute: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(hour, minute, 0).unwrap()
    }

    #[test]
    fn appearance_and_language_default_to_system_independently() {
        let defaults: Preferences = serde_json::from_str("{}").unwrap();
        assert!(matches!(defaults.theme, Theme::System));
        assert!(matches!(defaults.language, Language::System));

        let explicit_language: Preferences = serde_json::from_str(r#"{"language":"es"}"#).unwrap();
        assert!(matches!(explicit_language.theme, Theme::System));
        assert!(matches!(explicit_language.language, Language::Es));
    }

    #[test]
    fn sharing_privacy_defaults_on_and_preserves_an_opt_out() {
        assert!(Preferences::default().hide_while_sharing);
        let previous: Preferences = serde_json::from_str(r#"{"language":"es"}"#).unwrap();
        assert!(previous.hide_while_sharing);

        let disabled: Preferences = serde_json::from_str(r#"{"hideWhileSharing":false}"#).unwrap();
        let saved = serde_json::to_string(&disabled).unwrap();
        assert!(
            !serde_json::from_str::<Preferences>(&saved)
                .unwrap()
                .hide_while_sharing
        );
    }

    #[test]
    fn system_language_respects_preference_order_and_falls_back_to_english() {
        assert!(matches!(
            preferred_language(["fr-CA", "es-CL", "en-US"]),
            Language::Es
        ));
        assert!(matches!(
            preferred_language(["en-GB", "es-ES"]),
            Language::En
        ));
        assert!(matches!(preferred_language(["ja-JP"]), Language::En));
    }

    #[test]
    fn existing_preferences_load_with_quick_start_disabled() {
        let preferences: Preferences =
            serde_json::from_str(r#"{"theme":"dark","duration":"minutes30"}"#).unwrap();
        assert!(matches!(preferences.theme, Theme::Dark));
        assert!(matches!(preferences.language, Language::System));
        assert!(matches!(preferences.duration, ManualDuration::Minutes30));
        assert!(!preferences.quick_start.activity);
        assert!(!preferences.quick_start.caffeinate);
        assert_eq!(preferences.caffeinate.seconds, 1800);
    }

    #[test]
    fn daytime_schedule_includes_start_and_excludes_end() {
        let schedule = Schedule {
            enabled: true,
            ..Schedule::default()
        };
        assert!(!schedule.contains(time(8, 59)));
        assert!(schedule.contains(time(9, 0)));
        assert!(!schedule.contains(time(17, 0)));
    }

    #[test]
    fn overnight_schedule_stays_active_after_midnight() {
        let schedule = Schedule {
            enabled: true,
            start: "22:00".into(),
            end: "06:00".into(),
        };
        assert!(schedule.contains(time(22, 0)));
        assert!(schedule.contains(time(5, 59)));
        assert!(!schedule.contains(time(6, 0)));
        assert!(!schedule.contains(time(12, 0)));
    }
}
