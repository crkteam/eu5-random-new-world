//! 地點地圖：`locations.png` 的每個像素顏色對應一個地點，顏色表在
//! `named_locations/00_default.txt`（`stockholm = dda910`）。
//!
//! 結果是一張「標籤圖」：每個像素存地點編號（顏色表中的順序）。
//! 遇到顏色表沒有宣告的顏色、重複的顏色或格式錯誤，都明確失敗。

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::error::{Error, Result, read_text};

/// 地點編號；EU5 約有 3.6 萬個地點，`u16` 足夠，整張地圖只要 268 MB。
pub type Label = u16;

/// 標籤用 `u16`，最多這麼多個地點。
pub const MAX_LOCATIONS: usize = Label::MAX as usize;

/// 原版地圖的相對路徑（相對於 `game/in_game/map_data`）。
pub const NAMED_COLORS: &str = "named_locations/00_default.txt";
pub const LOCATIONS_PNG: &str = "locations.png";

/// 一個地點：名稱與 24 位元 RGB 顏色（`0xRRGGBB`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedColor {
    pub key: String,
    pub color: u32,
}

/// 解析顏色表：每行 `key = rrggbb`，`#` 之後是註解。
pub fn parse_named_colors(text: &str, path: &Path) -> Result<Vec<NamedColor>> {
    let mut entries = Vec::new();
    let mut by_color: HashMap<u32, usize> = HashMap::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let bare = line.split('#').next().unwrap_or("").trim();
        if bare.is_empty() {
            continue;
        }
        let bad = || Error::format(path, format!("line {number}: expected `key = rrggbb`"));
        let (key, hex) = bare.split_once('=').ok_or_else(bad)?;
        let (key, hex) = (key.trim(), hex.trim());
        // 與原專案相同：鍵以字母開頭、只含英數與底線；顏色 1–6 位十六進位（`1e05` = `001e05`）。
        let valid_key = key.starts_with(|c: char| c.is_ascii_alphabetic())
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        let valid_hex = (1..=6).contains(&hex.len()) && hex.chars().all(|c| c.is_ascii_hexdigit());
        if !valid_key || !valid_hex {
            return Err(bad());
        }
        let color = u32::from_str_radix(hex, 16).map_err(|_| bad())?;
        if let Some(first) = by_color.insert(color, number) {
            return Err(Error::format(
                path,
                format!("line {number}: colour {hex} already used on line {first}"),
            ));
        }
        if let Some(first) = by_key.insert(key.to_owned(), number) {
            return Err(Error::format(
                path,
                format!("line {number}: {key} already declared on line {first}"),
            ));
        }
        entries.push(NamedColor { key: key.to_owned(), color });
    }
    if entries.len() > MAX_LOCATIONS {
        return Err(Error::format(
            path,
            format!("{} locations; at most {MAX_LOCATIONS} fit", entries.len()),
        ));
    }
    Ok(entries)
}

/// 整張地圖的地點標籤。
pub struct LocationMap {
    pub width: usize,
    pub height: usize,
    /// 地點，`labels` 的值就是這裡的索引。
    pub locations: Vec<NamedColor>,
    /// 逐列儲存：`labels[y * width + x]`。
    pub labels: Vec<Label>,
}

impl LocationMap {
    pub fn label(&self, x: usize, y: usize) -> Label {
        self.labels[y * self.width + x]
    }

    /// 每 `step` 像素取一點的縮小標籤圖：(寬, 高, 標籤)。
    pub fn downsample(&self, step: usize) -> (usize, usize, Vec<Label>) {
        let (w, h) = (self.width / step, self.height / step);
        let labels = (0..h)
            .flat_map(|y| (0..w).map(move |x| self.label(x * step, y * step)))
            .collect();
        (w, h, labels)
    }

    /// 每個地點佔多少像素（索引同 `locations`）。
    pub fn pixel_counts(&self) -> Vec<u64> {
        let mut counts = vec![0u64; self.locations.len()];
        for &label in &self.labels {
            counts[usize::from(label)] += 1;
        }
        counts
    }
}

/// 顏色 → 地點編號的查表，涵蓋全部 2^24 種顏色（32 MB），比雜湊表快很多。
struct ColorTable(Vec<Label>);

impl ColorTable {
    /// 沒有宣告的顏色。
    const NONE: Label = Label::MAX;

    fn new(locations: &[NamedColor]) -> Self {
        let mut table = vec![Self::NONE; 1 << 24];
        for (index, location) in locations.iter().enumerate() {
            // parse_named_colors 已保證數量 < Label::MAX，所以不會撞到 NONE。
            table[location.color as usize] = index as Label;
        }
        ColorTable(table)
    }
}

/// 把一列 RGB 像素換成標籤；回傳第一個未宣告顏色的 x 與該列未宣告的數量。
fn label_row(table: &ColorTable, rgb: &[u8], out: &mut [Label]) -> (Option<usize>, u64) {
    let mut first = None;
    let mut missing = 0;
    for (x, (pixel, label)) in rgb.chunks_exact(3).zip(out.iter_mut()).enumerate() {
        let color = (u32::from(pixel[0]) << 16) | (u32::from(pixel[1]) << 8) | u32::from(pixel[2]);
        *label = table.0[color as usize];
        if *label == ColorTable::NONE {
            missing += 1;
            first.get_or_insert(x);
        }
    }
    (first, missing)
}

