//! eu5-rnw 的視窗（egui）。
//!
//! 版型：上方標題列、左側設定面板、中央地圖區、下方狀態列。
//! 中央先顯示原版地圖（新世界另外上色）；「生成」之後會呼叫 `core/` 的產生器。
//! 讀檔等慢的工作都在背景執行緒做，視窗不會卡住。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use eu5_rnw_core::geography::{Kind, read_geography};
use eu5_rnw_core::locations::read_location_map;

/// 原專案的世界類型（之後由 core 提供）。
const PRESETS: [(&str, &str); 6] = [
    ("continents", "大陸"),
    ("supercontinent", "超大陸"),
    ("small_continents", "小型大陸"),
    ("archipelago", "群島"),
    ("islands", "島嶼"),
    ("wall", "高牆"),
];

type Outcome<T> = Result<T, String>;

/// 一件在背景執行緒做的工作，與它的結果和總耗時。
enum Task<T> {
    Idle,
    Running(mpsc::Receiver<(Outcome<T>, Duration)>),
    Done(Outcome<T>, Duration),
}

impl<T: Send + 'static> Task<T> {
    fn start(
        &mut self,
        ctx: &egui::Context,
        work: impl FnOnce(&egui::Context) -> Outcome<T> + Send + 'static,
    ) {
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            let outcome = work(&ctx);
            // 收不到代表視窗已關閉，結果不需要了。
            let _ = sender.send((outcome, start.elapsed()));
            ctx.request_repaint();
        });
        *self = Task::Running(receiver);
    }

    fn poll(&mut self) {
        if let Task::Running(receiver) = self {
            match receiver.try_recv() {
                Ok((outcome, took)) => *self = Task::Done(outcome, took),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    *self = Task::Done(Err("背景工作意外中止（程式錯誤）".into()), Duration::ZERO);
                }
            }
        }
    }

    /// 畫出狀態：執行中、錯誤（紅字）或交給 `done` 畫結果，後面附總耗時。
    fn show(&mut self, ui: &mut egui::Ui, done: impl FnOnce(&mut egui::Ui, &T)) {
        self.poll();
        match self {
            Task::Idle => {}
            Task::Running(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("執行中…");
                });
            }
            Task::Done(outcome, took) => {
                match outcome {
                    Ok(value) => done(ui, value),
                    Err(message) => {
                        ui.colored_label(ui.visuals().error_fg_color, message.as_str());
                    }
                }
                ui.weak(format!("總耗時 {:.2} 秒", took.as_secs_f64()));
            }
        }
    }

    /// 「執行」按鈕；執行中不能再按。
    fn button(
        &mut self,
        ui: &mut egui::Ui,
        work: impl FnOnce(&egui::Context) -> Outcome<T> + Send + 'static,
    ) {
        let running = matches!(self, Task::Running(_));
        if ui.add_enabled(!running, egui::Button::new("執行")).clicked() {
            self.start(ui.ctx(), work);
        }
    }
}

