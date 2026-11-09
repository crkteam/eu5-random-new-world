//! 新世界的生成範圍，照原專案的基本模式（`old/src/rnw/eu5_canvas.py` 的 `select_canvas`）：
//! 原版新世界，加上大西洋、太平洋裡距離新世界 `reach` 以內、
//! 又離其他一切（舊世界陸地與近海、大洋洲等）超過 `guard` 的海域。
//!
//! 計算都在縮小的格子上做（每格 `step` 原圖像素），x 方向左右相接。

use crate::locations::Label;

/// 可以納入的遠洋大陸。
pub const OCEANS: [&str; 2] = ["atlantic_ocean_continent", "pacific_ocean_continent"];
/// 延伸距離與緩衝（原圖像素），原專案的預設值。
pub const REACH_PX: u32 = 1600;
pub const GUARD_PX: u32 = 96;

/// 每個格子到最近來源格子的步數，最多算到 `limit`（更遠的格子也是 `limit`）。
///
/// 一圈一圈往外擴，奇數圈走 8 個鄰居、偶數圈只走 4 個，得到近似圓形的八角形距離
/// （與原專案 `distance_from_wrapped` 相同）。x 方向左右相接，y 方向不相接。
pub fn distance(sources: &[bool], (width, height): (usize, usize), limit: u32) -> Vec<u32> {
    assert_eq!(sources.len(), width * height, "sources must cover the grid");
    let mut out = vec![limit; sources.len()];
    let mut frontier: Vec<usize> = Vec::new();
    for (index, &source) in sources.iter().enumerate() {
        if source {
            out[index] = 0;
            frontier.push(index);
        }
    }
    const SIDES: [(isize, isize); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    const CORNERS: [(isize, isize); 4] = [(-1, -1), (1, -1), (-1, 1), (1, 1)];
    let mut round = 0;
    while !frontier.is_empty() && round + 1 < limit {
        round += 1;
        let mut next = Vec::new();
        for &index in &frontier {
            let (x, y) = ((index % width) as isize, (index / width) as isize);
            let corners: &[(isize, isize)] = if round % 2 == 1 { &CORNERS } else { &[] };
            for &(dx, dy) in SIDES.iter().chain(corners) {
                let ny = y + dy;
                if ny < 0 || ny >= height as isize {
                    continue;
                }
                let nx = (x + dx).rem_euclid(width as isize);
                let neighbour = ny as usize * width + nx as usize;
                if out[neighbour] > round {
                    out[neighbour] = round;
                    next.push(neighbour);
                }
            }
        }
        frontier = next;
    }
    out
}

/// 生成範圍：新世界的格子，加上遠洋的整片海域：有一格距離新世界不超過 `reach` 格，
/// 而且每一格都離「其他」格子超過 `guard` 格。
///
/// `labels` 是縮小的標籤圖；以地點索引的 `new_world`、`ocean`（在 `OCEANS` 大陸裡）、`sea`。
/// 「其他」＝不是新世界也不在 `OCEANS` 大陸裡的地點；遠洋裡的島（百慕達等）不算其他，
/// 但它本身不是海，所以也不會被納入。
pub fn new_world_domain(
    labels: &[Label],
    size: (usize, usize),
    (new_world, ocean, sea): (&[bool], &[bool], &[bool]),
    reach: u32,
    guard: u32,
) -> Vec<bool> {
    let cells = |keep: &dyn Fn(usize) -> bool| -> Vec<bool> {
        labels.iter().map(|&l| keep(usize::from(l))).collect()
    };
    let near_new = distance(&cells(&|l| new_world[l]), size, reach + 1);
    let near_other = distance(&cells(&|l| !new_world[l] && !ocean[l]), size, guard + 1);
    // 以整個海域為單位（第 9 步要重用地點 ID，不能把一片海切成範圍內外兩半）：
    // 有一格在延伸距離內就算近，每一格都在緩衝外才算遠。
    let (mut near, mut far) = (vec![false; new_world.len()], vec![true; new_world.len()]);
    for (i, &l) in labels.iter().enumerate() {
        let l = usize::from(l);
        near[l] |= near_new[i] <= reach;
        far[l] &= near_other[i] > guard;
    }
    labels
        .iter()
        .map(|&l| {
            let l = usize::from(l);
            new_world[l] || (ocean[l] && sea[l] && near[l] && far[l])
        })
        .collect()
}

/// 比這個面積（原圖像素）小的範圍碎塊與範圍外小洞，是縮小取樣時參差邊界留下的。
pub const FRAGMENT_PX: usize = 1024;

/// 清掉邊界的碎塊（少於 `small` 格），讓範圍內外的地點在格子上都不會被切成孤立的小片：
/// 太小的範圍碎塊交給旁邊範圍外最常見的地點；太小、又不含島嶼陸地的範圍外小洞併進範圍，
/// 標籤換成旁邊範圍內最常見的地點。含島嶼陸地的洞留給 `islands::fill_holes`。
pub fn tidy(labels: &mut [Label], domain: &mut [bool], size: (usize, usize), small: usize, island_land: &[bool]) {
    for inside in [true, false] {
        let mask: Vec<bool> = domain.iter().map(|&d| d == inside).collect();
        let (part, sizes) = crate::land::components(&mask, size);
        let mut pieces: Vec<Vec<usize>> = vec![Vec::new(); sizes.len()];
        for (i, &p) in part.iter().enumerate() {
            if p != u32::MAX && sizes[p as usize] < small {
                pieces[p as usize].push(i);
            }
        }
        for cells in pieces.iter().filter(|c| !c.is_empty()) {
            if !inside && cells.iter().any(|&i| island_land[usize::from(labels[i])]) {
                continue;
            }
            // 上下邊的範圍外是整個世界的外圍，不是洞。
            if !inside && cells.iter().any(|&i| i < size.0 || i >= size.0 * (size.1 - 1)) {
                continue;
            }
            let mut counts = std::collections::BTreeMap::new();
            for &i in cells {
                for n in crate::land::neighbours(i, size) {
                    if domain[n] != inside {
                        *counts.entry(labels[n]).or_insert(0usize) += 1;
                    }
                }
            }
            let Some((&label, _)) = counts.iter().max_by_key(|&(&l, &c)| (c, std::cmp::Reverse(l))) else { continue };
            for &i in cells {
                labels[i] = label;
                domain[i] = !inside;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: &[&str]) -> (Vec<bool>, (usize, usize)) {
        let cells = rows.iter().flat_map(|r| r.chars().map(|c| c == '#')).collect();
        (cells, (rows[0].len(), rows.len()))
    }

    #[test]
    fn distance_is_octagonal() {
        let (sources, size) = grid(&[".......", ".......", ".......", "...#...", ".......", ".......", "......."]);
        let d = distance(&sources, size, 99);
        let at = |x: usize, y: usize| d[y * 7 + x];
        assert_eq!(at(3, 3), 0);
        assert_eq!((at(4, 3), at(4, 4)), (1, 1));
        assert_eq!((at(5, 3), at(5, 5)), (2, 3));
        // 位移 (3, 3)：斜、直、斜、直，4 步（歐氏距離 4.24）。
        assert_eq!((at(6, 3), at(6, 6)), (3, 4));
    }

    #[test]
    fn distance_wraps_in_x_but_not_y() {
        let (sources, size) = grid(&["#.....", "......", "......"]);
        let d = distance(&sources, size, 99);
        assert_eq!(d[5], 1, "left edge reaches the right edge");
        assert_eq!(d[2 * 6], 2, "two rows down");
    }

    #[test]
    fn distance_stops_at_the_limit() {
        let (sources, size) = grid(&["#........."]);
        assert_eq!(distance(&sources, size, 3), [0, 1, 2, 3, 3, 3, 3, 3, 2, 1]);
        let (none, size) = grid(&["...."]);
        assert_eq!(distance(&none, size, 5), [5, 5, 5, 5]);
    }

    #[test]
    fn domain_takes_near_ocean_away_from_others() {
        // 地點 0 = 新世界，1、4、5、6 = 遠洋的海，2 = 舊世界近海，3 = 遠洋裡的島。
        let new_world = [true, false, false, false, false, false, false];
        let ocean = [false, true, false, true, true, true, true];
        let sea = [true, true, true, false, true, true, true];
        let labels: Vec<Label> = vec![0, 1, 1, 4, 4, 5, 2, 6, 6, 6, 3, 6, 6, 1];
        // 延伸 2 格、緩衝 1 格：海 1 有格子靠近新世界（經由左右相接的最右格也算），整片納入；
        // 海 4 太遠；海 5 和 6 各有一格貼著舊世界近海，整片不納入；島 3 本身不是海。
        let domain = new_world_domain(&labels, (14, 1), (&new_world, &ocean, &sea), 2, 1);
        let expect = [1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        assert_eq!(domain, expect.map(|e| e == 1));
    }

    #[test]
    fn tidy_removes_fragments_and_pockets() {
        // 範圍（#）裡有一格範圍外的小洞（標籤 9），範圍外有一格孤立的範圍碎塊（標籤 5）；
        // 標籤 8 的洞是島嶼陸地，要留著。
        let (mut domain, size) = grid(&["......", ".###..", ".#.#.#", ".###..", ".#.#..", ".###.."]);
        let mut labels: Vec<Label> = domain.iter().map(|&d| if d { 1 } else { 2 }).collect();
        labels[2 * 6 + 2] = 9;
        labels[4 * 6 + 2] = 8;
        labels[2 * 6 + 5] = 5;
        let island_land: Vec<bool> = (0..10).map(|l| l == 8).collect();
        tidy(&mut labels, &mut domain, size, 2, &island_land);
        assert!(domain[2 * 6 + 2] && labels[2 * 6 + 2] == 1, "the pocket joins the domain");
        assert!(!domain[4 * 6 + 2] && labels[4 * 6 + 2] == 8, "an island hole stays");
        assert!(!domain[2 * 6 + 5] && labels[2 * 6 + 5] == 2, "the fragment goes to its outside neighbour");
        assert!(domain[6 + 1], "the main domain stays");
    }
}
