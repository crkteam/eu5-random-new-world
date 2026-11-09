//! 把一種格子（陸地、海或湖）切成指定數量的地點（計畫第 9 步）。
//!
//! 每個不相連的部分至少一塊，其餘依面積分配。每部分內依大尺度的密度噪聲撒種子
//! （有的地方地點密、有的地方疏），依「離種子的距離」
//! 從種子往外長（上下左右一步一步長，所以每塊都相連），再做幾輪 Lloyd：種子移到所在塊的
//! 中心附近再重長，塊的大小會比較平均。
//!
//! 距離是在噪聲位移過的座標上量的（原專案 `wobble_borders` 的位移量與尺度），邊界才會彎曲，
//! 不是整齊的蜂巢；因為還是一格一格往相鄰長，不需要原專案事後的修補。
//! 全部單執行緒、同分時依格子編號，結果固定。

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::land::{components, neighbours};
use crate::noise::{NoiseParams, fractal};
use crate::rng::at;

/// Lloyd 的輪數；少幾輪，地點大小才有些差異。
pub const LLOYD_ROUNDS: usize = 2;
/// 邊界的噪聲位移（原圖像素）與噪聲尺度，同原專案。
pub const WOBBLE_PX: f64 = 16.0;
const WOBBLE: NoiseParams = NoiseParams { feature_px: 48.0, octaves: 3, persistence: 0.5 };
/// 地點密度的大尺度變化：密度 = exp(`DENSITY_SPREAD` × 噪聲)，像原版有的地方地點密、有的地方疏。
/// shortcut: 純噪聲；原版是沿海、河谷密，內陸、高山、極地疏，有地形資料後再照地形。
const DENSITY: NoiseParams = NoiseParams { feature_px: 1200.0, octaves: 3, persistence: 0.5 };
const DENSITY_SPREAD: f64 = 1.5;
/// 位移以 1/`SUB` 格為單位存成整數。
const SUB: i64 = 16;

/// 把 `mask` 的格子切成剛好 `count` 塊；回傳每格的塊編號（不在 `mask` 裡是 `u32::MAX`）。
/// 不相連的部分比 `count` 多、或格子比 `count` 少，就失敗。
/// `step` 是每格的原圖像素。
pub fn split(mask: &[bool], size: (usize, usize), step: usize, count: usize, seed: u64) -> Result<Vec<u32>, String> {
    let (part, sizes) = components(mask, size);
    let total: usize = sizes.iter().sum();
    if sizes.len() > count {
        return Err(format!("{} separate pieces but only {count} locations to give them", sizes.len()));
    }
    if total < count {
        return Err(format!("{count} locations but only {total} cells"));
    }
    let quota = allot(&sizes, count);
    // 每部分依密度加權抽 `quota` 格當種子（加權抽樣：取 -ln(u)/密度 最小的）。
    let (width, period) = (size.0, (size.0 * step) as f64);
    let key = |i: usize| {
        let (x, y) = (((i % width) * step) as f64, ((i / width) * step) as f64);
        let density = (DENSITY_SPREAD * fractal(at(seed, 3), &DENSITY, period, x, y)).exp();
        let u = ((at(seed, i as u64) >> 11) as f64 + 1.0) / (1u64 << 53) as f64;
        -u.ln() / density
    };
    let mut cells: Vec<(u32, f64, usize)> = (0..mask.len()).filter(|&i| mask[i]).map(|i| (part[i], key(i), i)).collect();
    cells.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
    let cells: Vec<usize> = cells.into_iter().map(|c| c.2).collect();
    let mut seeds = Vec::with_capacity(count);
    let mut start = 0;
    for (p, &q) in quota.iter().enumerate() {
        seeds.extend_from_slice(&cells[start..start + q]);
        start += sizes[p];
    }
    let wobble = wobble(mask, size, step, seed);
    let mut owner = grow(mask, size, &seeds, &wobble);
    for _ in 0..LLOYD_ROUNDS {
        seeds = centres(&owner, size, &seeds);
        owner = grow(mask, size, &seeds, &wobble);
    }
    Ok(owner)
}

/// 每部分至少 1，其餘依大小按比例（最大餘數），且不超過該部分的格子數。
fn allot(sizes: &[usize], count: usize) -> Vec<usize> {
    let total: usize = sizes.iter().sum();
    let extra = count - sizes.len();
    let mut quota: Vec<usize> = sizes.iter().map(|&s| 1 + extra * s / total).collect();
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&p| (Reverse(extra * sizes[p] % total), p));
    let mut left = count - quota.iter().sum::<usize>();
    while left > 0 {
        for &p in &order {
            if left > 0 && quota[p] < sizes[p] {
                quota[p] += 1;
                left -= 1;
            }
        }
    }
    quota
}

/// 每格的噪聲位移（1/`SUB` 格）；x 方向的噪聲隨地圖左右相接。
fn wobble(mask: &[bool], (width, _): (usize, usize), step: usize, seed: u64) -> Vec<(i64, i64)> {
    let period = (width * step) as f64;
    let scale = WOBBLE_PX / step as f64 * SUB as f64;
    (0..mask.len())
        .map(|i| {
            if !mask[i] {
                return (0, 0);
            }
            let (x, y) = (((i % width) * step) as f64, ((i / width) * step) as f64);
            let along = |key: u64| (fractal(at(seed, key), &WOBBLE, period, x, y) * scale).round() as i64;
            (along(1), along(2))
        })
        .collect()
}

