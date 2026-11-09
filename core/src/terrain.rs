//! 第 11 步：EU5 地形的高度貼花（11a：讀取）。
//!
//! 美洲的高低起伏不在 `heightmap.png`，而在 16 塊 `heightmap_N_16` 高度貼花（原專案 C25、C36）：
//! - `terrain_cache/ingame_decals.json` 給每塊的中心（地形單位，y 向北）、大小（貼花像素）與 scale；
//!   地形大小在 `terrain_properties.json`（65536×32768），scale 2，所以每個地點像素是 2×2 貼花像素。
//! - 遊戲讀 `.info`／`.bin`（旁邊的 PNG 已過時）：`.info` 列出每個小塊在 `.bin` 的位置與長度。
//!   小塊 128×128，四周各多 2 像素取自鄰居，共 132×132，每塊一段 LZAV 串流；
//!   影像南邊在上（列從南往北），小塊逐列排；第 0 層之後是一層層縮小的版本（這裡不讀）。

use std::path::{Path, PathBuf};

use crate::error::{Error, Result, read_text};
use crate::lzav;

/// 地形資料夾（相對於遊戲資料夾）。
pub const TERRAIN: &str = "game/in_game/gfx/terrain2";
pub const DECAL_CACHE: &str = "terrain_cache/ingame_decals.json";
pub const PROPERTIES: &str = "terrain_properties.json";
pub const TILE: usize = 128;
pub const BORDER: usize = 2;
pub const SIDE: usize = TILE + 2 * BORDER;
/// 高度貼花的 scale：每個貼花像素 2 個地形單位。
pub const SCALE: f64 = 2.0;

/// 一塊高度貼花在全世界貼花座標（北邊在上，每地點像素 2×2）的位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decal {
    pub name: String,
    pub left: usize,
    pub top: usize,
    pub width: usize,
    pub height: usize,
}

/// 全世界的貼花像素大小，與所有 `heightmap_*` 高度貼花。
pub fn read_decals(game: &Path) -> Result<((usize, usize), Vec<Decal>)> {
    let terrain = game.join(TERRAIN);
    let properties_path = terrain.join(PROPERTIES);
    let properties = read_text(&properties_path)?;
    let resolution = properties.find("\"resolution\"").ok_or_else(|| Error::format(&properties_path, "no resolution"))?;
    let world_x = number_after(&properties[resolution..], "\"x\"").ok_or_else(|| Error::format(&properties_path, "no resolution x"))?;
    let world_y = number_after(&properties[resolution..], "\"y\"").ok_or_else(|| Error::format(&properties_path, "no resolution y"))?;
    let world = ((world_x / SCALE) as usize, (world_y / SCALE) as usize);

    let cache_path = terrain.join(DECAL_CACHE);
    let cache = read_text(&cache_path)?;
    let bad = |name: &str, what: &str| Error::format(&cache_path, format!("{name}: {what}"));
    let mut decals = Vec::new();
    for entry in cache.split("\"decal_name\"").skip(1) {
        let name = entry.split('"').nth(1).unwrap_or("");
        if !name.starts_with("heightmap_") {
            continue;
        }
        let field = |section: &str, key: &str| {
            let at = entry.find(section).ok_or_else(|| bad(name, &format!("no {section}")))?;
            number_after(&entry[at..], key).ok_or_else(|| bad(name, &format!("no {section} {key}")))
        };
        let (cx, cy) = (field("\"position\"", "\"x\"")?, field("\"position\"", "\"y\"")?);
        let (sx, sy) = (field("\"scale\"", "\"x\"")?, field("\"scale\"", "\"y\"")?);
        let (w, h) = (field("\"decal_size\"", "\"x\"")?, field("\"decal_size\"", "\"y\"")?);
        if field("\"rotation\"", "")? != 0.0 || sx != SCALE || sy != SCALE {
            return Err(bad(name, "a height decal must be unrotated at scale 2"));
        }
        let left = cx / SCALE - w / 2.0;
        let top = (world_y - cy) / SCALE - h / 2.0;
        if left < 0.0 || top < 0.0 || left + w > world.0 as f64 || top + h > world.1 as f64 || left.fract() != 0.0 || top.fract() != 0.0 {
            return Err(bad(name, "the decal does not sit on whole pixels inside the world"));
        }
        decals.push(Decal { name: name.to_string(), left: left as usize, top: top as usize, width: w as usize, height: h as usize });
    }
    if decals.is_empty() {
        return Err(Error::format(&cache_path, "no heightmap decals"));
    }
    Ok((world, decals))
}

