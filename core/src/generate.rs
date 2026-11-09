//! 從遊戲檔到新地圖預覽的整條流程：視窗只呼叫這裡。
//!
//! `read_canvas`：讀地圖與地理，縮成每格 `step` 像素，決定生成範圍（第 7a 步）。
//! `generate`：填島洞、分島群、切陸地（第 7b 步）、搬島群。

use std::path::Path;

use crate::domain::{FRAGMENT_PX, GUARD_PX, OCEANS, new_world_domain, tidy};
use crate::geography::{Kind, read_geography};
use crate::islands::{self, COAST_ZONE_PX, Group, ISLAND_CONTINENTS, MARGIN_PX};
use crate::land::{Cell, LandParams, components, cut};
use crate::partition::split;
use crate::rng::at;
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
    let (width, height, mut labels) = map.downsample(step);
    let mut domain = new_world_domain(
        &labels,
        (width, height),
        (&new_world, &ocean, &sea),
        reach_px / step as u32,
        GUARD_PX / step as u32,
    );
    let island_land: Vec<bool> = (0..land.len()).map(|l| land[l] && islandish[l]).collect();
    tidy(&mut labels, &mut domain, (width, height), FRAGMENT_PX.div_ceil(step * step), &island_land);
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

/// 第 9 步：新世界切成地點，數量剛好是可以重用的原版地點數。
pub struct Locations {
    /// 每格的地點編號（範圍外與島群是 `u32::MAX`）：陸地 `0..land`，
    /// 之後是湖 `land..land + lakes`，再來是海。
    pub part: Vec<u32>,
    pub land: usize,
    pub lakes: usize,
    pub seas: usize,
}

/// 範圍內可以重用的原版地點（陸地, 水域），不含島群帶走的地點。
/// shortcut: 不能通行與不可擁有的地點也算陸地，會隨機落在新大陸上；要讓它們成為山脈或荒地時再分開。
pub fn pools(canvas: &Canvas, world: &World) -> (Vec<usize>, Vec<usize>) {
    let mut inside = canvas.new_world.clone();
    for (i, &l) in canvas.labels.iter().enumerate() {
        inside[usize::from(l)] |= canvas.domain[i];
    }
    for group in &world.groups {
        for &l in &group.locations {
            inside[l] = false;
        }
    }
    (0..inside.len()).filter(|&l| inside[l]).partition(|&l| canvas.land[l])
}

/// 把陸地切成陸地地點；每個湖一個地點，其餘水域地點全給海。
pub fn cut_locations(canvas: &Canvas, world: &World, seed: u64) -> Result<Locations, String> {
    let size = (canvas.width, canvas.height);
    let (land_ids, water_ids) = pools(canvas, world);
    let mask = |kind: Cell| -> Vec<bool> { world.cells.iter().map(|&c| c == kind).collect() };
    let (lake, lake_sizes) = components(&mask(Cell::Lake), size);
    let lakes = lake_sizes.len();
    let seas = water_ids.len().checked_sub(lakes).ok_or_else(|| {
        format!("{lakes} lakes but only {} water locations", water_ids.len())
    })?;
    // 陸地和海互不相干，各用一條執行緒；結果與執行緒無關。
    let (land, sea) = std::thread::scope(|scope| {
        let land = scope.spawn(|| split(&mask(Cell::Land), size, land_ids.len(), at(seed, 9)));
        let sea = split(&mask(Cell::Sea), size, seas, at(seed, 10));
        (land.join().expect("land split panicked"), sea)
    });
    let land = land.map_err(|e| format!("land: {e}"))?;
    let sea = sea.map_err(|e| format!("sea: {e}"))?;
    let (l, k) = (land_ids.len() as u32, lakes as u32);
    let part = (0..world.cells.len())
        .map(|i| match world.cells[i] {
            Cell::Land => land[i],
            Cell::Lake => l + lake[i],
            Cell::Sea => l + k + sea[i],
            Cell::Outside => u32::MAX,
        })
        .collect();
    Ok(Locations { part, land: land_ids.len(), lakes, seas })
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

    /// 需要本機安裝 EU5。每個地點都要剛好一塊，數量等於可以重用的原版地點數。
    #[test]
    #[ignore = "needs an installed EU5"]
    fn locations_are_whole_and_counted() {
        let map_data = crate::game::choose_game(None).unwrap().map_data();
        let canvas = read_canvas(&map_data, 4, crate::domain::REACH_PX).unwrap();
        let size = (canvas.width, canvas.height);
        for seed in [42, 7] {
            for (key, params) in PRESETS {
                let world = generate(&canvas, seed, &params, 8).unwrap();
                let started = std::time::Instant::now();
                let found = cut_locations(&canvas, &world, seed).unwrap();
                let took = started.elapsed();
                let (land, water) = pools(&canvas, &world);
                assert_eq!((found.land, found.lakes + found.seas), (land.len(), water.len()), "{key}");
                // 同編號相連的塊數＝地點數，表示每個地點都只有一塊。
                let mut seen = vec![false; found.part.len()];
                let mut pieces = 0;
                for start in 0..found.part.len() {
                    if found.part[start] == u32::MAX || seen[start] {
                        continue;
                    }
                    pieces += 1;
                    seen[start] = true;
                    let mut stack = vec![start];
                    while let Some(i) = stack.pop() {
                        for n in crate::land::neighbours(i, size) {
                            if !seen[n] && found.part[n] == found.part[i] {
                                seen[n] = true;
                                stack.push(n);
                            }
                        }
                    }
                }
                assert_eq!(pieces, land.len() + water.len(), "seed {seed} {key}: a location is in pieces or missing");
                println!("seed {seed} {key}: {} land, {} lakes, {} seas in {took:.2?}", found.land, found.lakes, found.seas);
            }
        }
    }
}
