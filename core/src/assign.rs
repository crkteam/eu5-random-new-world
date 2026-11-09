//! 第 9b 步：給 9a 的每一塊一個原版地點 ID。
//!
//! 照原版階層（大陸 → 次大陸 → 區域 → 地區 → 省 → 地點）由上往下分，陸地（含湖）與海各分一次。
//! 每一層把手上的塊分給子節點，塊數剛好是子節點的原版地點數，用二分法：
//! 1. 軸取這些子節點在原版排得較開的方向（東西或南北），相對位置才像原版。
//! 2. 子節點依原版位置沿這個軸排序，切成地點數最接近一半的兩組。
//! 3. 從軸最前端的一塊開始，沿相鄰關係先取離它近的，長成一團第一組要的數量
//!    （碰到不相連的部分就從下一個最前面的塊重新開始），剩下的給第二組；兩組再各自往下分。
//!
//! 到了省：原版湖從一塊不碰水的開始長成一團（先取不碰水的），其餘給陸地。
//! 同分時依編號，結果固定。

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

use crate::generate::Canvas;
use crate::land::neighbours;

/// 每一塊的原版地點（索引同 `part` 的塊編號）：陸地塊 `0..land.len()`，海塊接在後面。
pub fn assign(canvas: &Canvas, part: &[u32], (land, sea): (&[usize], &[usize]), lake: &[bool]) -> Result<Vec<usize>, String> {
    let (width, height) = (canvas.width, canvas.height);
    if let Some(&l) = land.iter().chain(sea).find(|&&l| canvas.parents[l].is_none()) {
        return Err(format!("location {l} has no place in definitions.txt"));
    }
    let seam = seam(part, width)?;
    let ux = |i: usize| ((i % width) as i64 - seam as i64).rem_euclid(width as i64);
    let count = land.len() + sea.len();

    let mut piece = vec![(0i64, 0i64, 0i64); count];
    let mut adjacent: Vec<Vec<u32>> = vec![Vec::new(); count];
    let mut wet = vec![false; count];
    for (i, &p) in part.iter().enumerate() {
        if p == u32::MAX {
            continue;
        }
        let e = &mut piece[p as usize];
        *e = (e.0 + ux(i), e.1 + (i / width) as i64, e.2 + 1);
        for n in neighbours(i, (width, height)) {
            let q = part[n];
            if q != p && q != u32::MAX {
                adjacent[p as usize].push(q);
            }
            // 陸地塊旁邊是海塊或範圍外，就是碰水。
            wet[p as usize] |= q == u32::MAX || q as usize >= land.len();
        }
    }
    for a in &mut adjacent {
        a.sort_unstable();
        a.dedup();
    }
    let mut vanilla = vec![(0i64, 0i64, 0i64); canvas.parents.len()];
    for (i, &l) in canvas.labels.iter().enumerate() {
        let e = &mut vanilla[usize::from(l)];
        *e = (e.0 + ux(i), e.1 + (i / width) as i64, e.2 + 1);
    }
    let mut ctx = Ctx {
        parents: &canvas.parents,
        vanilla,
        centre: piece.iter().map(|&(x, y, n)| (x / n.max(1), y / n.max(1))).collect(),
        adjacent,
        wet,
        lake,
        mark: vec![0; count],
        stamp: 0,
        out: vec![usize::MAX; count],
    };
    ctx.node(land.to_vec(), 0, (0..land.len() as u32).collect());
    ctx.node(sea.to_vec(), 0, (land.len() as u32..count as u32).collect());
    Ok(ctx.out)
}

/// 沒有任何塊的一欄，當作東西方向的接縫（取最長空白的中間），座標從那裡算起才不會被左右相接切斷。
fn seam(part: &[u32], width: usize) -> Result<usize, String> {
    let mut used = vec![false; width];
    for (i, &p) in part.iter().enumerate() {
        used[i % width] |= p != u32::MAX;
    }
    let mut best = (0, 0);
    let mut run = 0;
    for x in 0..2 * width {
        run = if used[x % width] { 0 } else { run + 1 };
        best = best.max((run.min(width), x));
    }
    match best {
        (0, _) => Err("the New World covers every column; nowhere to cut the map open".into()),
        (run, end) => Ok((end + width - run / 2) % width),
    }
}

/// 長第一組時起點往後退多少（相對於這些塊沿軸的寬度，千分比）。
const BACK_PERMILLE: i64 = 500;

struct Ctx<'a> {
    parents: &'a [Option<[String; 5]>],
    /// 原版每個地點的格子座標和與格數。
    vanilla: Vec<(i64, i64, i64)>,
    /// 每塊的中心。
    centre: Vec<(i64, i64)>,
    adjacent: Vec<Vec<u32>>,
    wet: Vec<bool>,
    lake: &'a [bool],
    /// `take` 用來標記「在這次的塊裡／已取走」。
    mark: Vec<u32>,
    stamp: u32,
    out: Vec<usize>,
}