struct App {
    preset: usize,
    seed: u64,
    status: String,
    /// 顯示「步驟測試」頁而不是產生器。
    show_steps: bool,
    game_check: Task<String>,
    map_check: Task<(String, egui::TextureHandle)>,
    geography_check: Task<String>,
    rng_check: Task<String>,
    height_check: Task<(String, egui::TextureHandle)>,
    /// 步驟 7a：遠洋延伸距離（原圖像素）與範圍圖。
    reach_px: u32,
    domain_check: Task<(String, egui::TextureHandle)>,
    /// 中央的原版地圖（步驟 4），啟動時開始讀。
    vanilla: Task<(String, egui::TextureHandle)>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let status = match load_cjk_font(&cc.egui_ctx) {
            Ok(()) => game_status().unwrap_or_else(|message| message),
            Err(message) => message,
        };
        let mut vanilla = Task::Idle;
        vanilla.start(&cc.egui_ctx, vanilla_map);
        Self {
            preset: 0,
            seed: 319_218_363,
            status,
            show_steps: false,
            game_check: Task::Idle,
            map_check: Task::Idle,
            geography_check: Task::Idle,
            rng_check: Task::Idle,
            height_check: Task::Idle,
            reach_px: eu5_rnw_core::domain::REACH_PX,
            domain_check: Task::Idle,
            vanilla,
        }
    }

    /// 每個已完成的步驟一個按鈕，讓擁有者自己看結果。
    fn steps(&mut self, ui: &mut egui::Ui) {
        ui.heading("步驟測試");
        ui.weak("每一步完成後會在這裡多一個按鈕。工作在背景執行，視窗不會卡住。");
        ui.add_space(12.0);

        ui.label("（看得到這頁的中文，就表示字型載入成功。）");
        ui.add_space(12.0);

        ui.strong("1. core 接進 workspace：找到 EU5");
        ui.weak("用 Steam 設定找出遊戲資料夾與 build 編號。");
        self.game_check.button(ui, |_| game_status());
        self.game_check.show(ui, |ui, text| {
            ui.label(text);
        });
        ui.add_space(12.0);

        ui.strong("2. 讀取 locations.png");
        ui.weak("讀整張 16384×8192 地點圖；預覽每 16 像素取一點，用各地點自己的顏色。");
        self.map_check.button(ui, check_locations);
        self.map_check.show(ui, |ui, (summary, texture)| {
            ui.label(summary);
            wide_image(ui, texture);
        });
        ui.add_space(12.0);

        ui.strong("3. 找出新世界的地點");
        ui.weak("讀 definitions.txt 的階層與 default.map 的類別，數出大陸 america 底下的地點。");
        self.geography_check.button(ui, |_| check_geography());
        self.geography_check.show(ui, |ui, text| {
            ui.label(text);
        });
        ui.add_space(12.0);

        ui.strong("4. 在視窗中央顯示原版地圖");
        ui.weak("視窗啟動時就在背景讀取；結果在「產生器」頁中央，這裡也顯示一份。按「執行」重新讀。");
        self.vanilla.button(ui, vanilla_map);
        self.vanilla.show(ui, |ui, map| show_map(ui, map, wide_image));
        ui.add_space(12.0);

        ui.strong("5. 可重現的亂數");
        ui.weak(format!(
            "用「產生器」頁設定的種子（目前 {}）：前 5 個數產生兩次要相同；\
             再用 1、2、8 個和全部核心的執行緒各算前 1000 萬個數，檢查碼要全部相同。",
            self.seed
        ));
        let seed = self.seed;
        self.rng_check.button(ui, move |_| Ok(check_rng(seed)));
        self.rng_check.show(ui, |ui, text| {
            ui.label(text);
        });
        ui.add_space(12.0);

        ui.strong("6. 噪聲高度場");
        ui.weak(format!(
            "用目前的種子（{}）在新世界範圍內算分形噪聲，每 4 像素一格；白＝高、黑＝低，範圍外是深藍。\
             換種子再按一次，圖案應該完全不同；同一個種子按兩次則完全相同。",
            self.seed
        ));
        let seed = self.seed;
        self.height_check.button(ui, move |ctx| height_map(ctx, seed));
        self.height_check.show(ui, |ui, (summary, texture)| {
            ui.label(summary);
            wide_image(ui, texture);
        });
        ui.add_space(12.0);

        ui.strong("7a. 新世界的生成範圍");
        ui.weak(
            "照原專案的基本模式：原版新世界（綠），加上大西洋、太平洋裡距離它不超過「延伸距離」、\
             又離其他一切（舊世界陸地與近海、大洋洲、印度洋、南冰洋等）超過 96 像素緩衝的海域（亮藍）。",
        );
        ui.horizontal(|ui| {
            ui.label("延伸距離（原圖像素，原專案預設 1600）");
            ui.add(egui::Slider::new(&mut self.reach_px, 0..=4096).step_by(100.0));
        });
        let reach = self.reach_px;
        self.domain_check.button(ui, move |ctx| domain_map(ctx, reach));
        self.domain_check.show(ui, |ui, (summary, texture)| {
            ui.horizontal_wrapped(|ui| {
                for (color, name) in DOMAIN_LEGEND {
                    swatch(ui, color, name);
                }
            });
            ui.label(summary);
            wide_image(ui, texture);
        });
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("設定");
        ui.add_space(8.0);

        ui.label("世界類型");
        egui::ComboBox::from_id_salt("preset")
            .selected_text(PRESETS[self.preset].1)
            .show_ui(ui, |ui| {
                for (index, (_, name)) in PRESETS.iter().enumerate() {
                    ui.selectable_value(&mut self.preset, index, *name);
                }
            });
        ui.add_space(8.0);

        ui.label("種子");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut self.seed));
            if ui.button("隨機").clicked() {
                self.seed = random_seed();
            }
        });
        ui.add_space(16.0);

        if ui.add_sized([ui.available_width(), 32.0], egui::Button::new("生成")).clicked() {
            let (key, _) = PRESETS[self.preset];
            self.status = format!("尚未實作：將以 {key}、種子 {} 生成", self.seed);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("title").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("eu5-rnw");
                ui.label("EU5 隨機新世界產生器（Rust 重寫版）");
                ui.separator();
                ui.selectable_value(&mut self.show_steps, false, "產生器");
                ui.selectable_value(&mut self.show_steps, true, "步驟測試");
            });
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.label(&self.status);
        });
        if self.show_steps {
            egui::CentralPanel::default_margins().show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.steps(ui));
            });
            return;
        }
        egui::Panel::left("settings")
            .resizable(false)
            .exact_size(240.0)
            .show(ui, |ui| self.settings(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| {
            ui.weak("原版地圖（生成後會換成新地圖）");
            self.vanilla.show(ui, |ui, map| show_map(ui, map, fit_image));
        });
    }
}

