//! 從遊戲檔到新地圖預覽的整條流程：視窗只呼叫這裡。
//!
//! `read_canvas`：讀地圖與地理，縮成每格 `step` 像素，決定生成範圍（第 7a 步）。
//! `generate`：填島洞、分島群、切陸地（第 7b 步）、搬島群。

use std::path::Path;

use crate::domain::{GUARD_PX, OCEANS, new_world_domain};
use crate::geography::{Kind, read_geography};
use crate::islands::{self, COAST_ZONE_PX, Group, ISLAND_CONTINENTS, MARGIN_PX};
use crate::land::{Cell, LandParams, cut};
use crate::locations::{Label, read_location_map};

/// 縮小的地圖與生成範圍；以地點索引的欄位是每個地點一筆。
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub step: usize,
    pub labels: Vec<Label>,
    pub new_world: Vec<bool>,
    /// 在 `OCEANS` 大陸裡。
    pub ocean: Vec<bool>,
    pub sea: Vec<bool>,
    /// 陸地（不是海也不是湖）。
    pub land: Vec<bool>,
    /// 在 `ISLAND_CONTINENTS` 大陸裡。
    pub islandish: Vec<bool>,
    /// 夠小、碰到島就跟著島走的海域。
    pub coast_zone: Vec<bool>,
    pub domain: Vec<bool>,
}

/// 讀 `map_data` 並決定生成範圍：延伸 `reach_px`、緩衝 `GUARD_PX`。
pub fn read_canvas(map_data: &Path, step: usize, reach_px: u32) -> Result<Canvas, String> {
    let map = read_location_map(map_data).map_err(|e| e.to_string())?;
    let geo = read_geography(map_data, &map.locations).map_err(|e| e.to_string())?;
    let pixels = map.pixel_counts();
    geo.unplaced(&map.locations, &pixels)?;
    let new_world = geo.new_world()?;
    let ocean = geo.on_continents(&OCEANS)?;
    let islandish = geo.on_continents(&ISLAND_CONTINENTS)?;
    let sea: Vec<bool> = geo.kind.iter().map(|&k| k == Kind::Sea).collect();
    let land: Vec<bool> = geo.kind.iter().map(|&k| k != Kind::Sea && k != Kind::Lake).collect();
    let coast_zone: Vec<bool> = (0..sea.len()).map(|l| sea[l] && pixels[l] <= COAST_ZONE_PX).collect();
    let (width, height, labels) = map.downsample(step);
    let domain = new_world_domain(
        &labels,
        (width, height),
        (&new_world, &ocean, &sea),
        reach_px / step as u32,
        GUARD_PX / step as u32,
    );
    Ok(Canvas { width, height, step, labels, new_world, ocean, sea, land, islandish, coast_zone, domain })
}

/// 生成結果：每格的類別，以及最後的標籤（搬過的島群在新位置）。
pub struct World {
    pub cells: Vec<Cell>,
    pub labels: Vec<Label>,
    pub groups: Vec<Group>,
    /// 每個島群的平移（南北, 東西，以格計）。
    pub moves: Vec<(isize, isize)>,
}

/// 在範圍內生成新世界，保留並必要時搬移原版的島群。
pub fn generate(canvas: &Canvas, seed: u64, params: &LandParams, threads: usize) -> Result<World, String> {
    let size = (canvas.width, canvas.height);
    let (domain, pinned) = islands::fill_holes(&canvas.labels, size, &canvas.domain, &canvas.land, &canvas.islandish);
    let groups = islands::groups(&canvas.labels, size, canvas.step, &pinned, &canvas.coast_zone)?;
    let mut cells = cut(seed, params, (canvas.width, canvas.height, canvas.step), &domain, threads)?;
    let moves = islands::place(&cells, &domain, size, &groups, MARGIN_PX.div_ceil(canvas.step))?;
    let mut labels = canvas.labels.clone();
    islands::carve(&mut cells, &mut labels, size, &groups, &moves);
    Ok(World { cells, labels, groups, moves })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::land::PRESETS;

    /// 需要本機安裝 EU5：`cargo test -p eu5-rnw-core --release -- --ignored --nocapture`
    /// 每種世界類型、幾個種子都要生成成功，搬過的島群周圍 `MARGIN_PX` 內沒有新陸地。
    #[test]
    #[ignore = "needs an installed EU5"]
    fn islands_end_on_open_sea() {
        let map_data = crate::game::choose_game(None).unwrap().map_data();
        let canvas = read_canvas(&map_data, 4, crate::domain::REACH_PX).unwrap();
        let size = (canvas.width, canvas.height);
        let margin = MARGIN_PX.div_ceil(canvas.step) as u32;
        for seed in [42, 7, 1000] {
            for (key, params) in PRESETS {
                let world = generate(&canvas, seed, &params, 8).unwrap();
                let mut kept = vec![false; world.cells.len()];
                for (group, &(dy, dx)) in world.groups.iter().zip(&world.moves) {
                    for &i in &group.cells {
                        let y = (i / canvas.width) as isize + dy;
                        let x = ((i % canvas.width) as isize + dx).rem_euclid(canvas.width as isize);
                        kept[y as usize * canvas.width + x as usize] = true;
                    }
                }
                let away = crate::domain::distance(&kept, size, margin + 1);
                let crowded = (0..kept.len()).filter(|&i| away[i] < margin && world.cells[i] == Cell::Land).count();
                assert_eq!(crowded, 0, "seed {seed} {key}: new land within the margin of an island group");
                println!("seed {seed} {key}: {} groups, {} moved", world.groups.len(), world.moves.iter().filter(|&&m| m != (0, 0)).count());
            }
        }
    }
}
