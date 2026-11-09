//! 第 11b 步：新地圖範圍內的高度（貼花單位）。
//!
//! 數值取自原專案量到的原版（`old/src/rnw/eu5_terrain.py`、claims C40、C46、C48）：
//! - 海：離岸 1～8 個貼花像素依原版中位數剖面從 5292 降到 2234，更遠都是 2234。湖也照這樣（先不分）。
//! - 陸：岸邊 5708，往內陸 `RAMP` 個貼花像素內平滑升到「內陸高度」。
//! - 內陸高度：每個新地點用它在原版的平均高度（平原約 6800、丘陵約 10000、山地約 16000），
//!   在格子上做模糊後雙線性取樣，地點交界才不會有台階；再加上隨高度變大的細噪聲。
//!
//! 距離在完整地點圖上算（每個地點像素 2 個貼花像素），最多 `REACH` 個地點像素。
//!
//! 第 11c 步：`height_layers` 把新高度寫進有改變的高度貼花（沒改到的小塊沿用原版位元組）。

use std::collections::HashMap;
use std::path::Path;

use crate::generate::{Assigned, Canvas, World};
use crate::locations::Label;
use crate::terrain::{Decal, Layer, decode_level0, pack, read_decals, read_height, read_height_layer};
use crate::noise::{NoiseParams, fractal};
use crate::rng::at;

/// 原版海岸剖面：(離岸貼花像素, 高度)。
pub const WATER_PROFILE: [(u32, f64); 8] =
    [(1, 5292.0), (2, 4636.0), (3, 4028.0), (4, 3470.0), (5, 2940.0), (6, 2416.0), (7, 2300.0), (8, 2234.0)];
pub const SEA_FLOOR: f64 = 2234.0;
pub const SHORE: f64 = 5708.0;
/// 陸地從岸邊升到內陸高度的距離（貼花像素）。
pub const RAMP: f64 = 60.0;
/// 原版沒有可用高度的地點（太小、沒落在格子上）用平原的平均高度。
pub const FLATLAND: f64 = 6843.0;
/// 重算範圍往外延伸到範圍外水域的寬度（地點像素）：原版大陸棚 8 個貼花像素的兩倍。
pub const SHELF_SPREAD: usize = 8;
/// 距離最多算到幾個地點像素（`RAMP` 的一半再多一點）。
pub const REACH: u8 = 40;
/// 內陸高度在格子上模糊的半徑（格），兩次方框模糊。
const BLUR_CELLS: usize = 6;
/// 細噪聲：振幅 = `GRAIN_BASE` + `GRAIN_SHARE` × 高出岸邊的部分。
const GRAIN: NoiseParams = NoiseParams { feature_px: 32.0, octaves: 4, persistence: 0.55 };
const GRAIN_BASE: f64 = 300.0;
const GRAIN_SHARE: f64 = 0.15;

/// 新範圍的高度模型。
pub struct Relief {
    /// 完整地點圖大小。
    pub width: usize,
    pub height: usize,
    /// 每個地點像素：0 = 不是生成的；1..=`REACH` = 陸地離水幾像素；128 + d = 水離陸 d 像素。
    pub coast: Vec<u8>,
    /// 格子上的內陸高度（模糊過）。
    base: Vec<f32>,
    grid: (usize, usize, usize),
    seed: u64,
}

const WATER: u8 = 128;