/// `key` 之後的第一個數字（`key` 空字串時就是開頭的第一個數字）。
fn number_after(text: &str, key: &str) -> Option<f64> {
    let rest = &text[text.find(key)? + key.len()..];
    let start = rest.find(|c: char| c.is_ascii_digit() || c == '-')?;
    let end = rest[start..].find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).map_or(rest.len(), |e| start + e);
    rest[start..end].parse().ok()
}

/// 一塊貼花的高度層（`<name>_height.info`／`.bin`）路徑。
pub fn height_paths(game: &Path, decal: &Decal) -> (PathBuf, PathBuf) {
    let folder = game.join(TERRAIN).join("decals").join(&decal.name);
    let stem = format!("{}_height", decal.name);
    (folder.join(format!("{stem}.info")), folder.join(format!("{stem}.bin")))
}

/// `.info` 的內容。
#[derive(Debug, PartialEq, Eq)]
pub struct LayerInfo {
    pub width: usize,
    pub height: usize,
    /// 每個值幾位元組。
    pub bytes: usize,
    /// 每個小塊在 `.bin` 的 (位置, 長度)。
    pub spans: Vec<(usize, usize)>,
}

pub fn parse_info(text: &str, path: &Path) -> Result<LayerInfo> {
    let bad = |what: &str| Error::format(path, what.to_string());
    let at = text.find("source_resolution").ok_or_else(|| bad("no source_resolution"))?;
    let width = number_after(&text[at..], "{").ok_or_else(|| bad("no width"))? as usize;
    let after_width = &text[at..];
    let open = after_width.find('{').ok_or_else(|| bad("no size"))?;
    let mut numbers = after_width[open + 1..].split_whitespace();
    numbers.next();
    let height: usize = numbers.next().and_then(|n| n.parse().ok()).ok_or_else(|| bad("no height"))?;
    let channels = number_after(text, "num_channels").ok_or_else(|| bad("no num_channels"))?;
    let bytes = number_after(text, "bytes_per_channel").ok_or_else(|| bad("no bytes_per_channel"))? as usize;
    if channels != 1.0 || !(1..=2).contains(&bytes) {
        return Err(bad("only one channel of 1 or 2 bytes is supported"));
    }
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("offset=") {
        rest = &rest[at..];
        let offset = number_after(rest, "offset=").ok_or_else(|| bad("bad offset"))? as usize;
        let size = number_after(rest, "size=").ok_or_else(|| bad("bad size"))? as usize;
        spans.push((offset, size));
        rest = &rest["offset=".len()..];
    }
    Ok(LayerInfo { width, height, bytes, spans })
}

/// 一個貼花圖層：`.info` 的內容與 `.bin` 的位元組。
pub struct Layer {
    pub info: LayerInfo,
    pub bin: Vec<u8>,
}

/// 讀一塊貼花的原版高度層檔案。
pub fn read_height_layer(game: &Path, decal: &Decal) -> Result<Layer> {
    let (info_path, bin_path) = height_paths(game, decal);
    let info = parse_info(&read_text(&info_path)?, &info_path)?;
    if (info.width, info.height, info.bytes) != (decal.width, decal.height, 2) {
        return Err(Error::format(
            &info_path,
            format!("{}x{}x{} does not match the decal cache", info.width, info.height, info.bytes),
        ));
    }
    let bin = std::fs::read(&bin_path).map_err(|e| Error::io(&bin_path, e))?;
    Ok(Layer { info, bin })
}

/// 讀一塊貼花的高度（第 0 層），北邊在上。
pub fn read_height(game: &Path, decal: &Decal, threads: usize) -> Result<Vec<u16>> {
    let layer = read_height_layer(game, decal)?;
    decode_level0(&layer, threads).map_err(|message| Error::format(&height_paths(game, decal).1, message))
}