/// 從各種子往外長：每格歸給「能從自己的塊走到、（位移後）離種子最近」的種子。
fn grow(mask: &[bool], size: (usize, usize), seeds: &[usize], wobble: &[(i64, i64)]) -> Vec<u32> {
    let mut owner = vec![u32::MAX; mask.len()];
    let mut heap: BinaryHeap<Reverse<(u64, usize, u32)>> =
        seeds.iter().enumerate().map(|(p, &s)| Reverse((0, s, p as u32))).collect();
    while let Some(Reverse((_, i, p))) = heap.pop() {
        if owner[i] != u32::MAX {
            continue;
        }
        owner[i] = p;
        for n in neighbours(i, size) {
            if mask[n] && owner[n] == u32::MAX {
                heap.push(Reverse((distance2(seeds[p as usize], n, size, wobble), n, p)));
            }
        }
    }
    owner
}

/// 兩格位移後的直線距離平方（1/`SUB` 格）；x 方向左右相接。
fn distance2(a: usize, b: usize, (width, _): (usize, usize), wobble: &[(i64, i64)]) -> u64 {
    let dx = offset(a % width, b % width, width) * SUB + wobble[b].0 - wobble[a].0;
    let dy = ((b / width) as i64 - (a / width) as i64) * SUB + wobble[b].1 - wobble[a].1;
    (dx * dx + dy * dy) as u64
}

/// 從 `a` 到 `b` 的 x 位移，走較短的那邊。
fn offset(a: usize, b: usize, width: usize) -> i64 {
    let d = (b as i64 - a as i64).rem_euclid(width as i64);
    if d > width as i64 / 2 { d - width as i64 } else { d }
}

/// 每塊的新種子：塊裡離塊中心最近的格子（中心以舊種子為原點算，處理左右相接）。
fn centres(owner: &[u32], size: (usize, usize), seeds: &[usize]) -> Vec<usize> {
    let width = size.0;
    let mut sum = vec![(0i64, 0i64, 0i64); seeds.len()];
    for (i, &p) in owner.iter().enumerate() {
        if p != u32::MAX {
            let s = seeds[p as usize];
            let e = &mut sum[p as usize];
            e.0 += offset(s % width, i % width, width);
            e.1 += (i / width) as i64 - (s / width) as i64;
            e.2 += 1;
        }
    }
    let mut best = vec![(i128::MAX, 0usize); seeds.len()];
    for (i, &p) in owner.iter().enumerate() {
        if p != u32::MAX {
            let s = seeds[p as usize];
            let (sx, sy, n) = sum[p as usize];
            // 放大 n 倍比較，避免除法的誤差；i128 防止大塊溢位。
            let dx = (offset(s % width, i % width, width) * n - sx) as i128;
            let dy = (((i / width) as i64 - (s / width) as i64) * n - sy) as i128;
            let d = dx * dx + dy * dy;
            best[p as usize] = best[p as usize].min((d, i));
        }
    }
    best.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connected(owner: &[u32], size: (usize, usize), part: u32) -> bool {
        let mask: Vec<bool> = owner.iter().map(|&p| p == part).collect();
        components(&mask, size).1.len() == 1
    }

    #[test]
    fn allot_gives_every_piece_one_and_the_rest_by_size() {
        assert_eq!(allot(&[90, 9, 1], 12), [9, 2, 1], "9 more by share: 8.1, 0.81, 0.09 → largest remainders");
        assert_eq!(allot(&[1, 1, 1], 3), [1, 1, 1]);
        // 第二部分只有 2 格，多的給別人。
        assert_eq!(allot(&[10, 2], 12), [10, 2]);
    }

    #[test]
    fn split_makes_count_connected_parts() {
        // 兩塊陸地（中間一欄隔開），x 方向左右相接所以右邊那塊跨過接縫。
        let (w, h) = (60, 30);
        let mask: Vec<bool> = (0..w * h).map(|i| i % w != 20 && i % w != 50).collect();
        let owner = split(&mask, (w, h), 4, 17, 9).unwrap();
        for p in 0..17 {
            assert!(connected(&owner, (w, h), p), "part {p} is in one piece");
        }
        assert!(owner.iter().zip(&mask).all(|(&o, &m)| (o != u32::MAX) == m));
        let biggest = (0..17).map(|p| owner.iter().filter(|&&o| o == p).count()).max().unwrap();
        assert!(biggest < 3 * w * h / 17, "parts are roughly even, the biggest has {biggest}");
        assert_eq!(split(&mask, (w, h), 4, 17, 9).unwrap(), owner, "same seed, same parts");
    }

    #[test]
    fn split_fails_when_it_cannot() {
        let mask = [true, false, true, false];
        assert!(split(&mask, (4, 1), 4, 1, 0).unwrap_err().contains("2 separate pieces"));
        assert!(split(&mask, (4, 1), 4, 3, 0).unwrap_err().contains("only 2 cells"));
    }
}
