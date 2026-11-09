//! 範圍內的原版小島：保留，並在被新陸地碰到時整群搬到外海。
//!
//! 照原專案的擴張模式（`old/src/rnw/eu5_pacific.py` 的 `_holes`、`_groups` 與 `islands.py`）：
//! 1. 範圍裡被完全包住、只含島嶼地點（大洋洲、大西洋、太平洋）的洞填進範圍。
//! 2. 彼此相距 `GROUP_PX` 內的島分成一群，連同碰到它們的小片海域（遊戲腳本會引用）。
//! 3. 陸地照常在整個範圍生成。
//! 4. 島群原地是外海、離陸地與範圍邊緣都有 `MARGIN_PX`，就留在原地；
//!    否則平移到成本最低的空位，成本 = 4×南北位移² + 東西位移²（盡量維持緯度）。找不到就失敗。
//! 5. 島群的格子在最後的位置換回原版。

use crate::domain::distance;
use crate::land::{Cell, components, neighbours};
use crate::locations::Label;

/// 可以搬移的島所屬的大陸。
pub const ISLAND_CONTINENTS: [&str; 3] = ["oceania", "atlantic_ocean_continent", "pacific_ocean_continent"];
/// 相距這麼近（原圖像素）的島算同一群。
pub const GROUP_PX: usize = 32;
/// 不超過這個面積（原圖像素）的海域，碰到島就跟著島走。
pub const COAST_ZONE_PX: u64 = 30_000;
/// 島群與新陸地、範圍邊緣、其他島群至少相隔（原圖像素）；原專案「大陸」的 coast_margin_px。
pub const MARGIN_PX: usize = 48;
/// 南北位移的成本權重。
const LATITUDE_WEIGHT: i64 = 4;
/// 搬移時最遠找到東西 `SEARCH_CELLS` 格、南北一半。
const SEARCH_CELLS: isize = 600;

/// 一個島群：它的地點（島與海域）與所佔的格子。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub locations: Vec<usize>,
    pub cells: Vec<usize>,
}

/// 把只含島嶼的封閉洞填進範圍。回傳新的範圍，以及洞裡的島嶼地點（以地點索引）。
///
/// `land` 是「這個地點是陸地」，`islandish` 是「這個地點在 `ISLAND_CONTINENTS`」。
pub fn fill_holes(
    labels: &[Label],
    (width, height): (usize, usize),
    domain: &[bool],
    land: &[bool],
    islandish: &[bool],
) -> (Vec<bool>, Vec<bool>) {
    let outside: Vec<bool> = domain.iter().map(|&d| !d).collect();
    let (part, sizes) = components(&outside, (width, height));
    // 碰到上下邊的塊，以及最大的那塊，是範圍外的世界；其他都是封閉的洞。
    let mut outer = vec![false; sizes.len()];
    if let Some(largest) = (0..sizes.len()).max_by_key(|&p| (sizes[p], usize::MAX - p)) {
        outer[largest] = true;
    }
    for x in 0..width {
        for i in [x, (height - 1) * width + x] {
            if part[i] != u32::MAX {
                outer[part[i] as usize] = true;
            }
        }
    }
    // 洞裡有陸地，而且陸地全是島嶼，才填。
    let (mut has_island, mut has_other) = (vec![false; sizes.len()], vec![false; sizes.len()]);
    for (i, &p) in part.iter().enumerate() {
        let l = usize::from(labels[i]);
        if p != u32::MAX && land[l] {
            if islandish[l] { has_island[p as usize] = true } else { has_other[p as usize] = true }
        }
    }
    let fill: Vec<bool> = (0..sizes.len()).map(|p| !outer[p] && has_island[p] && !has_other[p]).collect();
    let mut filled = domain.to_vec();
    let mut pinned = vec![false; land.len()];
    for (i, &p) in part.iter().enumerate() {
        if p != u32::MAX && fill[p as usize] {
            filled[i] = true;
            let l = usize::from(labels[i]);
            pinned[l] |= land[l];
        }
    }
    (filled, pinned)
}