/// 解開一層的第 0 層（2 位元組），北邊在上；`threads` 條執行緒依小塊列分工，結果與執行緒數無關。
pub fn decode_level0(layer: &Layer, threads: usize) -> std::result::Result<Vec<u16>, String> {
    let LayerInfo { width, height, bytes, ref spans } = layer.info;
    if bytes != 2 {
        return Err(format!("{bytes}-byte samples are not heights"));
    }
    let data = &layer.bin;
    let (down, across) = (height.div_ceil(TILE), width.div_ceil(TILE));
    if spans.len() < down * across {
        return Err(format!("{} tiles listed, level 0 needs {}", spans.len(), down * across));
    }
    let mut south_up = vec![0u16; width * height];
    let per = down.div_ceil(threads.max(1));
    let faults: Vec<std::result::Result<(), String>> = std::thread::scope(|scope| {
        let workers: Vec<_> = south_up
            .chunks_mut(TILE * width * per)
            .enumerate()
            .map(|(worker, chunk)| {
                scope.spawn(move || {
                    for (r, rows) in chunk.chunks_mut(TILE * width).enumerate() {
                        let row_tile = worker * per + r;
                        for column_tile in 0..across {
                            let (offset, size) = spans[row_tile * across + column_tile];
                            let piece = data.get(offset..offset + size).ok_or("tile outside .bin")?;
                            let tile = lzav::decode(piece, SIDE * SIDE * 2)
                                .map_err(|e| format!("tile {row_tile},{column_tile}: {e}"))?;
                            let left = column_tile * TILE;
                            let span = TILE.min(width - left);
                            for y in 0..rows.len() / width {
                                for x in 0..span {
                                    let at = ((BORDER + y) * SIDE + BORDER + x) * 2;
                                    rows[y * width + left + x] = u16::from_le_bytes([tile[at], tile[at + 1]]);
                                }
                            }
                        }
                    }
                    Ok(())
                })
            })
            .collect();
        workers.into_iter().map(|w| w.join().expect("tile worker panicked")).collect()
    });
    if let Some(Err(message)) = faults.into_iter().find(|f| f.is_err()) {
        return Err(message);
    }
    Ok(flip(&south_up, width, height))
}

/// 上下翻轉（南邊在上 ↔ 北邊在上）。
fn flip(samples: &[u16], width: usize, height: usize) -> Vec<u16> {
    let mut out = vec![0u16; samples.len()];
    for (y, row) in samples.chunks_exact(width).enumerate() {
        out[(height - 1 - y) * width..(height - y) * width].copy_from_slice(row);
    }
    out
}

/// 一個小塊的原始位元組（132×132，四周 2 像素取自鄰居，影像邊緣重複邊上的值）。
fn tile_bytes(samples: &[u16], (rows, columns): (usize, usize), row_tile: usize, column_tile: usize) -> Vec<u8> {
    let mut raw = Vec::with_capacity(SIDE * SIDE * 2);
    for i in 0..SIDE {
        let y = (row_tile * TILE + i).saturating_sub(BORDER).min(rows - 1);
        for j in 0..SIDE {
            let x = (column_tile * TILE + j).saturating_sub(BORDER).min(columns - 1);
            raw.extend_from_slice(&samples[y * columns + x].to_le_bytes());
        }
    }
    raw
}

/// 一層的每個縮小層：(列, 欄) 與南邊在上的值；每層取上一層的奇數像素，直到一個小塊。
fn levels(north_up: &[u16], (width, height): (usize, usize)) -> Vec<((usize, usize), Vec<u16>)> {
    let mut out = vec![((height, width), flip(north_up, width, height))];
    loop {
        let ((rows, columns), last) = out.last().expect("level 0");
        if *rows <= TILE && *columns <= TILE {
            return out;
        }
        let (rows, columns) = (*rows, *columns);
        let (next_rows, next_columns) = (rows / 2, columns / 2);
        let next: Vec<u16> = (0..next_rows)
            .flat_map(|i| (0..next_columns).map(move |j| (i, j)))
            .map(|(i, j)| last[(2 * i + 1) * columns + 2 * j + 1])
            .collect();
        out.push(((next_rows, next_columns), next));
    }
}

