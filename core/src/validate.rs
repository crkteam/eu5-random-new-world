//! 第 10 步：放大成完整解析度的地點圖，並檢查它（計畫第 10 步）。
//!
//! 放大：生成的格子整格換成新地點；島群照原圖逐像素搬到新位置（保留小島細節）；
//! 其餘保留原版像素，但原本屬於被重新分配或被搬走的地點的像素，換成那一格的標籤。
//!
//! 檢查（有問題就失敗，列出前幾個）：
//! 1. 新分配的地點剛好一塊；其他地點的塊數不比原版多（原版本來就有 1782 個多塊地點，多是島）。
//! 2. `definitions.txt` 裡的地點都還有像素；不在裡面的顏色沒有像素。
//! 3. 生成的格子類別與地點類別一致：陸地是陸地地點、海是海、湖是湖。

use crate::generate::{Assigned, Canvas, Locations, World};
use crate::land::Cell;
use crate::locations::Label;

/// 完整解析度的新地點圖（大小同原版）。
pub fn full_map(canvas: &Canvas, world: &World, found: &Locations, done: &Assigned) -> Vec<Label> {
    let map = &canvas.vanilla;
    let (w, step) = (canvas.width, canvas.step);
    // 每格若是島群的目的地：(島群, 平移)。
    let mut target: Vec<Option<(usize, (isize, isize))>> = vec![None; found.part.len()];
    let mut moved = vec![false; map.locations.len()];
    for (g, (group, &shift)) in world.groups.iter().zip(&world.moves).enumerate() {
        for &c in &group.cells {
            let y = (c / w) as isize + shift.0;
            let x = ((c % w) as isize + shift.1).rem_euclid(w as isize);
            target[y as usize * w + x as usize] = Some((g, shift));
        }
        for &l in &group.locations {
            moved[l] = true;
        }
    }
    for &l in &done.ids {
        moved[l] = true;
    }
    let mut out = vec![0 as Label; map.labels.len()];
    for (y, row) in out.chunks_exact_mut(map.width).enumerate() {
        for (x, pixel) in row.iter_mut().enumerate() {
            let i = (y / step).min(canvas.height - 1) * w + (x / step).min(w - 1);
            *pixel = match (found.part[i], target[i]) {
                (p, _) if p != u32::MAX => done.ids[p as usize] as Label,
                (_, Some((g, (dy, dx)))) => {
                    let sy = (y as isize - dy * step as isize) as usize;
                    let sx = (x as isize - dx * step as isize).rem_euclid(map.width as isize) as usize;
                    let v = map.labels[sy * map.width + sx];
                    if world.groups[g].locations.contains(&usize::from(v)) { v } else { done.labels[i] }
                }
                _ => {
                    let v = map.labels[y * map.width + x];
                    if moved[usize::from(v)] { done.labels[i] } else { v }
                }
            };
        }
    }
    out
}

/// 檢查通過時的摘要數字。
pub struct Report {
    /// 新分配的地點數。
    pub assigned: usize,
    /// 原版就是多塊、維持不變或變少的地點數。
    pub multi_piece: usize,
    pub placed: usize,
}

/// 檢查完整的新地點圖。
pub fn check(canvas: &Canvas, done: &Assigned, new: &[Label]) -> Result<Report, String> {
    let map = &canvas.vanilla;
    let placed: Vec<bool> = canvas.parents.iter().map(Option::is_some).collect();
    let size = (map.width, map.height);
    let count = map.locations.len();
    let (before, after) = std::thread::scope(|scope| {
        let before = scope.spawn(|| pieces(&map.labels, size, count));
        (before.join().expect("piece count panicked"), pieces(new, size, count))
    });
    let mut assigned = vec![false; count];
    for &l in &done.ids {
        assigned[l] = true;
    }
    let name = |l: usize| map.locations[l].key.as_str();
    let mut problems: Vec<String> = Vec::new();
    for l in 0..count {
        match (placed[l], after[l]) {
            (true, 0) => problems.push(format!("{} has no pixels", name(l))),
            (false, n) if n > 0 => problems.push(format!("{} is not in definitions.txt but has pixels", name(l))),
            (true, n) if assigned[l] && n != 1 => problems.push(format!("{} is in {n} pieces", name(l))),
            (true, n) if !assigned[l] && n > before[l] => {
                problems.push(format!("{} is in {n} pieces, vanilla {}", name(l), before[l]))
            }
            _ => {}
        }
    }
    for (i, &cell) in done.cells.iter().enumerate() {
        let l = usize::from(done.labels[i]);
        let fits = match cell {
            Cell::Outside => true,
            Cell::Land => canvas.land[l],
            Cell::Sea => canvas.sea[l],
            Cell::Lake => !canvas.land[l] && !canvas.sea[l],
        };
        if !fits {
            problems.push(format!("{} is not {cell:?} but fills a {cell:?} cell", name(l)));
            break;
        }
    }
    if !problems.is_empty() {
        let shown = problems.iter().take(8).cloned().collect::<Vec<_>>().join("; ");
        return Err(format!("{} problems in the new map: {shown}", problems.len()));
    }
    Ok(Report {
        assigned: done.ids.len(),
        multi_piece: (0..count).filter(|&l| after[l] > 1).count(),
        placed: placed.iter().filter(|&&p| p).count(),
    })
}

