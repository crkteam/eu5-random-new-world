//! 在生成範圍內切出陸地與海洋（計畫第 7 步）。
//!
//! 1. 在範圍內算噪聲高度場（`noise`）。
//! 2. 範圍邊緣 `EDGE_PX` 內一律是海，再往內 `RAMP_PX` 逐漸壓低高度，海岸會彎離邊界，
//!    也不會貼到範圍外的舊世界（例如貼著格陵蘭的巴芬灣）。
//! 3. 取最高的格子當陸地，數量剛好是範圍的 `land_share`（陸地比例相對整個範圍，同原專案）。
//! 4. 太小的島變成海；被陸地包住的水：大的當內海，其餘填成陸地。
//!    湖不在這裡做：第 9b 步把原版的湖放進它所屬省的新土地裡（擁有者選的做法）。
//!
//! 參數照原專案各世界類型的預設值（`old/src/rnw/settings.py`）。

use crate::domain::distance;
use crate::noise::{NoiseParams, height_field};

/// 範圍邊緣一律是海的寬度，與再往內逐漸壓低高度的寬度（原圖像素）。
/// shortcut: 固定值；原專案依範圍寬度與 coast_margin 算，海岸看起來不自然再改。
pub const EDGE_PX: usize = 200;
pub const RAMP_PX: usize = 200;

/// 一種世界類型的參數。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LandParams {
    /// 陸地佔整個範圍的比例。
    pub land_share: f64,
    pub noise: NoiseParams,
    /// 小於這個面積（原圖像素）的島變成海。
    pub min_island_px: usize,
    /// 被陸地包住的水：大於 `max_lake_px` 當內海，其餘填成陸地。
    pub max_lake_px: usize,
}

const fn params(land_share: f64, feature_px: f64, octaves: u32, persistence: f64, min_island_px: usize) -> LandParams {
    LandParams {
        land_share,
        noise: NoiseParams { feature_px, octaves, persistence },
        min_island_px,
        max_lake_px: 25_000,
    }
}

/// 世界類型（鍵同原專案）與參數。
/// shortcut: 「高牆」原專案有專門的地形（landform），這裡先用它的陸地比例與尺度當一般大陸。
pub const PRESETS: [(&str, LandParams); 6] = [
    ("continents", params(0.34, 1600.0, 7, 0.52, 600)),
    ("supercontinent", params(0.46, 3600.0, 7, 0.47, 6000)),
    ("small_continents", params(0.255, 1000.0, 7, 0.52, 1200)),
    ("archipelago", params(0.17, 420.0, 7, 0.58, 250)),
    ("islands", params(0.153, 300.0, 6, 0.60, 120)),
    ("wall", params(0.212, 1000.0, 7, 0.52, 600)),
];

pub fn preset(key: &str) -> Option<LandParams> {
    PRESETS.iter().find(|(k, _)| *k == key).map(|(_, p)| *p)
}

/// 每個格子的結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// 範圍外，維持原版。
    Outside,
    Sea,
    Land,
    /// 只在第 9b 步出現：原版的湖放進新土地。
    Lake,
}

/// 切出陸地與海洋。`grid` 是 (寬, 高, 每格原圖像素)；`domain` 是第 7a 步的範圍。
pub fn cut(
    seed: u64,
    params: &LandParams,
    grid: (usize, usize, usize),
    domain: &[bool],
    threads: usize,
) -> Result<Vec<Cell>, String> {
    let (width, height, step) = grid;
    let field = height_field(seed, &params.noise, grid, domain, threads);
    let (edge, ramp) = (EDGE_PX.div_ceil(step), RAMP_PX.div_ceil(step));
    let outside: Vec<bool> = domain.iter().map(|&d| !d).collect();
    let away = distance(&outside, (width, height), (edge + ramp + 1) as u32);
    // 邊緣帶內沒有分數（一定是海）；斜坡上最多壓低 2，等於噪聲的整個值域。
    let scores: Vec<Option<f32>> = (0..domain.len())
        .map(|i| {
            let d = away[i] as usize;
            (domain[i] && d >= edge).then(|| {
                let t = ((d - edge) as f32 / ramp.max(1) as f32).min(1.0);
                field.values[i] - 2.0 * (1.0 - t)
            })
        })
        .collect();
    let target = (domain.iter().filter(|&&d| d).count() as f64 * params.land_share).round() as usize;
    let land = highest(&scores, target)?;
    Ok(clean(&land, domain, (width, height, step), params))
}