/// 用 core 找 EU5，回報位置與 Steam build。
fn game_status() -> Outcome<String> {
    let game = eu5_rnw_core::game::choose_game(None).map_err(|e| e.to_string())?;
    Ok(format!(
        "找到 EU5：{}（build {}）",
        game.directory.display(),
        game.build_id.as_deref().unwrap_or("未知")
    ))
}

/// 步驟 2：讀地點圖，回報統計並做一張縮小預覽。
fn check_locations(ctx: &egui::Context) -> Outcome<(String, egui::TextureHandle)> {
    let game = eu5_rnw_core::game::choose_game(None).map_err(|e| e.to_string())?;
    let start = Instant::now();
    let map = read_location_map(&game.map_data()).map_err(|e| e.to_string())?;
    let seconds = start.elapsed().as_secs_f64();

    let counts = map.pixel_counts();
    let unused = counts.iter().filter(|&&c| c == 0).count();
    let (largest, pixels) = counts
        .iter()
        .enumerate()
        .max_by_key(|(_, c)| **c)
        .ok_or("顏色表沒有任何地點")?;
    let summary = format!(
        "{}×{} 像素，{} 個地點（{unused} 個沒有像素），最大的是 {}（{pixels} 像素），讀圖 {seconds:.2} 秒",
        map.width,
        map.height,
        map.locations.len(),
        map.locations[largest].key,
    );

    let colors: Vec<[u8; 3]> = map.locations.iter().map(|l| rgb(l.color)).collect();
    let texture = ctx.load_texture("locations", preview(&map, 16, &colors), egui::TextureOptions::NEAREST);
    Ok((summary, texture))
}

/// 步驟 3：每個地點的大陸與類別，數出新世界。
fn check_geography() -> Outcome<String> {
    let map_data = eu5_rnw_core::game::choose_game(None).map_err(|e| e.to_string())?.map_data();
    let map = read_location_map(&map_data).map_err(|e| e.to_string())?;
    let geo = read_geography(&map_data, &map.locations).map_err(|e| e.to_string())?;
    let unplaced = geo.unplaced(&map.locations, &map.pixel_counts())?;
    let new_world = geo.new_world()?;
    let count = |kind| (0..new_world.len()).filter(|&i| new_world[i] && geo.kind[i] == kind).count();
    Ok(format!(
        "{} 個大陸。新世界 {} 個地點（全部 {}）：陸地 {}、湖 {}、無法通行 {}、無主地 {}、海 {}。\n\
         另有 {} 個顏色表裡的地點不在 definitions.txt，地圖上也沒有像素。",
        geo.continents.len(),
        new_world.iter().filter(|&&n| n).count(),
        map.locations.len(),
        count(Kind::Land),
        count(Kind::Lake),
        count(Kind::Impassable),
        count(Kind::NonOwnable),
        count(Kind::Sea),
        unplaced.len(),
    ))
}