impl Relief {
    /// `new` 是第 10 步的完整地點圖；`vanilla` 是原版高度在格子上的取樣（`terrain::sample_heights(step)`）。
    ///
    /// 要重算高度的像素：新地點是重新分配的，或原本的地點被重新分配或搬走（邊界格子裡換成鄰近地點的零星像素，
    /// 不重算就會在新海裡留下原版海岸的陸地高度）；搬過去的島群所在的格子除外，那裡沿用島群的原版高度。
    pub fn new(canvas: &Canvas, world: &World, done: &Assigned, new: &[Label], vanilla: &[u16], seed: u64) -> Self {
        let (cw, ch, step) = (canvas.width, canvas.height, canvas.step);
        // 每個原版地點在格子上的平均高度（只算陸地高度）。
        let mut sum = vec![(0f64, 0u32); canvas.parents.len()];
        for (i, &l) in canvas.labels.iter().enumerate() {
            if f64::from(vanilla[i]) > SHORE {
                let e = &mut sum[usize::from(l)];
                *e = (e.0 + f64::from(vanilla[i]), e.1 + 1);
            }
        }
        let mean = |l: usize| if sum[l].1 > 0 { sum[l].0 / f64::from(sum[l].1) } else { FLATLAND };
        let land_cell: Vec<bool> = (0..done.cells.len()).map(|i| done.cells[i] == crate::land::Cell::Land).collect();
        let raw: Vec<f32> =
            (0..done.cells.len()).map(|i| if land_cell[i] { mean(usize::from(done.labels[i])) as f32 } else { 0.0 }).collect();
        let base = blur(&raw, &land_cell, (cw, ch));

        let map = &canvas.vanilla;
        let (w, h) = (map.width, map.height);
        let mut changed = vec![false; canvas.parents.len()];
        for &l in &done.ids {
            changed[l] = true;
        }
        let mut island = vec![false; done.cells.len()];
        for (group, &shift) in world.groups.iter().zip(&world.moves) {
            if shift == (0, 0) {
                continue;
            }
            for &l in &group.locations {
                changed[l] = true;
            }
            for &c in &group.cells {
                let y = (c / cw) as isize + shift.0;
                let x = ((c % cw) as isize + shift.1).rem_euclid(cw as isize);
                island[y as usize * cw + x as usize] = true;
            }
        }
        let cell_of = |i: usize| (i / w / step).min(ch - 1) * cw + (i % w / step).min(cw - 1);
        let mut redo: Vec<bool> = (0..new.len())
            .map(|i| !island[cell_of(i)] && (changed[usize::from(new[i])] || changed[usize::from(map.labels[i])]))
            .collect();
        let wet: Vec<bool> = (0..canvas.parents.len()).map(|l| !canvas.land[l]).collect();
        // 範圍外緊鄰的水也重算：原版靠舊海岸的淺水大陸棚（例如楚科奇海貼著美洲北極海岸那圈），
        // 舊陸地變成海以後會在海中央留下一圈舊海岸線。
        spread_into_water(&mut redo, (w, h), SHELF_SPREAD, |i| wet[usize::from(new[i])] && !island[cell_of(i)]);
        let coast = coast_distance(new, (w, h), &redo, &wet);
        Relief { width: w, height: h, coast, base, grid: (cw, ch, step), seed }
    }

    /// 地點像素 (x, y) 是不是生成的。
    pub fn generated(&self, x: usize, y: usize) -> bool {
        self.coast[y * self.width + x] != 0
    }

    /// 貼花像素 (dx, dy)（全世界座標，每地點像素 2 個）的高度；那裡必須是生成的。
    pub fn height_at(&self, dx: usize, dy: usize) -> f64 {
        let (x, y) = (dx / 2, dy / 2);
        let code = self.coast[y * self.width + x];
        debug_assert!(code != 0, "height_at outside the generated area");
        if code >= WATER {
            // 地點像素距離 d ≈ 貼花像素 2d - 0.5。
            let d = (2.0 * f64::from(code - WATER) - 0.5).max(1.0);
            return water_height(d);
        }
        let d = 2.0 * f64::from(code) - 0.5;
        let t = smoothstep((d / RAMP).min(1.0));
        let inland = self.base_at(dx as f64 / 2.0, dy as f64 / 2.0);
        let lifted = SHORE + (inland.max(SHORE) - SHORE) * t;
        let (fx, fy) = (dx as f64 / 2.0, dy as f64 / 2.0);
        let period = self.width as f64;
        let grain = fractal(at(self.seed, 20), &GRAIN, period, fx, fy) * (GRAIN_BASE + GRAIN_SHARE * (lifted - SHORE));
        (lifted + grain * t).max(SHORE)
    }

