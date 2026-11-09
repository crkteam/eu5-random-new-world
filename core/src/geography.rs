//! 地點的地理歸屬：`definitions.txt` 的五層階層（大陸 → 次大陸 → 區域 → 地區 → 省 → 地點）
//! 與 `default.map` 的類別清單（海域、湖泊、無法通行的山、無主地）。
//!
//! 新世界 = 大陸 `america` 底下的所有地點（與原專案相同）。

use std::collections::HashMap;
use std::path::Path;

use crate::error::{Error, Result, read_text};
use crate::locations::NamedColor;
use crate::pdx::{self, Item, Value};

pub const DEFINITIONS: &str = "definitions.txt";
pub const DEFAULT_MAP: &str = "default.map";

/// 新世界所在的大陸。
pub const NEW_WORLD: &str = "america";

/// 地點之上的五層。
pub const LEVELS: [&str; 5] = ["continent", "subcontinent", "region", "area", "province"];

/// 一個地點與它的五層上級（順序同 `LEVELS`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub key: String,
    pub parents: [String; 5],
}

/// 解析 `definitions.txt`：五層巢狀區塊，最內層是地點名稱清單。
pub fn parse_definitions(text: &str, path: &Path) -> Result<Vec<Placement>> {
    let items = pdx::parse(text).map_err(|e| Error::format(path, e.to_string()))?;
    let mut out = Vec::new();
    let mut stack = Vec::new();
    walk(&items, &mut stack, &mut out, path)?;
    let mut seen: HashMap<&str, &[String; 5]> = HashMap::new();
    for placement in &out {
        if let Some(first) = seen.insert(&placement.key, &placement.parents) {
            return Err(Error::format(
                path,
                format!(
                    "{} is in both {} and {}",
                    placement.key, first[4], placement.parents[4]
                ),
            ));
        }
    }
    Ok(out)
}

fn walk<'a>(
    items: &'a [Item],
    stack: &mut Vec<&'a str>,
    out: &mut Vec<Placement>,
    path: &Path,
) -> Result<()> {
    for item in items {
        match item {
            Item::Pair {
                key,
                value: Value::Block(inner),
                ..
            } if stack.len() < LEVELS.len() => {
                stack.push(key);
                walk(inner, stack, out, path)?;
                stack.pop();
            }
            Item::Bare(Value::Scalar(key)) if stack.len() == LEVELS.len() => {
                let parents = std::array::from_fn(|level| stack[level].to_owned());
                out.push(Placement {
                    key: key.clone(),
                    parents,
                });
            }
            // 區塊屬性，例如 `not_eligible_for_dynamic_country_name = yes`；與地理無關。
            Item::Pair { value: Value::Scalar(_), .. } if !stack.is_empty() => {}
            _ => {
                let level = LEVELS.get(stack.len()).copied().unwrap_or("location");
                let inside = stack.last().copied().unwrap_or("the top level");
                return Err(Error::format(
                    path,
                    format!("inside {inside}: expected a {level}, found {item:?}"),
                ));
            }
        }
    }
    Ok(())
}

/// 地點類別；不在任何清單裡的就是一般陸地。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Land,
    Sea,
    Lake,
    Impassable,
    NonOwnable,
}

/// `default.map` 的清單名稱與對應類別。
pub const KIND_LISTS: [(&str, Kind); 4] = [
    ("sea_zones", Kind::Sea),
    ("lakes", Kind::Lake),
    ("impassable_mountains", Kind::Impassable),
    ("non_ownable", Kind::NonOwnable),
];

/// 解析 `default.map` 的類別清單（同名清單可出現多次）；一個地點出現在兩種清單就失敗。
pub fn parse_kinds(text: &str, path: &Path) -> Result<HashMap<String, Kind>> {
    let items = pdx::parse(text).map_err(|e| Error::format(path, e.to_string()))?;
    let mut kinds = HashMap::new();
    for item in &items {
        let Item::Pair { key, value, .. } = item else {
            continue;
        };
        let Some(&(_, kind)) = KIND_LISTS.iter().find(|(name, _)| name == key) else {
            continue;
        };
        let Value::Block(entries) = value else {
            return Err(Error::format(path, format!("{key} is not a list")));
        };
        for entry in entries {
            let Item::Bare(Value::Scalar(location)) = entry else {
                return Err(Error::format(
                    path,
                    format!("{key}: expected a location, found {entry:?}"),
                ));
            };
            let merged = match kinds.get(location).copied() {
                None => kind,
                Some(other) if other == kind => kind,
                // 原版有 47 片大洋同時列在 sea_zones 與 impassable_mountains：船進不去的海，仍是海。
                Some(Kind::Sea | Kind::Impassable) if matches!(kind, Kind::Sea | Kind::Impassable) => {
                    Kind::Sea
                }
                Some(other) => {
                    return Err(Error::format(
                        path,
                        format!("{location} is both {other:?} and {kind:?}"),
                    ));
                }
            };
            kinds.insert(location.clone(), merged);
        }
    }
    Ok(kinds)
}