/// 步驟 5：同一個種子兩次產生的數列，以及不同執行緒數算出的檢查碼。
fn check_rng(seed: u64) -> String {
    use eu5_rnw_core::rng::{Rng, checksum};

    let first = |seed| {
        let mut rng = Rng::new(seed);
        (0..5).map(|_| rng.next_u64()).collect::<Vec<_>>()
    };
    let (a, b) = (first(seed), first(seed));
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get() as u64);
    let sums: Vec<(u64, u64)> =
        [1, 2, 8, cores].into_iter().map(|t| (t, checksum(seed, 10_000_000, t))).collect();
    let same = sums.iter().all(|&(_, sum)| sum == sums[0].1);
    let mut text = format!(
        "第一次：{a:?}\n第二次：{b:?}\n{}\n",
        if a == b { "兩次相同 ✔" } else { "兩次不同 ✘" }
    );
    for (threads, sum) in &sums {
        text += &format!("{threads} 個執行緒：檢查碼 {sum:016x}\n");
    }
    text += if same { "所有執行緒數結果相同 ✔" } else { "執行緒數不同、結果不同 ✘" };
    text
}

/// 地圖圖例：（是否新世界, 類別, 顏色, 說明）。無主地與無法通行用同一色。
const LEGEND: [(bool, &[Kind], u32, &str); 8] = [
    (false, &[Kind::Land], 0x9a9586, "舊世界陸地"),
    (false, &[Kind::Impassable, Kind::NonOwnable], 0x5e5a50, "舊世界荒地"),
    (false, &[Kind::Lake], 0x5a86ad, "舊世界湖"),
    (false, &[Kind::Sea], 0x1e3550, "舊世界海"),
    (true, &[Kind::Land], 0x6dbb4f, "新世界陸地"),
    (true, &[Kind::Impassable, Kind::NonOwnable], 0x3f6e32, "新世界荒地"),
    (true, &[Kind::Lake], 0x7fc4e8, "新世界湖"),
    (true, &[Kind::Sea], 0x2f6f9f, "新世界海"),
];

/// 步驟 4：原版地圖，每 4 像素取一點（4096×2048），依新世界與類別上色。
fn vanilla_map(ctx: &egui::Context) -> Outcome<(String, egui::TextureHandle)> {
    let map_data = eu5_rnw_core::game::choose_game(None).map_err(|e| e.to_string())?.map_data();
    let map = read_location_map(&map_data).map_err(|e| e.to_string())?;
    let geo = read_geography(&map_data, &map.locations).map_err(|e| e.to_string())?;
    geo.unplaced(&map.locations, &map.pixel_counts())?;
    let new_world = geo.new_world()?;
    let colors: Vec<[u8; 3]> = (0..map.locations.len())
        .map(|i| {
            let (.., color, _) = LEGEND
                .iter()
                .find(|(nw, kinds, ..)| *nw == new_world[i] && kinds.contains(&geo.kind[i]))
                .expect("LEGEND covers every kind for both worlds");
            rgb(*color)
        })
        .collect();
    let summary = format!(
        "{}×{} 像素的原版地圖，縮成 1/4 顯示；新世界 {} 個地點。",
        map.width,
        map.height,
        new_world.iter().filter(|&&n| n).count()
    );
    let options = egui::TextureOptions {
        mipmap_mode: Some(egui::TextureFilter::Linear),
        ..egui::TextureOptions::LINEAR
    };
    Ok((summary, ctx.load_texture("vanilla", preview(&map, 4, &colors), options)))
}

/// 畫地圖、圖例與說明。
fn show_map(
    ui: &mut egui::Ui,
    (summary, texture): &(String, egui::TextureHandle),
    image: fn(&mut egui::Ui, &egui::TextureHandle),
) {
    ui.horizontal_wrapped(|ui| {
        for (.., color, name) in LEGEND {
            swatch(ui, color, name);
        }
    });
    ui.label(summary);
    image(ui, texture);
}

