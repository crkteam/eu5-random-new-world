//! 第 11b 步：新地圖範圍內的高度（貼花單位）。
//!
//! 數值取自原專案量到的原版（`old/src/rnw/eu5_terrain.py`、claims C40、C46、C48）：
//! - 海：離岸 1～8 個貼花像素依原版中位數剖面從 5292 降到 2234，更遠都是 2234。湖也照這樣（先不分）。
//! - 陸：岸邊 5708，往內陸 `RAMP` 個貼花像素內平滑升到「內陸高度」。
//! - 內陸高度：每個新地點用它在原版的平均高度（平原約 6800、丘陵約 10000、山地約 16000），
//!   在格子上做模糊後雙線性取樣，地點交界才不會有台階；再加上隨高度變大的細噪聲。
//!
//! 距離在完整地點圖上算（每個地點像素 2 個貼花像素），最多 `REACH` 個地點像素。

use crate::generate::{Assigned, Canvas, World};
use crate::locations::Label;
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
    pub fn new(canvas: &Canvas, done: &Assigned, new: &[Label], vanilla: &[u16], seed: u64) -> Self {
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
        let mut generated = vec![false; canvas.parents.len()];
        for &l in &done.ids {
            generated[l] = true;
        }
        let wet: Vec<bool> = (0..canvas.parents.len()).map(|l| !canvas.land[l]).collect();
        let coast = coast_distance(new, (w, h), &generated, &wet);
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

/// 生成區的每個地點像素離海岸幾像素（見 `Relief::coast`）。岸線＝上下左右有不同陸水的像素，
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
        if !generated[usize::from(labels[i])] {
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
                if out[n] == 0 && generated[usize::from(labels[n])] && is_wet(n) == side {
                    out[n] = if side { WATER + round } else { round };
                    next.push(n);
                }
            }
        }
        frontier = next;
    }
    // 超過 `REACH` 的生成像素也標成最遠。
    for i in 0..labels.len() {
        if out[i] == 0 && generated[usize::from(labels[i])] {
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
        let d = coast_distance(&labels, (10, 1), &[true, true], &[false, true]);
        // 左右相接：最左的陸地也挨著最右的水。
        assert_eq!(d[..5], [1, 2, 3, 2, 1]);
        assert_eq!(d[5..], [WATER + 1, WATER + 2, WATER + 3, WATER + 2, WATER + 1]);
        let none = coast_distance(&labels, (10, 1), &[false, false], &[false, true]);
        assert!(none.iter().all(|&c| c == 0), "nothing generated, nothing measured");
    }

    #[test]
    fn blur_ignores_the_sea() {
        let values = [10.0, 10.0, 0.0, 0.0];
        let land = [true, true, false, false];
        let out = blur(&values, &land, (4, 1));
        assert!(out.iter().all(|&v| (v - 10.0).abs() < 1e-4), "{out:?}");
    }
}