/// 每個地點（索引同顏色表）的大陸與類別。
pub struct Geography {
    /// 大陸名稱，依 `definitions.txt` 出現順序。
    pub continents: Vec<String>,
    /// 每個地點屬於 `continents` 的第幾個；`None` 是顏色表有、`definitions.txt` 沒有的地點。
    pub continent: Vec<Option<usize>>,
    pub kind: Vec<Kind>,
}

impl Geography {
    /// 對照顏色表：階層與類別清單裡的地點都必須在顏色表裡。
    pub fn new(
        locations: &[NamedColor],
        placements: &[Placement],
        kinds: &HashMap<String, Kind>,
    ) -> std::result::Result<Self, String> {
        let index: HashMap<&str, usize> = locations
            .iter()
            .enumerate()
            .map(|(i, l)| (l.key.as_str(), i))
            .collect();
        let mut continents: Vec<String> = Vec::new();
        let mut continent = vec![None; locations.len()];
        for placement in placements {
            let Some(&at) = index.get(placement.key.as_str()) else {
                return Err(format!(
                    "{DEFINITIONS}: {} is not in the colour table",
                    placement.key
                ));
            };
            let name = &placement.parents[0];
            continent[at] = Some(match continents.iter().position(|c| c == name) {
                Some(position) => position,
                None => {
                    continents.push(name.clone());
                    continents.len() - 1
                }
            });
        }
        if let Some(unknown) = kinds.keys().find(|key| !index.contains_key(key.as_str())) {
            return Err(format!(
                "{DEFAULT_MAP}: {unknown} is not in the colour table"
            ));
        }
        let kind = locations
            .iter()
            .map(|l| kinds.get(&l.key).copied().unwrap_or(Kind::Land))
            .collect();
        Ok(Self {
            continents,
            continent,
            kind,
        })
    }

    /// 每個地點是否屬於新世界；沒有 `america` 大陸就失敗。
    pub fn new_world(&self) -> std::result::Result<Vec<bool>, String> {
        self.on_continents(&[NEW_WORLD])
    }

    /// 每個地點是否屬於這些大陸之一；任何一個大陸不存在就失敗。
    pub fn on_continents(&self, names: &[&str]) -> std::result::Result<Vec<bool>, String> {
        let mut wanted = vec![false; self.continents.len()];
        for name in names {
            let at = self
                .continents
                .iter()
                .position(|c| c == name)
                .ok_or_else(|| format!("{DEFINITIONS} has no continent {name}"))?;
            wanted[at] = true;
        }
        Ok(self.continent.iter().map(|c| c.is_some_and(|c| wanted[c])).collect())
    }

    /// 不在 `definitions.txt` 的地點必須沒有像素（原版有 9 個這種沒用到的顏色）；
    /// `pixels` 是 `LocationMap::pixel_counts()`。回傳這些地點的索引。
    pub fn unplaced(&self, locations: &[NamedColor], pixels: &[u64]) -> std::result::Result<Vec<usize>, String> {
        let unplaced: Vec<usize> = (0..self.continent.len()).filter(|&i| self.continent[i].is_none()).collect();
        match unplaced.iter().find(|&&i| pixels[i] > 0) {
            Some(&i) => Err(format!(
                "{} has {} pixels but is not in {DEFINITIONS}",
                locations[i].key, pixels[i]
            )),
            None => Ok(unplaced),
        }
    }
}