/// 把北邊在上的高度打包成一層（第 0 層加上一層層縮小）。
/// 給了 `vanilla`（原版的層與它解開的第 0 層），第 0 層裡內容（含邊框）沒變的小塊直接沿用原版位元組。
/// 回傳新的層與重新壓縮的第 0 層小塊數。
pub fn pack(north_up: &[u16], (width, height): (usize, usize), vanilla: Option<(&Layer, &[u16])>) -> (Layer, usize) {
    let old = vanilla.map(|(layer, samples)| (layer, flip(samples, width, height)));
    let (mut bin, mut spans, mut changed) = (Vec::new(), Vec::new(), 0);
    for (index, ((rows, columns), samples)) in levels(north_up, (width, height)).iter().enumerate() {
        let across = columns.div_ceil(TILE);
        for r in 0..rows.div_ceil(TILE) {
            for c in 0..across {
                let raw = tile_bytes(samples, (*rows, *columns), r, c);
                let reused = match &old {
                    Some((layer, before)) if index == 0 && tile_bytes(before, (*rows, *columns), r, c) == raw => {
                        let (offset, size) = layer.info.spans[r * across + c];
                        Some(layer.bin[offset..offset + size].to_vec())
                    }
                    _ => None,
                };
                let packed = reused.unwrap_or_else(|| {
                    changed += usize::from(index == 0);
                    lzav::encode(&raw)
                });
                spans.push((bin.len(), packed.len()));
                bin.extend_from_slice(&packed);
            }
        }
    }
    (Layer { info: LayerInfo { width, height, bytes: 2, spans }, bin }, changed)
}

/// `.info` 的文字，格式同原版。
pub fn info_text(info: &LayerInfo) -> String {
    let tiles: Vec<String> =
        info.spans.iter().map(|(offset, size)| format!("{{\n\t\toffset={offset}\n\t\tsize={size}\n\t}}")).collect();
    format!(
        "source_resolution={{ {} {} }}\nnum_channels=1\nbytes_per_channel={}\nempty_value={{ 0 0 0 0 }}\nlzav=yes\ntile_infos={{ {} }}\n",
        info.width,
        info.height,
        info.bytes,
        tiles.join(" ")
    )
}