    /// 內陸高度：格子中心之間雙線性取樣（地點像素座標）。
    fn base_at(&self, x: f64, y: f64) -> f64 {
        let (cw, ch, step) = self.grid;
        let gx = (x / step as f64 - 0.5).max(0.0);
        let gy = (y / step as f64 - 0.5).clamp(0.0, (ch - 1) as f64);
        let (x0, y0) = (gx.floor() as usize % cw, gy.floor() as usize);
        let (x1, y1) = ((x0 + 1) % cw, (y0 + 1).min(ch - 1));
        let (tx, ty) = (gx.fract(), gy.fract());
        let v = |x: usize, y: usize| f64::from(self.base[y * cw + x]);
        let top = v(x0, y0) + (v(x1, y0) - v(x0, y0)) * tx;
        let bottom = v(x0, y1) + (v(x1, y1) - v(x0, y1)) * tx;
        top + (bottom - top) * ty
    }
}

/// 離岸 `d` 個貼花像素的海底高度（原版剖面線性內插）。
pub fn water_height(d: f64) -> f64 {
    for pair in WATER_PROFILE.windows(2) {
        let ((d0, h0), (d1, h1)) = (pair[0], pair[1]);
        if d <= f64::from(d1) {
            let t = ((d - f64::from(d0)) / f64::from(d1 - d0)).clamp(0.0, 1.0);
            return h0 + (h1 - h0) * t;
        }
    }
    SEA_FLOOR
}

fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// 只在陸地格子上做兩次方框模糊（除以陸地權重，海不會把海岸拉低）；x 方向左右相接。
fn blur(values: &[f32], land: &[bool], (w, h): (usize, usize)) -> Vec<f32> {
    let r = BLUR_CELLS as isize;
    let mut value: Vec<f32> = values.to_vec();
    let mut weight: Vec<f32> = land.iter().map(|&l| if l { 1.0 } else { 0.0 }).collect();
    for _ in 0..2 {
        for horizontal in [true, false] {
            let (mut nv, mut nw) = (vec![0f32; value.len()], vec![0f32; value.len()]);
            for y in 0..h {
                for x in 0..w {
                    let (mut sv, mut sw) = (0f32, 0f32);
                    for k in -r..=r {
                        let (xx, yy) = if horizontal {
                            ((x as isize + k).rem_euclid(w as isize) as usize, y)
                        } else {
                            let yy = y as isize + k;
                            if yy < 0 || yy >= h as isize {
                                continue;
                            }
                            (x, yy as usize)
                        };
                        let j = yy * w + xx;
                        sv += value[j] * weight[j];
                        sw += weight[j];
                    }
                    nv[y * w + x] = if sw > 0.0 { sv / sw } else { 0.0 };
                    nw[y * w + x] = sw / (2 * r + 1) as f32;
                }
            }
            (value, weight) = (nv, nw);
        }
    }
    value
}

/// 把 `mask` 往上下左右擴 `rounds` 圈，只擴進 `allowed` 的像素；x 方向左右相接。
fn spread_into_water(mask: &mut [bool], (w, h): (usize, usize), rounds: usize, allowed: impl Fn(usize) -> bool) {
    let mut frontier: Vec<usize> = (0..mask.len()).filter(|&i| mask[i]).collect();
    for _ in 0..rounds {
        let mut next = Vec::new();
        for &i in &frontier {
            let (x, y) = (i % w, i / w);
            let near = [Some(y * w + (x + w - 1) % w), Some(y * w + (x + 1) % w), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)];
            for n in near.into_iter().flatten() {
                if !mask[n] && allowed(n) {
                    mask[n] = true;
                    next.push(n);
                }
            }
        }
        frontier = next;
    }
}