/// 讀取 `map_data` 資料夾的階層與類別，對照顏色表。
pub fn read_geography(map_data: &Path, locations: &[NamedColor]) -> Result<Geography> {
    let definitions = map_data.join(DEFINITIONS);
    let placements = parse_definitions(&read_text(&definitions)?, &definitions)?;
    let default_map = map_data.join(DEFAULT_MAP);
    let kinds = parse_kinds(&read_text(&default_map)?, &default_map)?;
    Geography::new(locations, &placements, &kinds)
        .map_err(|message| Error::format(map_data, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFS: &str =
        "europe = { west = { scandinavia = { svealand = { uppland = { stockholm uppsala } } } } }
america = { north = { canada = { quebec_area = { quebec_province = { quebec lake_one } } } } }
ocean = { north_sea = { sea_region = { sea_area = { sea_province = { atlantic } } } } }";

    fn colors(keys: &[&str]) -> Vec<NamedColor> {
        keys.iter()
            .zip(1..)
            .map(|(key, color)| NamedColor {
                key: (*key).into(),
                color,
            })
            .collect()
    }

    #[test]
    fn reads_the_hierarchy() {
        let placements = parse_definitions(DEFS, Path::new("d")).unwrap();
        assert_eq!(placements.len(), 5);
        assert_eq!(placements[3].key, "lake_one");
        assert_eq!(
            placements[3].parents,
            [
                "america",
                "north",
                "canada",
                "quebec_area",
                "quebec_province"
            ]
            .map(String::from)
        );
    }

    #[test]
    fn rejects_bad_hierarchies() {
        let shallow = "europe = { west = { stockholm } }";
        assert!(
            parse_definitions(shallow, Path::new("d"))
                .unwrap_err()
                .to_string()
                .contains("expected a region")
        );
        let deep = "a = { b = { c = { d = { e = { f = { x } } } } } }";
        assert!(
            parse_definitions(deep, Path::new("d"))
                .unwrap_err()
                .to_string()
                .contains("expected a location")
        );
        let twice = "a = { b = { c = { d = { e = { x } f = { x } } } } }";
        assert!(
            parse_definitions(twice, Path::new("d"))
                .unwrap_err()
                .to_string()
                .contains("x is in both e and f")
        );
    }

    #[test]
    fn reads_kind_lists() {
        let text =
            "wrap_x = yes\nsea_zones = { atlantic }\nlakes = { lake_one }\nsea_zones = { other }";
        let kinds = parse_kinds(text, Path::new("m")).unwrap();
        assert_eq!(kinds["atlantic"], Kind::Sea);
        assert_eq!(kinds["other"], Kind::Sea);
        assert_eq!(kinds["lake_one"], Kind::Lake);
        let deep = "impassable_mountains = { deep }\nsea_zones = { deep }";
        assert_eq!(parse_kinds(deep, Path::new("m")).unwrap()["deep"], Kind::Sea);
        let clash = "sea_zones = { x }\nlakes = { x }";
        assert!(
            parse_kinds(clash, Path::new("m"))
                .unwrap_err()
                .to_string()
                .contains("x is both Sea and Lake")
        );
    }

    #[test]
    fn finds_the_new_world() {
        let placements = parse_definitions(DEFS, Path::new("d")).unwrap();
        let kinds = parse_kinds(
            "sea_zones = { atlantic }\nlakes = { lake_one }",
            Path::new("m"),
        )
        .unwrap();
        let geo = Geography::new(
            &colors(&["stockholm", "uppsala", "quebec", "lake_one", "atlantic"]),
            &placements,
            &kinds,
        )
        .unwrap();
        assert_eq!(geo.new_world().unwrap(), [false, false, true, true, false]);
        assert_eq!(
            geo.kind,
            [Kind::Land, Kind::Land, Kind::Land, Kind::Lake, Kind::Sea]
        );
        assert_eq!(
            geo.on_continents(&["europe", "ocean"]).unwrap(),
            [true, true, false, false, true]
        );
        assert_eq!(
            geo.on_continents(&["atlantis"]).unwrap_err(),
            "definitions.txt has no continent atlantis"
        );

        let extra = colors(&[
            "stockholm",
            "uppsala",
            "quebec",
            "lake_one",
            "atlantic",
            "lost",
        ]);
        let geo = Geography::new(&extra, &placements, &kinds).unwrap();
        assert_eq!(geo.unplaced(&extra, &[1, 1, 1, 1, 1, 0]).unwrap(), [5]);
        assert_eq!(
            geo.unplaced(&extra, &[1, 1, 1, 1, 1, 7]).unwrap_err(),
            "lost has 7 pixels but is not in definitions.txt"
        );
        let short = colors(&["stockholm", "uppsala", "quebec", "lake_one"]);
        assert!(
            Geography::new(&short, &placements, &kinds)
                .err()
                .unwrap()
                .contains("atlantic is not in the colour table")
        );
    }

    /// 需要本機安裝 EU5：`cargo test -p eu5-rnw-core --release -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an installed EU5"]
    fn reads_the_installed_geography() {
        let map_data = crate::game::choose_game(None).unwrap().map_data();
        let map = crate::locations::read_location_map(&map_data).unwrap();
        let locations = &map.locations;
        let geo = read_geography(&map_data, locations).unwrap();
        let unplaced = geo.unplaced(locations, &map.pixel_counts()).unwrap();
        println!("{} unplaced locations without pixels", unplaced.len());
        let new_world = geo.new_world().unwrap();
        let count = |kind| {
            (0..locations.len())
                .filter(|&i| new_world[i] && geo.kind[i] == kind)
                .count()
        };
        println!(
            "{} continents; New World {} of {} locations: {} land, {} lake, {} impassable, {} non-ownable, {} sea",
            geo.continents.len(),
            new_world.iter().filter(|&&n| n).count(),
            locations.len(),
            count(Kind::Land),
            count(Kind::Lake),
            count(Kind::Impassable),
            count(Kind::NonOwnable),
            count(Kind::Sea),
        );
        let oceans = geo.on_continents(&crate::domain::OCEANS).unwrap();
        let islands = (0..oceans.len()).filter(|&i| oceans[i] && geo.kind[i] != Kind::Sea).count();
        println!(
            "{} locations in {:?}, {islands} of them not sea",
            oceans.iter().filter(|&&o| o).count(),
            crate::domain::OCEANS
        );
    }
}