/// 讀取 `map_data` 資料夾的顏色表與 `locations.png`，建立標籤圖。
pub fn read_location_map(map_data: &Path) -> Result<LocationMap> {
    let colors_path = map_data.join(NAMED_COLORS);
    let locations = parse_named_colors(&read_text(&colors_path)?, &colors_path)?;
    let png_path = map_data.join(LOCATIONS_PNG);
    label_png(&png_path, &locations).map(|(width, height, labels)| LocationMap {
        width,
        height,
        locations,
        labels,
    })
}

/// 逐列解碼 PNG（不把整張 400 MB 的 RGB 留在記憶體），每列直接換成標籤。
fn label_png(path: &Path, locations: &[NamedColor]) -> Result<(usize, usize, Vec<Label>)> {
    let png_error = |e: png::DecodingError| Error::format(path, e.to_string());
    let file = File::open(path).map_err(|e| Error::io(path, e))?;
    let mut reader = png::Decoder::new(BufReader::new(file)).read_info().map_err(png_error)?;
    let info = reader.info();
    let (width, height) = (info.width as usize, info.height as usize);
    if info.interlaced {
        return Err(Error::format(path, "interlaced PNG is not supported"));
    }
    match reader.output_color_type() {
        (png::ColorType::Rgb, png::BitDepth::Eight) => {}
        other => return Err(Error::format(path, format!("expected 8-bit RGB, found {other:?}"))),
    }

    let table = ColorTable::new(locations);
    let mut labels = vec![0 as Label; width * height];
    let mut row = vec![0u8; width * 3];
    let mut missing = 0u64;
    let mut first_missing = None;
    for (y, out) in labels.chunks_exact_mut(width).enumerate() {
        if reader.read_row(&mut row).map_err(png_error)?.is_none() {
            return Err(Error::format(path, format!("image ends at row {y} of {height}")));
        }
        let (first, count) = label_row(&table, &row, out);
        missing += count;
        if let (None, Some(x)) = (first_missing, first) {
            first_missing = Some((x, y));
        }
    }
    if let Some((x, y)) = first_missing {
        return Err(Error::format(
            path,
            format!("{missing} pixels have colours missing from {NAMED_COLORS}, first at ({x}, {y})"),
        ));
    }
    Ok((width, height, labels))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors(text: &str) -> Result<Vec<NamedColor>> {
        parse_named_colors(text, Path::new("test.txt"))
    }

    #[test]
    fn reads_pairs_and_comments() {
        let parsed = colors("\u{feff}# 標題\nstockholm = dda910 # 首都\n\nuppsala=540601\n").unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], NamedColor { key: "stockholm".into(), color: 0xdd_a9_10 });
        assert_eq!(parsed[1].key, "uppsala");
        // 省略前導 0 的短顏色
        assert_eq!(colors("kastelholm = 1e05").unwrap()[0].color, 0x00_1e_05);
    }

    #[test]
    fn refuses_bad_and_duplicate_entries() {
        let message = |text| colors(text).unwrap_err().to_string();
        assert!(message("a = 1234567").contains("line 1"));
        assert!(message("a = zzzzzz").contains("expected"));
        assert!(message("1a = 000001").contains("expected"));
        assert!(message("a = 000001\nb = 000001").contains("already used on line 1"));
        assert!(message("a = 000001\na = 000002").contains("already declared"));
    }

    #[test]
    fn labels_a_row_and_counts_unknown_colours() {
        let locations = colors("a = 000000\nb = ff0000").unwrap();
        let table = ColorTable::new(&locations);
        let rgb = [0, 0, 0, 255, 0, 0, 1, 2, 3, 255, 0, 0];
        let mut out = [0; 4];
        let (first, missing) = label_row(&table, &rgb, &mut out);
        assert_eq!(&out[..2], &[0, 1]);
        assert_eq!(out[3], 1);
        assert_eq!((first, missing), (Some(2), 1));
    }

    /// 需要本機安裝 EU5：`cargo test -p eu5-rnw-core --release -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an installed EU5"]
    fn reads_the_installed_map() {
        let game = crate::game::choose_game(None).unwrap();
        let start = std::time::Instant::now();
        let map = read_location_map(&game.map_data()).unwrap();
        let seconds = start.elapsed().as_secs_f64();
        let counts = map.pixel_counts();
        let unused = counts.iter().filter(|&&c| c == 0).count();
        let (largest, pixels) = counts.iter().enumerate().max_by_key(|(_, c)| **c).unwrap();
        println!(
            "{}x{}, {} locations, {unused} without pixels, largest {} ({pixels} px), {seconds:.2} s",
            map.width,
            map.height,
            map.locations.len(),
            map.locations[largest].key,
        );
        assert_eq!(counts.iter().sum::<u64>(), (map.width * map.height) as u64);
    }

    #[test]
    fn reads_a_small_png() {
        let dir = std::env::temp_dir().join(format!("eu5rnw-png-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("named_locations")).unwrap();
        std::fs::write(dir.join(NAMED_COLORS), "sea = 0000ff\nland = 00ff00\n").unwrap();
        // 3×2：上排 海 陸 陸，下排 海 海 陸
        let pixels: [u8; 18] = [0, 0, 255, 0, 255, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255, 0, 255, 0];
        let mut encoder = png::Encoder::new(File::create(dir.join(LOCATIONS_PNG)).unwrap(), 3, 2);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(&pixels).unwrap();

        let map = read_location_map(&dir).unwrap();
        assert_eq!((map.width, map.height), (3, 2));
        assert_eq!(map.labels, [0, 1, 1, 0, 0, 1]);
        assert_eq!(map.label(2, 1), 1);
        assert_eq!(map.pixel_counts(), [3, 3]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