impl Ctx<'_> {
    /// `locs` 都在同一個第 `level - 1` 層節點裡；把 `pieces`（一樣多）分給它們。
    fn node(&mut self, locs: Vec<usize>, level: usize, pieces: Vec<u32>) {
        debug_assert_eq!(locs.len(), pieces.len());
        if locs.is_empty() {
            return;
        }
        if locs.len() == 1 {
            self.out[pieces[0] as usize] = locs[0];
            return;
        }
        if level == 5 {
            let (lakes, lands): (Vec<usize>, Vec<usize>) = locs.into_iter().partition(|&l| self.lake[l]);
            if !lakes.is_empty() && !lands.is_empty() {
                // 湖擠成一團：從一塊不碰水的開始長，先取不碰水的。
                let mut order = pieces.clone();
                order.sort_by_key(|&p| (self.wet[p as usize], p));
                let (water, rest) = self.take(pieces, lakes.len(), &order, true, (0, 0));
                self.bisect(lakes.into_iter().map(|l| vec![l]).collect(), 5, water);
                self.bisect(lands.into_iter().map(|l| vec![l]).collect(), 5, rest);
            } else {
                self.bisect(lakes.into_iter().chain(lands).map(|l| vec![l]).collect(), 5, pieces);
            }
            return;
        }
        let mut groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for &l in &locs {
            groups.entry(self.parents[l].as_ref().expect("checked in assign")[level].as_str()).or_default().push(l);
        }
        let groups: Vec<Vec<usize>> = groups.into_values().collect();
        if groups.len() == 1 {
            self.node(locs, level + 1, pieces);
        } else {
            self.bisect(groups, level, pieces);
        }
    }

    /// 把 `pieces` 分給同一層的幾組地點（塊數＝地點總數）。
    fn bisect(&mut self, mut groups: Vec<Vec<usize>>, level: usize, pieces: Vec<u32>) {
        if groups.len() == 1 {
            let locs = groups.pop().expect("one group");
            self.node(locs, level + 1, pieces);
            return;
        }
        let along = |c: (i64, i64), y: bool| if y { c.1 } else { c.0 };
        // 組的原版位置；整組都沒有格子（太小）時用全部的平均。
        let sums: Vec<(i64, i64, i64)> = groups
            .iter()
            .map(|g| g.iter().fold((0, 0, 0), |a, &l| {
                let v = self.vanilla[l];
                (a.0 + v.0, a.1 + v.1, a.2 + v.2)
            }))
            .collect();
        let all = sums.iter().fold((0, 0, 0), |a, v| (a.0 + v.0, a.1 + v.1, a.2 + v.2));
        let centre = |s: &(i64, i64, i64)| {
            let s = if s.2 > 0 { *s } else { all };
            (s.0 * 1000 / s.2.max(1), s.1 * 1000 / s.2.max(1))
        };
        // 軸取原版各組排得較開的方向，新地圖才會像原版那樣東西或南北相鄰。
        let span = |y: bool| {
            let values = sums.iter().map(|s| along(centre(s), y));
            values.clone().max().unwrap_or(0) - values.min().unwrap_or(0)
        };
        let y = span(true) >= span(false);
        let key = |s: &(i64, i64, i64)| along(centre(s), y);
        let mut order: Vec<usize> = (0..groups.len()).collect();
        order.sort_by_key(|&g| (key(&sums[g]), groups[g][0]));
        let mut sorted: Vec<Vec<usize>> = order.iter().map(|&g| std::mem::take(&mut groups[g])).collect();
        let total = pieces.len();
        let mut prefix = 0;
        let (mut cut, mut first) = (1, 0);
        let mut best = usize::MAX;
        for (k, g) in sorted.iter().enumerate().take(sorted.len() - 1) {
            prefix += g.len();
            let gap = (2 * prefix).abs_diff(total);
            if gap < best {
                (best, cut, first) = (gap, k + 1, prefix);
            }
        }
        // 從軸的最前端長出第一組。
        let mut order = pieces.clone();
        order.sort_by_key(|&p| (along(self.centre[p as usize], y), p));
        // 起點退到最前端後面半個寬度，長出來的介於圓團與直條之間，剩下的比較不會斷成月牙。
        let ends = |y: bool| {
            let v = pieces.iter().map(|&p| along(self.centre[p as usize], y));
            v.clone().max().unwrap_or(0) - v.min().unwrap_or(0)
        };
        let back = if y { (0, ends(true) * BACK_PERMILLE / 1000) } else { (ends(false) * BACK_PERMILLE / 1000, 0) };
        let (a, b) = self.take(pieces, first, &order, false, back);
        let second = sorted.split_off(cut);
        self.bisect(sorted, level, a);
        self.bisect(second, level, b);
    }

    /// 從 `order` 裡第一塊沒取走的開始，往相鄰的塊長成一團，取 `count` 塊；回傳（取走的, 剩下的）。
    /// 先取離起點近的（`dry_first` 時先取不碰水的），所以是一團而不是一條；
    /// 長不下去（不相連）就從 `order` 的下一塊重新開始。
    fn take(&mut self, pieces: Vec<u32>, count: usize, order: &[u32], dry_first: bool, back: (i64, i64)) -> (Vec<u32>, Vec<u32>) {
        self.stamp += 2;
        let (inside, taken) = (self.stamp, self.stamp + 1);
        for &p in &pieces {
            self.mark[p as usize] = inside;
        }
        let mut heap = BinaryHeap::new();
        let (mut got, mut next) = (Vec::with_capacity(count), 0);
        let mut start = (0, 0);
        let cost = |ctx: &Self, start: (i64, i64), p: u32| {
            let (x, y) = ctx.centre[p as usize];
            let wet = dry_first && ctx.wet[p as usize];
            Reverse((wet, (x - start.0).pow(2) + (y - start.1).pow(2), p))
        };
        while got.len() < count {
            let Some(Reverse((_, _, p))) = heap.pop() else {
                while self.mark[order[next] as usize] != inside {
                    next += 1;
                }
                let c = self.centre[order[next] as usize];
                start = (c.0 - back.0, c.1 - back.1);
                heap.push(cost(self, start, order[next]));
                continue;
            };
            if self.mark[p as usize] != inside {
                continue;
            }
            self.mark[p as usize] = taken;
            got.push(p);
            for &q in &self.adjacent[p as usize] {
                if self.mark[q as usize] == inside {
                    heap.push(cost(self, start, q));
                }
            }
        }
        let rest = pieces.into_iter().filter(|&p| self.mark[p as usize] == inside).collect();
        (got, rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seam_is_the_middle_of_the_widest_empty_run() {
        let m = u32::MAX;
        // 欄 0、1、6 有塊；2..=5 空（最長，中間取 3）、7 空。
        let part = [0, 1, m, m, m, m, 2, m];
        assert_eq!(seam(&part, 8).unwrap(), 3);
        // 空白跨過接縫：欄 6、7、0 空。
        let part = [m, 1, 1, 1, 1, 1, m, m];
        assert_eq!(seam(&part, 8).unwrap(), 7);
        assert!(seam(&[0, 1, 2], 3).is_err());
    }

    /// 一條 1×12 的陸地，原版兩個區域：西邊的區域 a（3 個地點）、東邊的 b（9 個，其中一省有湖）。
    #[test]
    fn regions_keep_their_side_and_lakes_stay_dry() {
        let w = 16;
        let part: Vec<u32> = (0..w as u32).map(|x| if (2..14).contains(&x) { x - 2 } else { u32::MAX }).collect();
        let path = |region: &str, province: &str| {
            Some(["america".into(), "north".into(), region.into(), format!("{region}_area"), province.into()])
        };
        // 地點 0..3 在 a；3..12 在 b，其中 3..6 是 b1 省（5 是湖），6..12 是 b2 省。
        let mut parents: Vec<Option<[String; 5]>> = Vec::new();
        for l in 0..12 {
            parents.push(match l {
                0..3 => path("a", "a1"),
                3..6 => path("b", "b1"),
                _ => path("b", "b2"),
            });
        }
        // 原版位置：a 在東、b 在西（顛倒過來，看排序是否照原版）。
        let labels: Vec<u16> = (0..w).map(|x| if x < 8 { 6 } else { 0 }).collect();
        let canvas = Canvas {
            width: w,
            height: 1,
            step: 4,
            labels,
            new_world: vec![true; 12],
            ocean: vec![false; 12],
            sea: vec![false; 12],
            land: (0..12).map(|l| l != 5).collect(),
            islandish: vec![false; 12],
            coast_zone: vec![false; 12],
            domain: part.iter().map(|&p| p != u32::MAX).collect(),
            parents,
        };
        let lake: Vec<bool> = (0..12).map(|l| l == 5).collect();
        let land: Vec<usize> = (0..12).collect();
        let ids = assign(&canvas, &part, (&land, &[]), &lake).unwrap();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, land, "every location is used once");
        let region = |p: usize| if ids[p] < 3 { 'a' } else { 'b' };
        let row: String = (0..12).map(region).collect();
        assert_eq!(row, "bbbbbbbbbaaa", "a lies east in vanilla, so it takes the east end");
        let lake_piece = ids.iter().position(|&l| l == 5).unwrap();
        assert!(lake_piece > 0, "the lake avoids the shore piece");
    }
}