/// 每個地點有幾塊（上下左右相連；x 方向左右相接）。`count` 是地點總數。
///
/// 逐列找出同標籤的連續段，和上一列重疊的同標籤段併在一起（並查集），
/// 完整的 16384×8192 圖只需要幾百萬段，不必逐像素記錄塊編號。
pub fn pieces(labels: &[Label], (width, height): (usize, usize), count: usize) -> Vec<u32> {
    assert_eq!(labels.len(), width * height, "labels must cover the map");
    let mut parent: Vec<u32> = Vec::new();
    let mut runs_label: Vec<Label> = Vec::new();
    let mut previous: Vec<(usize, usize, u32)> = Vec::new();
    let mut current: Vec<(usize, usize, u32)> = Vec::new();
    for y in 0..height {
        let row = &labels[y * width..(y + 1) * width];
        current.clear();
        let mut start = 0;
        for x in 1..=width {
            if x == width || row[x] != row[start] {
                let id = parent.len() as u32;
                parent.push(id);
                runs_label.push(row[start]);
                current.push((start, x, id));
                start = x;
            }
        }
        // 左右相接：最後一段與第一段同標籤就是同一塊。
        if current.len() > 1 && row[0] == row[width - 1] {
            union(&mut parent, current[0].2, current[current.len() - 1].2);
        }
        // 和上一列重疊的同標籤段（兩個指標一起往右走）。
        let (mut a, mut b) = (0, 0);
        while a < previous.len() && b < current.len() {
            let (p, c) = (previous[a], current[b]);
            if p.0 < c.1 && c.0 < p.1 && runs_label[p.2 as usize] == runs_label[c.2 as usize] {
                union(&mut parent, p.2, c.2);
            }
            if p.1 <= c.1 { a += 1 } else { b += 1 }
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let mut out = vec![0u32; count];
    for id in 0..parent.len() as u32 {
        if find(&mut parent, id) == id {
            out[usize::from(runs_label[id as usize])] += 1;
        }
    }
    out
}

fn find(parent: &mut [u32], mut id: u32) -> u32 {
    while parent[id as usize] != id {
        parent[id as usize] = parent[parent[id as usize] as usize];
        id = parent[id as usize];
    }
    id
}

fn union(parent: &mut [u32], a: u32, b: u32) {
    let (a, b) = (find(parent, a), find(parent, b));
    if a != b {
        // 小的當根，結果與處理順序無關。
        let (low, high) = (a.min(b), a.max(b));
        parent[high as usize] = low;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(rows: &[&str]) -> (Vec<Label>, (usize, usize)) {
        let labels = rows.iter().flat_map(|r| r.bytes().map(|b| Label::from(b - b'0'))).collect();
        (labels, (rows[0].len(), rows.len()))
    }

    #[test]
    fn pieces_counts_connected_parts() {
        let (labels, size) = map(&["0110", "0010", "2222", "0003"]);
        let found = pieces(&labels, size, 4);
        assert_eq!(found[1], 1, "1 is an L shape");
        assert_eq!(found[2], 1);
        assert_eq!(found[3], 1);
        // 0：上面 (0,0),(0,1),(1,1) 經左右相接連到 (3,0),(3,1) 是一塊；最下排被 2 隔開是另一塊。
        assert_eq!(found[0], 2);
    }

    #[test]
    fn pieces_join_across_the_seam_and_diagonals_do_not_count() {
        let (labels, size) = map(&["1001", "0110"]);
        let found = pieces(&labels, size, 2);
        assert_eq!(found[1], 2, "(0,0)+(3,0) join across the seam; (1,1)+(2,1) are another piece");
        assert_eq!(found[0], 2, "(1,0)+(2,0) and the seam pair (0,1)+(3,1) only touch diagonally");
    }

    /// 需要本機安裝 EU5：每種世界類型、兩個種子都放大並通過檢查。
    #[test]
    #[ignore = "needs an installed EU5"]
    fn generated_maps_pass_the_checks() {
        use crate::generate::{assign_ids, cut_locations, generate, read_canvas};
        let map_data = crate::game::choose_game(None).unwrap().map_data();
        let canvas = read_canvas(&map_data, 4, crate::domain::REACH_PX).unwrap();
        for seed in [42, 7] {
            for (key, params) in crate::land::PRESETS {
                let world = generate(&canvas, seed, &params, 8).unwrap();
                let found = cut_locations(&canvas, &world, seed).unwrap();
                let done = assign_ids(&canvas, &world, &found).unwrap();
                let started = std::time::Instant::now();
                let new = full_map(&canvas, &world, &found, &done);
                let report = check(&canvas, &done, &new).unwrap_or_else(|e| panic!("seed {seed} {key}: {e}"));
                println!("seed {seed} {key}: ok, {} multi-piece, {:.2?}", report.multi_piece, started.elapsed());
                // 反向檢查：把一個新地點正中央的一個像素塗成另一個新地點，兩個地點都會多一塊。
                let mut broken = new.clone();
                let i = (0..found.part.len()).find(|&i| found.part[i] != u32::MAX && found.part.get(i + 1) == Some(&found.part[i])).unwrap();
                let (x, y) = ((i % canvas.width) * 4 + 1, (i / canvas.width) * 4 + 1);
                let other = done.ids[(found.part[i] as usize + 1) % done.ids.len()] as Label;
                broken[y * canvas.vanilla.width + x] = other;
                let error = check(&canvas, &done, &broken).err().expect("a stray pixel must fail");
                assert!(error.contains("pieces"), "{error}");
            }
        }
    }
}