/// 把要保留的島分群，並帶上碰到它們的小片海域（`coast_zone`）。一片海域碰到兩群時歸第一群。
pub fn groups(
    labels: &[Label],
    size: (usize, usize),
    step: usize,
    pinned: &[bool],
    coast_zone: &[bool],
) -> Result<Vec<Group>, String> {
    let near_cells = GROUP_PX.div_ceil(step) as u32;
    let island: Vec<bool> = labels.iter().map(|&l| pinned[usize::from(l)]).collect();
    let near: Vec<bool> = distance(&island, size, near_cells + 1).iter().map(|&d| d <= near_cells).collect();
    let (part, _) = components(&near, size);
    // 每個島屬於哪一塊；同一個島跨兩塊就失敗（原專案也是）。
    let mut cluster = vec![u32::MAX; pinned.len()];
    for (i, &is_island) in island.iter().enumerate() {
        if is_island {
            let l = usize::from(labels[i]);
            if cluster[l] != u32::MAX && cluster[l] != part[i] {
                return Err(format!("island location {l} lies in two island groups"));
            }
            cluster[l] = part[i];
        }
    }
    // 依每群最小的地點編號排序，結果固定。
    let mut order: Vec<(usize, u32)> =
        (0..pinned.len()).filter(|&l| pinned[l] && cluster[l] != u32::MAX).map(|l| (l, cluster[l])).collect();
    order.sort_unstable();
    let mut ids: Vec<u32> = Vec::new();
    for &(_, c) in &order {
        if !ids.contains(&c) {
            ids.push(c);
        }
    }
    let mut owner = vec![usize::MAX; pinned.len()];
    for &(l, c) in &order {
        owner[l] = ids.iter().position(|&id| id == c).expect("cluster listed");
    }
    for (i, &is_island) in island.iter().enumerate() {
        if !is_island {
            continue;
        }
        let group = owner[usize::from(labels[i])];
        for n in neighbours(i, size) {
            let zone = usize::from(labels[n]);
            if coast_zone[zone] && owner[zone] > group {
                owner[zone] = group;
            }
        }
    }
    let mut out: Vec<Group> = (0..ids.len()).map(|_| Group { locations: Vec::new(), cells: Vec::new() }).collect();
    for (l, &g) in owner.iter().enumerate() {
        if g != usize::MAX {
            out[g].locations.push(l);
        }
    }
    for (i, &label) in labels.iter().enumerate() {
        let g = owner[usize::from(label)];
        if g != usize::MAX {
            out[g].cells.push(i);
        }
    }
    Ok(out)
}

/// 每格到 `mask` 的距離是否不超過 `cells`。
fn within(mask: &[bool], size: (usize, usize), cells: usize) -> Vec<bool> {
    distance(mask, size, cells as u32 + 1).iter().map(|&d| d as usize <= cells).collect()
}

/// 平移後的格子；超出上下邊就是 `None`，x 方向左右相接。
fn shifted(index: usize, (width, height): (usize, usize), (dy, dx): (isize, isize)) -> Option<usize> {
    let y = (index / width) as isize + dy;
    if y < 0 || y >= height as isize {
        return None;
    }
    let x = ((index % width) as isize + dx).rem_euclid(width as isize);
    Some(y as usize * width + x as usize)
}

/// 每個島群的平移（南北, 東西，以格計）；留在原地是 (0, 0)。`margin` 以格計。
pub fn place(
    cells: &[Cell],
    domain: &[bool],
    size: (usize, usize),
    groups: &[Group],
    margin: usize,
) -> Result<Vec<(isize, isize)>, String> {
    let (width, height) = size;
    // 外海：碰得到範圍外的海。
    let sea: Vec<bool> = cells.iter().map(|&c| c == Cell::Sea).collect();
    let (part, sizes) = components(&sea, size);
    let mut open = vec![false; sizes.len()];
    for i in 0..sea.len() {
        if sea[i] && neighbours(i, size).any(|n| !domain[n]) {
            open[part[i] as usize] = true;
        }
    }
    let land: Vec<bool> = cells.iter().map(|&c| c == Cell::Land || c == Cell::Lake).collect();
    let mut edge: Vec<bool> = domain.iter().map(|&d| !d).collect();
    for x in 0..width {
        edge[x] = true;
        edge[(height - 1) * width + x] = true;
    }
    let near_land = within(&land, size, margin);
    let near_edge = within(&edge, size, margin);
    let forbidden: Vec<bool> = (0..cells.len())
        .map(|i| near_land[i] || near_edge[i] || !(sea[i] && open[part[i] as usize]))
        .collect();

    let mut moves: Vec<Option<(isize, isize)>> = vec![None; groups.len()];
    let mut taken = vec![false; cells.len()];
    for (g, group) in groups.iter().enumerate() {
        if group.cells.iter().all(|&i| !forbidden[i]) {
            moves[g] = Some((0, 0));
            group.cells.iter().for_each(|&i| taken[i] = true);
        }
    }
    let near_taken = within(&taken, size, margin);
    let mut blocked: Vec<bool> = (0..cells.len()).map(|i| forbidden[i] || near_taken[i]).collect();

    let mut offsets: Vec<(isize, isize)> = (-SEARCH_CELLS / 2..=SEARCH_CELLS / 2)
        .flat_map(|dy| (-SEARCH_CELLS..=SEARCH_CELLS).map(move |dx| (dy, dx)))
        .collect();
    let cost = |(dy, dx): (isize, isize)| LATITUDE_WEIGHT * (dy as i64).pow(2) + (dx as i64).pow(2);
    offsets.sort_unstable_by_key(|&o| (cost(o), o));

    let mut movers: Vec<usize> = (0..groups.len()).filter(|&g| moves[g].is_none()).collect();
    movers.sort_unstable_by_key(|&g| (usize::MAX - groups[g].cells.len(), g));
    for g in movers {
        let fits = |o| groups[g].cells.iter().all(|&i| shifted(i, size, o).is_some_and(|j| !blocked[j]));
        let Some(&found) = offsets.iter().find(|&&o| fits(o)) else {
            return Err(format!(
                "no free ocean holds the island group of {} cells; lower the land share or try another seed",
                groups[g].cells.len()
            ));
        };
        moves[g] = Some(found);
        let mut placed = vec![false; cells.len()];
        for &i in &groups[g].cells {
            placed[shifted(i, size, found).expect("checked by fits")] = true;
        }
        for (b, n) in blocked.iter_mut().zip(within(&placed, size, margin)) {
            *b |= n;
        }
    }
    Ok(moves.into_iter().map(|m| m.expect("every group placed")).collect())
}