/// 分數最高的 `count` 個格子（同分時索引小的優先，結果固定）。
fn highest(scores: &[Option<f32>], count: usize) -> Result<Vec<bool>, String> {
    let mut ranked: Vec<(f32, usize)> =
        scores.iter().enumerate().filter_map(|(i, s)| s.map(|s| (s, i))).collect();
    if count > ranked.len() {
        return Err(format!(
            "land share needs {count} cells but only {} are away from the edge",
            ranked.len()
        ));
    }
    ranked.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut land = vec![false; scores.len()];
    for &(_, i) in &ranked[..count] {
        land[i] = true;
    }
    Ok(land)
}

/// 清掉小島、分出內海，其餘被包住的水填成陸地。
fn clean(land: &[bool], domain: &[bool], (width, height, step): (usize, usize, usize), params: &LandParams) -> Vec<Cell> {
    let area = step * step;
    let size = (width, height);
    let mut land = land.to_vec();
    let (parts, sizes) = components(&land, size);
    for (i, cell) in land.iter_mut().enumerate() {
        if *cell && sizes[parts[i] as usize] * area < params.min_island_px {
            *cell = false;
        }
    }
    // 範圍內的水；碰到範圍外的那幾塊是外海，其他被陸地包住。
    let water: Vec<bool> = (0..land.len()).map(|i| domain[i] && !land[i]).collect();
    let (parts, sizes) = components(&water, size);
    let mut open = vec![false; sizes.len()];
    for i in 0..water.len() {
        if water[i] && neighbours(i, size).any(|n| !domain[n]) {
            open[parts[i] as usize] = true;
        }
    }
    (0..land.len())
        .map(|i| {
            if !domain[i] {
                Cell::Outside
            } else if land[i] {
                Cell::Land
            } else {
                let part = parts[i] as usize;
                let px = sizes[part] * area;
                if open[part] || px > params.max_lake_px { Cell::Sea } else { Cell::Land }
            }
        })
        .collect()
}

/// 上下左右的鄰居；x 方向左右相接。
pub(crate) fn neighbours(index: usize, (width, height): (usize, usize)) -> impl Iterator<Item = usize> {
    let (x, y) = (index % width, index / width);
    let left = y * width + (x + width - 1) % width;
    let right = y * width + (x + 1) % width;
    let up = (y > 0).then(|| index - width);
    let down = (y + 1 < height).then(|| index + width);
    [Some(left), Some(right), up, down].into_iter().flatten()
}

