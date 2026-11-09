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

/// 生成範圍：新世界的格子，加上距離新世界不超過 `reach` 格、
/// 離「其他」格子超過 `guard` 格的遠洋海域格子。
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
    labels
        .iter()
        .enumerate()
        .map(|(i, &l)| {
            let l = usize::from(l);
            new_world[l] || (ocean[l] && sea[l] && near_new[i] <= reach && near_other[i] > guard)
        })
        .collect()
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
        // 地點 0 = 新世界，1 = 遠洋的海，2 = 舊世界近海，3 = 遠洋裡的島。
        let new_world = [true, false, false, false];
        let ocean = [false, true, false, true];
        let sea = [true, true, true, false];
        let labels: Vec<Label> = vec![0, 1, 1, 1, 1, 1, 2, 1, 1, 1, 3, 1, 1, 1];
        // 延伸 4 格、緩衝 1 格：舊世界近海（6）兩側各 1 格不納入，島（10）本身不納入，
        // 島旁邊的海照樣納入；最右邊的格子經由左右相接靠近新世界。
        let domain = new_world_domain(&labels, (14, 1), (&new_world, &ocean, &sea), 4, 1);
        let expect = [1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 1, 1, 1];
        assert_eq!(domain, expect.map(|e| e == 1));
    }
}