/// 把島群放到最後的位置：那些格子變回範圍外，標籤換成島群原本的標籤。
pub fn carve(
    cells: &mut [Cell],
    labels: &mut [Label],
    size: (usize, usize),
    groups: &[Group],
    moves: &[(isize, isize)],
) {
    let original = labels.to_vec();
    for (group, &offset) in groups.iter().zip(moves) {
        for &i in &group.cells {
            let j = shifted(i, size, offset).expect("placed inside the map");
            cells[j] = Cell::Outside;
            labels[j] = original[i];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 由字元畫出標籤圖：每個字元是一個地點編號（'0'–'9'）。
    fn grid(rows: &[&str]) -> (Vec<Label>, (usize, usize)) {
        let labels = rows.iter().flat_map(|r| r.bytes().map(|b| Label::from(b - b'0'))).collect();
        (labels, (rows[0].len(), rows.len()))
    }

    #[test]
    fn fills_island_holes_only() {
        // 0 = 範圍外的世界，1 = 範圍內的海，2 = 島嶼（可搬），3 = 舊世界的島。
        let (labels, size) = grid(&["0000000000", "1111111111", "1121111311", "1111111111", "0000000000"]);
        let domain: Vec<bool> = labels.iter().map(|&l| l == 1).collect();
        let land = [true, false, true, true];
        let islandish = [false, false, true, false];
        let (filled, pinned) = fill_holes(&labels, size, &domain, &land, &islandish);
        assert!(filled[2 * 10 + 2], "the island hole is filled");
        assert!(!filled[2 * 10 + 7], "an Old World island stays a hole");
        assert!(!filled[0], "the world outside stays outside");
        assert_eq!(pinned, [false, false, true, false]);
    }

    #[test]
    fn groups_take_their_coast_zones() {
        // 2、3 是相鄰的兩個島，4 是碰到它們的小海域，5 是遠處的島，1 是大海。
        let (labels, size) = grid(&["1111111111", "1442111111", "1431111151", "1111111111"]);
        let pinned = [false, false, true, true, false, true];
        let coast = [false, false, false, false, true, false];
        // 每格 32 像素：GROUP_PX 是 1 格，所以 5 離得夠遠，自成一群。
        let found = groups(&labels, size, 32, &pinned, &coast).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].locations, [2, 3, 4]);
        assert_eq!(found[0].cells.len(), 5);
        assert_eq!(found[1].locations, [5]);
    }

    #[test]
    fn a_group_on_open_sea_stays_and_one_on_land_moves() {
        let (w, h) = (40, 20);
        // x 0–14 陸地、15–29 海、30–39 範圍外（擋住往左繞過接縫的路）。
        let domain: Vec<bool> = (0..w * h).map(|i| i % w < 30).collect();
        let cells: Vec<Cell> = (0..w * h)
            .map(|i| if !domain[i] { Cell::Outside } else if i % w < 15 { Cell::Land } else { Cell::Sea })
            .collect();
        let at = |x: usize, y: usize| y * w + x;
        let groups = vec![
            Group { locations: vec![0], cells: vec![at(24, 10)] },
            Group { locations: vec![1], cells: vec![at(5, 10), at(6, 10)] },
        ];
        let moves = place(&cells, &domain, (w, h), &groups, 2).unwrap();
        assert_eq!(moves[0], (0, 0), "open sea, away from land and edge");
        // 陸地到 x=14，邊距 2 格：x=17 起才行，同一緯度最近；繞過接縫往左要 25 格，比較貴。
        assert_eq!(moves[1], (0, 12));

        let mut cells = cells;
        let mut labels = vec![9 as Label; w * h];
        labels[at(5, 10)] = 1;
        labels[at(6, 10)] = 1;
        carve(&mut cells, &mut labels, (w, h), &groups, &moves);
        assert_eq!((cells[at(17, 10)], labels[at(17, 10)]), (Cell::Outside, 1));
        assert_eq!(cells[at(5, 10)], Cell::Land, "the old place keeps the new land");
    }

    #[test]
    fn placement_fails_without_room() {
        let (w, h) = (10, 6);
        let domain = vec![true; w * h];
        let cells = vec![Cell::Land; w * h];
        let groups = vec![Group { locations: vec![0], cells: vec![2 * w + 3] }];
        assert!(place(&cells, &domain, (w, h), &groups, 1).unwrap_err().contains("no free ocean"));
    }
}