/// 連通塊（上下左右相連，x 方向左右相接）：每格的塊編號（不在 `mask` 裡是 `u32::MAX`）與每塊大小。
pub fn components(mask: &[bool], size: (usize, usize)) -> (Vec<u32>, Vec<usize>) {
    let mut part = vec![u32::MAX; mask.len()];
    let mut sizes = Vec::new();
    let mut stack = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || part[start] != u32::MAX {
            continue;
        }
        let id = sizes.len() as u32;
        part[start] = id;
        stack.push(start);
        let mut count = 0;
        while let Some(i) = stack.pop() {
            count += 1;
            for n in neighbours(i, size) {
                if mask[n] && part[n] == u32::MAX {
                    part[n] = id;
                    stack.push(n);
                }
            }
        }
        sizes.push(count);
    }
    (part, sizes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: &[&str]) -> (Vec<bool>, (usize, usize)) {
        let cells = rows.iter().flat_map(|r| r.chars().map(|c| c == '#')).collect();
        (cells, (rows[0].len(), rows.len()))
    }

    #[test]
    fn components_wrap_in_x() {
        let (mask, size) = grid(&["#..#", "....", ".#.."]);
        let (part, sizes) = components(&mask, size);
        assert_eq!(part[0], part[3], "the two edge cells join across the seam");
        assert_eq!(sizes, [2, 1]);
        assert_eq!(part[1], u32::MAX);
    }

    #[test]
    fn highest_takes_the_top_cells() {
        let scores = [Some(0.5), None, Some(0.9), Some(-0.2), Some(0.9)];
        assert_eq!(highest(&scores, 2).unwrap(), [false, false, true, false, true]);
        assert!(highest(&scores, 5).unwrap_err().contains("only 4"));
    }

    #[test]
    fn clean_drops_islands_and_sorts_water() {
        // 範圍是內部 7×5；外圈一格是範圍外。
        let (domain, size) = grid(&[".........", ".#######.", ".#######.", ".#######.", ".#######.", ".#######.", "........."]);
        let (land, _) = grid(&[".........", ".#.......", ".........", "...####..", "...#.##..", "...####..", "........."]);
        let p = LandParams { min_island_px: 2, max_lake_px: 4, ..preset("continents").unwrap() };
        let cells = clean(&land, &domain, (size.0, size.1, 1), &p);
        let at = |x: usize, y: usize| cells[y * size.0 + x];
        assert_eq!(at(0, 0), Cell::Outside);
        assert_eq!(at(1, 1), Cell::Sea, "a one-cell island is too small");
        assert_eq!(at(4, 4), Cell::Land, "a one-cell lake is filled");
        assert_eq!(at(2, 2), Cell::Sea, "water touching the outside is open sea");
        assert_eq!(at(3, 3), Cell::Land);
    }

    #[test]
    fn enclosed_water_fills_or_becomes_inland_sea() {
        let (domain, size) = grid(&["..........", ".########.", ".########.", ".########.", ".########.", ".........."]);
        let (land, _) = grid(&["..........", ".########.", ".#..#...#.", ".#..#...#.", ".########.", ".........."]);
        let p = LandParams { min_island_px: 1, max_lake_px: 5, ..preset("continents").unwrap() };
        let cells = clean(&land, &domain, (size.0, size.1, 1), &p);
        assert_eq!(cells[2 * size.0 + 2], Cell::Land, "4 cells: filled");
        assert_eq!(cells[2 * size.0 + 5], Cell::Sea, "6 cells: an inland sea");
    }

    #[test]
    fn cut_keeps_the_edge_wet_and_the_share() {
        let (w, h, step) = (200, 100, 16);
        let domain: Vec<bool> = (0..w * h).map(|i| (i % w) >= 20 && (i % w) < 180 && i / w >= 10 && i / w < 90).collect();
        let p = LandParams { min_island_px: 0, max_lake_px: usize::MAX, ..preset("continents").unwrap() };
        let cells = cut(7, &p, (w, h, step), &domain, 4).unwrap();
        let inside = domain.iter().filter(|&&d| d).count();
        let land = cells.iter().filter(|&&c| c == Cell::Land).count();
        assert_eq!(land, (inside as f64 * p.land_share).round() as usize);
        let edge = EDGE_PX.div_ceil(step);
        for (i, &c) in cells.iter().enumerate() {
            let (x, y) = (i % w, i / w);
            let near = x < 20 + edge || x >= 180 - edge || y < 10 + edge || y >= 90 - edge;
            assert!(!(near && c == Cell::Land), "land at the edge ({x}, {y})");
        }
        assert_eq!(cut(7, &p, (w, h, step), &domain, 1).unwrap(), cells, "threads do not matter");
    }
}