/// 圖例的一格：色塊加說明。
fn swatch(ui: &mut egui::Ui, color: u32, name: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    let [r, g, b] = rgb(color);
    ui.painter().rect_filled(rect, 2.0, egui::Color32::from_rgb(r, g, b));
    ui.label(name);
    ui.add_space(8.0);
}

/// 步驟 7a 的圖例。
const DOMAIN_LEGEND: [(u32, &str); 5] = [
    (0x6dbb4f, "原版新世界"),
    (0x3a9ad9, "納入的遠洋"),
    (0x1e3550, "沒納入的大西洋、太平洋"),
    (0x2a4055, "其他海域"),
    (0x9a9586, "其他陸地、湖、荒地"),
];

/// 步驟 7a：生成範圍＝新世界＋延伸距離內、緩衝外的大西洋與太平洋，每 4 像素一格。
fn domain_map(ctx: &egui::Context, reach_px: u32) -> Outcome<(String, egui::TextureHandle)> {
    use eu5_rnw_core::domain::{GUARD_PX, OCEANS, new_world_domain};

    const STEP: usize = 4;
    let map_data = eu5_rnw_core::game::choose_game(None).map_err(|e| e.to_string())?.map_data();
    let map = read_location_map(&map_data).map_err(|e| e.to_string())?;
    let geo = read_geography(&map_data, &map.locations).map_err(|e| e.to_string())?;
    geo.unplaced(&map.locations, &map.pixel_counts())?;
    let new_world = geo.new_world()?;
    let ocean = geo.on_continents(&OCEANS)?;
    let sea: Vec<bool> = geo.kind.iter().map(|&k| k == Kind::Sea).collect();
    let (w, h, labels) = map.downsample(STEP);
    let start = Instant::now();
    let domain = new_world_domain(
        &labels,
        (w, h),
        (&new_world, &ocean, &sea),
        reach_px / STEP as u32,
        GUARD_PX / STEP as u32,
    );
    let seconds = start.elapsed().as_secs_f64();

    let (mut original, mut added) = (0usize, 0usize);
    let pixels: Vec<u8> = labels
        .iter()
        .zip(&domain)
        .flat_map(|(&label, &inside)| {
            let l = usize::from(label);
            let color = if new_world[l] {
                original += 1;
                DOMAIN_LEGEND[0].0
            } else if inside {
                added += 1;
                DOMAIN_LEGEND[1].0
            } else if ocean[l] && sea[l] {
                DOMAIN_LEGEND[2].0
            } else if sea[l] {
                DOMAIN_LEGEND[3].0
            } else {
                DOMAIN_LEGEND[4].0
            };
            rgb(color)
        })
        .collect();
    let summary = format!(
        "延伸 {reach_px} 像素、緩衝 {GUARD_PX} 像素：原版新世界 {original} 格，加上遠洋 {added} 格，範圍共 {} 格\
         （原版的 {:.1} 倍，佔整張地圖 {:.1}%）；算範圍 {seconds:.2} 秒。",
        original + added,
        (original + added) as f64 / original.max(1) as f64,
        (original + added) as f64 * 100.0 / (w * h) as f64,
    );
    let options = egui::TextureOptions {
        mipmap_mode: Some(egui::TextureFilter::Linear),
        ..egui::TextureOptions::LINEAR
    };
    let image = egui::ColorImage::from_rgb([w, h], &pixels);
    Ok((summary, ctx.load_texture("domain", image, options)))
}

/// 縮到剩下的寬和高都放得下（不能捲動的區域用）。
fn fit_image(ui: &mut egui::Ui, texture: &egui::TextureHandle) {
    ui.add(egui::Image::new(texture).shrink_to_fit());
}

/// 只依寬度縮放（捲動區用）：捲動區給的可用高度是畫面剩下的高度，越下面越小，
/// 用 `shrink_to_fit` 會把排在底部的圖縮到看不見。
fn wide_image(ui: &mut egui::Ui, texture: &egui::TextureHandle) {
    let width = ui.available_width();
    let [w, h] = texture.size();
    let size = egui::vec2(width, width * h as f32 / w as f32);
    ui.add(egui::Image::new(texture).fit_to_exact_size(size));
}

