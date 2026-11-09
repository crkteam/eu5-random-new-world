//! 把一種格子（陸地、海或湖）切成指定數量的地點（計畫第 9 步）。
//!
//! 每個不相連的部分至少一塊，其餘依面積分配。每部分內隨機撒種子，依「離種子的直線距離」
//! 從種子往外長（上下左右一步一步長，所以每塊都相連），再做幾輪 Lloyd：種子移到所在塊的
//! 中心附近再重長，塊的大小會比較平均。全部單執行緒、同分時依格子編號，結果固定。

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::land::{components, neighbours};
use crate::rng::at;

/// Lloyd 的輪數。
pub const LLOYD_ROUNDS: usize = 4;

/// 把 `mask` 的格子切成剛好 `count` 塊；回傳每格的塊編號（不在 `mask` 裡是 `u32::MAX`）。
/// 不相連的部分比 `count` 多、或格子比 `count` 少，就失敗。
pub fn split(mask: &[bool], size: (usize, usize), count: usize, seed: u64) -> Result<Vec<u32>, String> {
    let (part, sizes) = components(mask, size);
    let total: usize = sizes.iter().sum();
    if sizes.len() > count {
        return Err(format!("{} separate pieces but only {count} locations to give them", sizes.len()));
    }
    if total < count {
        return Err(format!("{count} locations but only {total} cells"));
    }
    let quota = allot(&sizes, count);
    // 每部分取雜湊最小的 `quota` 格當種子。
    let mut cells: Vec<usize> = (0..mask.len()).filter(|&i| mask[i]).collect();
    cells.sort_unstable_by_key(|&i| (part[i], at(seed, i as u64), i));
    let mut seeds = Vec::with_capacity(count);
    let mut start = 0;
    for (p, &q) in quota.iter().enumerate() {
        seeds.extend_from_slice(&cells[start..start + q]);
        start += sizes[p];
    }
    let mut owner = grow(mask, size, &seeds);
    for _ in 0..LLOYD_ROUNDS {
        seeds = centres(&owner, size, &seeds);
        owner = grow(mask, size, &seeds);
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

/// 從各種子往外長：每格歸給「能從自己的塊走到、離種子最近」的種子。
fn grow(mask: &[bool], size: (usize, usize), seeds: &[usize]) -> Vec<u32> {
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
                heap.push(Reverse((distance2(seeds[p as usize], n, size), n, p)));
            }
        }
    }
    owner
}

/// 兩格的直線距離平方；x 方向左右相接。
fn distance2(a: usize, b: usize, (width, _): (usize, usize)) -> u64 {
    let (dx, dy) = (offset(a % width, b % width, width), (a / width).abs_diff(b / width) as i64);
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
        let owner = split(&mask, (w, h), 17, 9).unwrap();
        for p in 0..17 {
            assert!(connected(&owner, (w, h), p), "part {p} is in one piece");
        }
        assert!(owner.iter().zip(&mask).all(|(&o, &m)| (o != u32::MAX) == m));
        let biggest = (0..17).map(|p| owner.iter().filter(|&&o| o == p).count()).max().unwrap();
        assert!(biggest < 3 * w * h / 17, "parts are roughly even, the biggest has {biggest}");
        assert_eq!(split(&mask, (w, h), 17, 9).unwrap(), owner, "same seed, same parts");
    }

    #[test]
    fn split_fails_when_it_cannot() {
        let mask = [true, false, true, false];
        assert!(split(&mask, (4, 1), 1, 0).unwrap_err().contains("2 separate pieces"));
        assert!(split(&mask, (4, 1), 3, 0).unwrap_err().contains("only 2 cells"));
    }
}