/// `generated` 像素離海岸幾像素（見 `Relief::coast`）。岸線＝上下左右有不同陸水的像素，
/// 從岸線往同一邊一圈圈擴（奇數圈也走斜角，近似圓形），最多 `REACH` 圈。
fn coast_distance(labels: &[Label], (w, h): (usize, usize), generated: &[bool], wet: &[bool]) -> Vec<u8> {
    let is_wet = |i: usize| wet[usize::from(labels[i])];
    let near = |i: usize| -> [Option<usize>; 4] {
        let (x, y) = (i % w, i / w);
        [Some(y * w + (x + w - 1) % w), Some(y * w + (x + 1) % w), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)]
    };
    let mut out = vec![0u8; labels.len()];
    let mut frontier = Vec::new();
    for i in 0..labels.len() {
        if !generated[i] {
            continue;
        }
        let side = is_wet(i);
        if near(i).into_iter().flatten().any(|n| is_wet(n) != side) {
            out[i] = if side { WATER + 1 } else { 1 };
            frontier.push(i);
        }
    }
    for round in 2..=REACH {
        let mut next = Vec::new();
        for &i in &frontier {
            let side = is_wet(i);
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            let corners: &[(isize, isize)] = if round % 2 == 0 { &[(-1, -1), (1, -1), (-1, 1), (1, 1)] } else { &[] };
            for &(ox, oy) in [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().chain(corners) {
                let ny = y + oy;
                if ny < 0 || ny >= h as isize {
                    continue;
                }
                let n = ny as usize * w + (x + ox).rem_euclid(w as isize) as usize;
                if out[n] == 0 && generated[n] && is_wet(n) == side {
                    out[n] = if side { WATER + round } else { round };
                    next.push(n);
                }
            }
        }
        frontier = next;
    }
    // 超過 `REACH` 的生成像素也標成最遠。
    for i in 0..labels.len() {
        if out[i] == 0 && generated[i] {
            out[i] = if is_wet(i) { WATER + REACH } else { REACH };
        }
    }
    out
}

/// 島群搬移後，生成範圍外每格的原版高度要從哪一格取（格子座標）：搬過的島群取來源，其他取自己。
pub fn source_cell(world: &World, (w, _): (usize, usize)) -> Vec<Option<usize>> {
    let mut out = vec![None; world.cells.len()];
    for (group, &(dy, dx)) in world.groups.iter().zip(&world.moves) {
        for &c in &group.cells {
            let y = (c / w) as isize + dy;
            let x = ((c % w) as isize + dx).rem_euclid(w as isize);
            out[y as usize * w + x as usize] = Some(c);
        }
    }
    out
}

/// 第 11c 步的一塊貼花：新的高度層、重新壓縮的第 0 層小塊數、新的高度（北邊在上）。
pub struct DecalLayer {
    pub decal: Decal,
    pub layer: Layer,
    pub changed_tiles: usize,
    pub heights: Vec<u16>,
}