/// 步驟 6：新世界範圍內的噪聲高度場，畫成灰階圖。
fn height_map(ctx: &egui::Context, seed: u64) -> Outcome<(String, egui::TextureHandle)> {
    use eu5_rnw_core::noise::{NoiseParams, height_field};

    let map_data = eu5_rnw_core::game::choose_game(None).map_err(|e| e.to_string())?.map_data();
    let map = read_location_map(&map_data).map_err(|e| e.to_string())?;
    let geo = read_geography(&map_data, &map.locations).map_err(|e| e.to_string())?;
    geo.unplaced(&map.locations, &map.pixel_counts())?;
    let new_world = geo.new_world()?;

    const STEP: usize = 4;
    let (w, h, labels) = map.downsample(STEP);
    let mask: Vec<bool> = labels.iter().map(|&l| new_world[usize::from(l)]).collect();
    let params = NoiseParams::default();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let start = Instant::now();
    let field = height_field(seed, &params, (w, h, STEP), &mask, threads);
    let seconds = start.elapsed().as_secs_f64();

    let inside: Vec<f32> = mask.iter().zip(&field.values).filter(|(m, _)| **m).map(|(_, v)| *v).collect();
    let (min, max) = inside.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    let mean = inside.iter().map(|&v| f64::from(v)).sum::<f64>() / inside.len().max(1) as f64;
    let pixels: Vec<u8> = mask
        .iter()
        .zip(&field.values)
        .flat_map(|(&m, &v)| {
            if m {
                // 用實際最低、最高拉開對比。
                let g = ((v - min) / (max - min).max(f32::EPSILON) * 255.0) as u8;
                [g, g, g]
            } else {
                [0x10, 0x1c, 0x2c]
            }
        })
        .collect();
    let summary = format!(
        "種子 {seed}：{} 格（{w}×{h} 中的新世界），噪聲值 {min:.3}～{max:.3}，平均 {mean:.3}；\
         特徵寬度 {} 像素、{} 層、persistence {}；{threads} 個執行緒算噪聲 {seconds:.2} 秒。",
        inside.len(),
        params.feature_px,
        params.octaves,
        params.persistence,
    );
    let options = egui::TextureOptions {
        mipmap_mode: Some(egui::TextureFilter::Linear),
        ..egui::TextureOptions::LINEAR
    };
    let image = egui::ColorImage::from_rgb([w, h], &pixels);
    Ok((summary, ctx.load_texture("height", image, options)))
}

/// 每 `step` 像素取一點，用每個地點的顏色畫成縮圖。
fn preview(
    map: &eu5_rnw_core::locations::LocationMap,
    step: usize,
    colors: &[[u8; 3]],
) -> egui::ColorImage {
    let (w, h, labels) = map.downsample(step);
    let pixels: Vec<u8> = labels.iter().flat_map(|&l| colors[usize::from(l)]).collect();
    egui::ColorImage::from_rgb([w, h], &pixels)
}

fn rgb(color: u32) -> [u8; 3] {
    let [_, r, g, b] = color.to_be_bytes();
    [r, g, b]
}

/// egui 內建字型沒有中文字，從 Windows 載入微軟正黑體。
fn load_cjk_font(ctx: &egui::Context) -> Result<(), String> {
    let path = std::path::Path::new(r"C:\Windows\Fonts\msjh.ttc");
    let bytes = std::fs::read(path)
        .map_err(|e| format!("無法載入中文字型 {}：{e}", path.display()))?;
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("cjk".to_owned(), egui::FontData::from_owned(bytes).into());
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("cjk".to_owned());
    }
    ctx.set_fonts(fonts);
    Ok(())
}

/// 用時間當亂數來源；種子本身才是可重現的關鍵。
fn random_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    (nanos as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 1
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("eu5-rnw")
            .with_inner_size([1160.0, 740.0])
            .with_min_inner_size([860.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native("eu5-rnw", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
