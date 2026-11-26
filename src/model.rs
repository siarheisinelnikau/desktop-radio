use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

pub const GROUPS: [&str; 5] = [
    "BELARUS",
    "RUSSIA",
    "KAZAKHSTAN",
    "INTERNATIONAL",
    "FAVORITES",
];
pub const FREQUENCIES: [u32; 10] = [31, 62, 125, 250, 500, 1_000, 2_000, 4_000, 8_000, 16_000];
pub const FAVORITES: &str = "FAVORITES";

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Station {
    pub id: String,
    pub name: String,
    pub label: String,
    pub aliases: String,
    pub streams: Vec<String>,
    pub group: String,
    pub country_code: Option<String>,
    pub label_ru: Option<String>,
    pub label_kk: Option<String>,
    pub track_source_id: Option<u32>,
    #[serde(skip)]
    search: String,
}

impl Station {
    pub fn detail(&self, language: &str) -> String {
        let description = match language {
            "ru" => self.label_ru.as_deref(),
            "kk" => self.label_kk.as_deref(),
            _ => None,
        }
        .unwrap_or(&self.label);
        match &self.country_code {
            Some(country) if !description.is_empty() => format!("{country}  ·  {description}"),
            Some(country) => country.clone(),
            None => description.to_owned(),
        }
    }
}

pub struct Catalog(pub Vec<Station>);

impl Catalog {
    pub fn load() -> Self {
        let mut stations: Vec<Station> =
            serde_json::from_str(include_str!("../Assets/stations.json"))
                .expect("valid built-in station catalog");
        for station in &mut stations {
            station.search = normalize(&format!(
                "{} {} {} {} {} {}",
                station.name,
                station.label,
                station.aliases,
                station.country_code.as_deref().unwrap_or_default(),
                station.label_ru.as_deref().unwrap_or_default(),
                station.label_kk.as_deref().unwrap_or_default()
            ));
        }
        Self(stations)
    }

    pub fn get(&self, id: &str) -> Option<usize> {
        self.0.iter().position(|station| station.id == id)
    }

    pub fn list(
        &self,
        prefs: &Preferences,
        group: &str,
        query: &str,
        include_hidden: bool,
    ) -> Vec<usize> {
        let normalized = normalize(query);
        let words = normalized.split_whitespace().collect::<Vec<_>>();
        let when = now();
        let mut result = Vec::new();
        for region in GROUPS {
            if if words.is_empty() {
                region != group
            } else {
                region == FAVORITES || prefs.hidden_groups.contains(region)
            } {
                continue;
            }
            let mut listed = self
                .0
                .iter()
                .enumerate()
                .filter(|(_, station)| {
                    let selected = if region == FAVORITES {
                        prefs.favorites.contains(&station.id)
                    } else {
                        station.group == region
                    };
                    selected
                        && (include_hidden || prefs.visible(&station.id, when))
                        && words.iter().all(|word| station.search.contains(word))
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if let Some(order) = prefs.order.get(region) {
                let ranks: HashMap<_, _> = order
                    .iter()
                    .enumerate()
                    .map(|(index, key)| (key, index))
                    .collect();
                listed.sort_by_key(|index| {
                    ranks.get(&self.0[*index].id).copied().unwrap_or(usize::MAX)
                });
            }
            result.extend(listed);
        }
        result
    }
}

fn normalize(text: &str) -> String {
    text.to_lowercase()
        .nfd()
        .filter(|character| !is_combining_mark(*character))
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect()
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    pub language: String,
    pub theme: String,
    pub visualization: String,
    pub smooth_visualization: bool,
    pub show_intro: bool,
    pub play_on_startup: bool,
    pub check_updates_on_startup: bool,
    pub hide_unavailable: bool,
    pub group: String,
    pub station_id: Option<String>,
    pub volume: u8,
    pub equalizer_enabled: bool,
    pub equalizer: [i8; 10],
    pub favorites: HashSet<String>,
    pub hidden_stations: HashSet<String>,
    pub hidden_groups: HashSet<String>,
    pub order: HashMap<String, Vec<String>>,
    pub unavailable: HashMap<String, UnavailableAt>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UnavailableAt {
    Epoch(u64),
    Date(String),
}

impl UnavailableAt {
    fn seconds(&self) -> u64 {
        match self {
            Self::Epoch(seconds) => *seconds,
            Self::Date(value) => {
                time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
                    .ok()
                    .and_then(|date| date.unix_timestamp().try_into().ok())
                    .unwrap_or(0)
            }
        }
    }
}

impl Preferences {
    pub fn visible(&self, id: &str, now: u64) -> bool {
        !self.hidden_stations.contains(id)
            && !(self.hide_unavailable
                && self
                    .unavailable
                    .get(id)
                    .is_some_and(|failed| now.saturating_sub(failed.seconds()) < 86_400))
    }

    pub fn load(path: &Path) -> Self {
        let mut prefs: Self = std::fs::read(path)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        prefs.volume = prefs.volume.min(100);
        prefs
            .equalizer
            .iter_mut()
            .for_each(|gain| *gain = (*gain).clamp(-12, 12));
        prefs
            .hidden_groups
            .retain(|region| GROUPS.contains(&region.as_str()));
        if prefs.hidden_groups.len() == GROUPS.len() {
            prefs.hidden_groups.remove(GROUPS[0]);
        }
        if !GROUPS.contains(&prefs.group.as_str()) || prefs.hidden_groups.contains(&prefs.group) {
            prefs.group = GROUPS
                .iter()
                .find(|region| !prefs.hidden_groups.contains(**region))
                .unwrap()
                .to_string();
        }
        prefs
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let staged = path.with_extension("tmp");
        std::fs::write(&staged, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(staged, path)?;
        Ok(())
    }
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            language: "en".into(),
            theme: "graphite".into(),
            visualization: "spectrum".into(),
            smooth_visualization: true,
            show_intro: true,
            play_on_startup: true,
            check_updates_on_startup: true,
            hide_unavailable: false,
            group: GROUPS[0].into(),
            station_id: None,
            volume: 65,
            equalizer_enabled: false,
            equalizer: [0; 10],
            favorites: HashSet::new(),
            hidden_stations: HashSet::new(),
            hidden_groups: HashSet::new(),
            order: HashMap::new(),
            unavailable: HashMap::new(),
        }
    }
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("DesktopRadio")
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn group_name(group: &str, language: &str) -> &'static str {
    label(
        match group {
            "BELARUS" => "Belarus",
            "RUSSIA" => "Russia",
            "KAZAKHSTAN" => "Kazakhstan",
            "INTERNATIONAL" => "World",
            _ => "Favorites",
        },
        language,
    )
}

pub fn label<'a>(english: &'a str, language: &str) -> &'a str {
    if !matches!(language, "ru" | "kk") {
        return english;
    }
    crate::strings::lookup(english, language).unwrap_or(english)
}