/// 全世界的高度縮圖：每 `step` 個地點像素取一點（貼花像素是地點像素的 2 倍）。
/// 回傳 (寬, 高, 高度)，大小同 `LocationMap::downsample(step)`。
pub fn sample_heights(game: &Path, step: usize, threads: usize) -> Result<(usize, usize, Vec<u16>)> {
    let (world, decals) = read_decals(game)?;
    let (w, h) = (world.0 / 2 / step, world.1 / 2 / step);
    let mut out = vec![0u16; w * h];
    for decal in &decals {
        let heights = read_height(game, decal, threads)?;
        for y in (decal.top / 2).div_ceil(step)..((decal.top + decal.height) / 2).div_ceil(step).min(h) {
            for x in (decal.left / 2).div_ceil(step)..((decal.left + decal.width) / 2).div_ceil(step).min(w) {
                let (dx, dy) = (x * step * 2 - decal.left, y * step * 2 - decal.top);
                out[y * w + x] = heights[dy * decal.width + dx];
            }
        }
    }
    Ok((w, h, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_lists_size_and_tiles() {
        let text = "source_resolution={ 8192 4096 }\nnum_channels=1\nbytes_per_channel=2\nempty_value={ 0 0 0 0 }\nlzav=yes\n\
                    tile_infos={ {\n\t\toffset=0\n\t\tsize=17\n\t} {\n\t\toffset=17\n\t\tsize=5\n\t} }\n";
        let info = parse_info(text, Path::new("x.info")).unwrap();
        assert_eq!(info, LayerInfo { width: 8192, height: 4096, bytes: 2, spans: vec![(0, 17), (17, 5)] });
        assert!(parse_info("num_channels=3", Path::new("x.info")).is_err());
    }

    #[test]
    fn packed_layers_decode_and_reuse_untouched_tiles() {
        // 300×200：3×2 個小塊，加縮小層 150×100（2 塊）與 75×50（1 塊）。
        let (w, h) = (300, 200);
        let before: Vec<u16> = (0..w * h).map(|i| ((i % w) * 7 + (i / w) * 13) as u16).collect();
        let (layer, changed) = pack(&before, (w, h), None);
        assert_eq!(changed, 6);
        assert_eq!(layer.info.spans.len(), 6 + 2 + 1);
        assert_eq!(decode_level0(&layer, 3).unwrap(), before);
        let text = info_text(&layer.info);
        assert_eq!(parse_info(&text, Path::new("x.info")).unwrap(), layer.info, "the .info text reads back");
        // 只改北邊左上角一個像素：南邊在上存，所以落在最後一列小塊的第一塊，邊框不跨到別塊。
        let mut after = before.clone();
        after[0] = 9;
        let (again, changed) = pack(&after, (w, h), Some((&layer, &before)));
        assert_eq!(changed, 1);
        assert_eq!(decode_level0(&again, 1).unwrap(), after);
        let (s0, s1) = (layer.info.spans[0], again.info.spans[0]);
        assert_eq!(layer.bin[s0.0..s0.0 + s0.1], again.bin[s1.0..s1.0 + s1.1], "an untouched tile keeps its bytes");
    }

    #[test]
    fn numbers_follow_their_keys() {
        assert_eq!(number_after("\"x\":\t29957,\n\"y\": -3.5", "\"y\""), Some(-3.5));
        assert_eq!(number_after("\"rotation\":\t249.20808,", ""), Some(249.20808));
        assert_eq!(number_after("abc", "\"x\""), None);
    }

    /// 需要本機安裝 EU5：16 塊高度貼花排成 4×4 鋪滿世界，第一塊讀得出來，海底大約 2234。
    #[test]
    #[ignore = "needs an installed EU5"]
    fn height_decals_tile_the_world() {
        let game = crate::game::choose_game(None).unwrap().directory;
        let (world, decals) = read_decals(&game).unwrap();
        assert_eq!(world, (32768, 16384));
        assert_eq!(decals.len(), 16);
        let area: usize = decals.iter().map(|d| d.width * d.height).sum();
        assert_eq!(area, world.0 * world.1, "the decals cover the world once");
        let heights = read_height(&game, &decals[0], 8).unwrap();
        let mut sorted = heights.clone();
        sorted.sort_unstable();
        println!("{}: min {} median {} max {}", decals[0].name, sorted[0], sorted[sorted.len() / 2], sorted[sorted.len() - 1]);
        assert_eq!(read_height(&game, &decals[0], 1).unwrap(), heights, "threads do not matter");
    }

    /// 需要本機安裝 EU5：拼好的高度和原版地點對得上（高於 4000 是陸地），證明位置與上下翻轉都對。
    #[test]
    #[ignore = "needs an installed EU5"]
    fn heights_line_up_with_locations() {
        use crate::geography::{Kind, read_geography};
        let game = crate::game::choose_game(None).unwrap();
        let started = std::time::Instant::now();
        let (w, h, heights) = sample_heights(&game.directory, 8, 8).unwrap();
        let took = started.elapsed();
        let map = crate::locations::read_location_map(&game.map_data()).unwrap();
        let geo = read_geography(&game.map_data(), &map.locations).unwrap();
        let (lw, lh, labels) = map.downsample(8);
        assert_eq!((lw, lh), (w, h));
        let new_world = geo.new_world().unwrap();
        let share = |only: &dyn Fn(usize) -> bool| {
            let points: Vec<usize> = (0..labels.len()).filter(|&i| only(usize::from(labels[i]))).collect();
            let wet = |i: usize| matches!(geo.kind[usize::from(labels[i])], Kind::Sea | Kind::Lake);
            points.iter().filter(|&&i| (heights[i] > 4000) != wet(i)).count() as f64 / points.len() as f64
        };
        let (all, america) = (share(&|l| geo.continent[l].is_some()), share(&|l| new_world[l]));
        println!("{took:.2?}; agree: world {:.3}, america {:.3}", all, america);
        assert!(america > 0.97 && all > 0.97, "heights do not line up with the location map");
    }
}