/// 逐塊產生有改變的高度貼花，每塊交給 `each`（一次只留一塊在記憶體）。
/// 生成的地點像素照 `relief`；搬過的島群取來源的原版高度（可能在別塊貼花）；其餘保留原版。
pub fn height_layers(
    game: &Path,
    canvas: &Canvas,
    world: &World,
    relief: &Relief,
    threads: usize,
    mut each: impl FnMut(DecalLayer) -> std::result::Result<(), String>,
) -> std::result::Result<(), String> {
    let (_, decals) = read_decals(game).map_err(|e| e.to_string())?;
    let (cw, step) = (canvas.width, canvas.step);
    // 每格若是搬過的島群的目的地：平移（貼花像素，南北, 東西）。
    let mut shift: Vec<Option<(isize, isize)>> = vec![None; world.cells.len()];
    for (group, &(dy, dx)) in world.groups.iter().zip(&world.moves) {
        if (dy, dx) == (0, 0) {
            continue;
        }
        for &c in &group.cells {
            let y = (c / cw) as isize + dy;
            let x = ((c % cw) as isize + dx).rem_euclid(cw as isize);
            shift[y as usize * cw + x as usize] = Some((dy * (2 * step) as isize, dx * (2 * step) as isize));
        }
    }
    let world_width = 2 * relief.width;
    let mut others: HashMap<usize, Vec<u16>> = HashMap::new();
    for (index, decal) in decals.iter().enumerate() {
        let touched = (decal.top / 2..(decal.top + decal.height) / 2).any(|y| {
            (decal.left / 2..(decal.left + decal.width) / 2)
                .any(|x| relief.generated(x, y) || shift[(y / step) * cw + x / step].is_some())
        });
        if !touched {
            continue;
        }
        let layer = read_height_layer(game, decal).map_err(|e| e.to_string())?;
        let vanilla = decode_level0(&layer, threads).map_err(|e| format!("{}: {e}", decal.name))?;
        let mut heights = vanilla.clone();
        // 島群來源可能在別塊貼花：先找出需要哪些，讀進來。
        let mut sources = Vec::new();
        for y in 0..decal.height {
            for x in 0..decal.width {
                let (gx, gy) = (decal.left + x, decal.top + y);
                if relief.generated(gx / 2, gy / 2) {
                    continue;
                }
                if let Some((dy, dx)) = shift[(gy / 2 / step) * cw + gx / 2 / step] {
                    let sx = (gx as isize - dx).rem_euclid(world_width as isize) as usize;
                    let sy = (gy as isize - dy) as usize;
                    sources.push((y * decal.width + x, sx, sy));
                }
            }
        }
        for &(_, sx, sy) in &sources {
            let owner = decals.iter().position(|d| (d.left..d.left + d.width).contains(&sx) && (d.top..d.top + d.height).contains(&sy));
            let owner = owner.ok_or_else(|| format!("island source {sx},{sy} is in no decal"))?;
            if owner != index && !others.contains_key(&owner) {
                others.insert(owner, read_height(game, &decals[owner], threads).map_err(|e| e.to_string())?);
            }
        }
        for &(at, sx, sy) in &sources {
            let d = decals.iter().position(|d| (d.left..d.left + d.width).contains(&sx) && (d.top..d.top + d.height).contains(&sy)).expect("checked");
            let from = if d == index { &vanilla } else { &others[&d] };
            heights[at] = from[(sy - decals[d].top) * decals[d].width + sx - decals[d].left];
        }
        // 生成的像素，依列分給各執行緒。
        let per = decal.height.div_ceil(threads.max(1));
        std::thread::scope(|scope| {
            for (worker, rows) in heights.chunks_mut(per * decal.width).enumerate() {
                scope.spawn(move || {
                    for (r, row) in rows.chunks_mut(decal.width).enumerate() {
                        let gy = decal.top + worker * per + r;
                        for (x, value) in row.iter_mut().enumerate() {
                            let gx = decal.left + x;
                            if relief.generated(gx / 2, gy / 2) {
                                *value = relief.height_at(gx, gy).round().clamp(0.0, f64::from(u16::MAX)) as u16;
                            }
                        }
                    }
                });
            }
        });
        if heights == vanilla {
            continue;
        }
        let (layer, changed_tiles) = pack(&heights, (decal.width, decal.height), Some((&layer, &vanilla)));
        each(DecalLayer { decal: decal.clone(), layer, changed_tiles, heights })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn water_follows_the_vanilla_shelf() {
        assert_eq!(water_height(1.0), 5292.0);
        assert_eq!(water_height(1.5), (5292.0 + 4636.0) / 2.0);
        assert_eq!(water_height(8.0), SEA_FLOOR);
        assert_eq!(water_height(30.0), SEA_FLOOR);
    }

    #[test]
    fn coast_distance_counts_from_the_shore_on_both_sides() {
        // 一列：標籤 0 是陸、1 是水；5 個陸 + 5 個水，全是生成的。
        let labels: Vec<Label> = (0..10).map(|x| if x < 5 { 0 } else { 1 }).collect();
        let d = coast_distance(&labels, (10, 1), &[true; 10], &[false, true]);
        // 左右相接：最左的陸地也挨著最右的水。
        assert_eq!(d[..5], [1, 2, 3, 2, 1]);
        assert_eq!(d[5..], [WATER + 1, WATER + 2, WATER + 3, WATER + 2, WATER + 1]);
        let none = coast_distance(&labels, (10, 1), &[false; 10], &[false, true]);
        assert!(none.iter().all(|&c| c == 0), "nothing generated, nothing measured");
    }

    #[test]
    fn spreading_stays_in_water_and_stops() {
        let mut mask = vec![false; 8];
        mask[0] = true;
        // 像素 3 是陸地；最右邊經由左右相接也碰到像素 0。
        spread_into_water(&mut mask, (8, 1), 2, |i| i != 3);
        assert_eq!(mask, [true, true, true, false, false, false, true, true]);
    }

    #[test]
    fn blur_ignores_the_sea() {
        let values = [10.0, 10.0, 0.0, 0.0];
        let land = [true, true, false, false];
        let out = blur(&values, &land, (4, 1));
        assert!(out.iter().all(|&v| (v - 10.0).abs() < 1e-4), "{out:?}");
    }

    /// 需要本機安裝 EU5：新的高度貼花讀回來和寫進去的一樣，`.info` 也讀得回來。
    #[test]
    #[ignore = "needs an installed EU5"]
    fn height_layers_read_back() {
        use crate::generate::{assign_ids, cut_locations, generate, read_canvas};
        use crate::terrain::{info_text, parse_info, sample_heights};
        let game = crate::game::choose_game(None).unwrap();
        let canvas = read_canvas(&game.map_data(), 4, crate::domain::REACH_PX).unwrap();
        let seed = 42;
        let world = generate(&canvas, seed, &crate::land::PRESETS[0].1, 8).unwrap();
        let found = cut_locations(&canvas, &world, seed).unwrap();
        let done = assign_ids(&canvas, &world, &found).unwrap();
        let labels = crate::validate::full_map(&canvas, &world, &found, &done);
        let (_, _, vanilla) = sample_heights(&game.directory, 4, 8).unwrap();
        let relief = Relief::new(&canvas, &world, &done, &labels, &vanilla, seed);
        let started = std::time::Instant::now();
        let mut count = 0;
        height_layers(&game.directory, &canvas, &world, &relief, 8, |out| {
            let back = decode_level0(&out.layer, 8)?;
            assert_eq!(back, out.heights, "{} reads back", out.decal.name);
            // 重算過的像素：水的地點不能有陸地高度，陸地不能有海底高度（原版海岸殘留就會被抓到）。
            let mut wrong = 0;
            for (i, &v) in back.iter().enumerate() {
                let (gx, gy) = (out.decal.left + i % out.decal.width, out.decal.top + i / out.decal.width);
                if !relief.generated(gx / 2, gy / 2) {
                    continue;
                }
                let dry = canvas.land[usize::from(labels[gy / 2 * relief.width + gx / 2])];
                wrong += usize::from(if dry { v < 3000 } else { v > 5500 });
            }
            assert_eq!(wrong, 0, "{}: heights disagree with land and water", out.decal.name);
            let info = parse_info(&info_text(&out.layer.info), Path::new("x.info")).map_err(|e| e.to_string())?;
            assert_eq!(info, out.layer.info);
            println!("{}: {} level-0 tiles rewritten, {} MB", out.decal.name, out.changed_tiles, out.layer.bin.len() >> 20);
            count += 1;
            Ok(())
        })
        .unwrap();
        println!("{count} decals in {:.2?}", started.elapsed());
        assert!(count > 0);
    }
}
