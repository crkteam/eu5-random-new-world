//! eu5-rnw 的視窗（egui）。
//!
//! 版型：上方標題列、左側設定面板、中央地圖區、下方狀態列。
//! 中央先顯示原版地圖（新世界另外上色）；按「生成」後換成 `core/` 產生的新地圖。
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
    land_check: Task<(String, egui::TextureHandle)>,
    /// 步驟 9：整張圖與放大的一角。
    locations_check: Task<(String, egui::TextureHandle, egui::TextureHandle)>,
    /// 步驟 9b：原版區域、新地圖區域、放大的地區與省。
    ids_check: Task<(String, egui::TextureHandle, egui::TextureHandle, egui::TextureHandle)>,
    /// 步驟 10：完整解析度的新地點圖（縮圖與原尺寸的一角）。
    full_check: Task<(String, egui::TextureHandle, egui::TextureHandle)>,
    /// 步驟 11a：原版的高度貼花。
    relief_check: Task<(String, egui::TextureHandle)>,
    /// 產生器頁「生成」的結果；`Idle` 時中央顯示原版地圖。
    generated: Task<(String, egui::TextureHandle)>,
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
            land_check: Task::Idle,
            locations_check: Task::Idle,
            ids_check: Task::Idle,
            full_check: Task::Idle,
            relief_check: Task::Idle,
            generated: Task::Idle,
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
        ui.add_space(12.0);

        ui.strong("7b. 切出陸地與海洋");
        let (key, name) = PRESETS[self.preset];
        ui.weak(format!(
            "用「產生器」頁的世界類型（目前：{name}）與種子（{}），在 7a 的範圍（延伸 {} 像素）裡：\
             依陸地比例取最高的格子當陸地，範圍邊緣附近一律是海，再清掉太小的島；被陸地包住的小片水填成陸地（湖在 9b 從原版放進來）。參數照原專案各世界類型的預設值。\
             範圍裡的原版小島（百慕達、聖赫勒拿、復活節島等）保留；被新陸地碰到的島群整群搬到最近的外海（盡量維持緯度）。",
            self.seed, self.reach_px
        ));
        let (seed, reach) = (self.seed, self.reach_px);
        self.land_check.button(ui, move |ctx| land_map(ctx, seed, key, reach));
        self.land_check.show(ui, |ui, land| show_land(ui, land, wide_image));
        ui.add_space(12.0);

        ui.strong("8. 「生成」按鈕");
        ui.weak(
            "到「產生器」頁選世界類型和種子，按「生成」：中央會換成新地圖（和 7b 同一套做法）。\
             同一組設定按兩次結果相同；按「回到原版地圖」可以切回原版。",
        );
        ui.add_space(12.0);

        ui.strong("9a. 切成地點");
        ui.weak(format!(
            "用 7b 同一組設定（{name}、種子 {}）生成後，把新陸地切成和範圍內原版陸地地點加湖一樣多塊，海切成和原版海一樣多塊。每塊都上下左右相連，大小盡量平均。每個地點一種顏色、深色線是地點邊界；下面第二張是陸地中心附近放大（每格 4 像素）。",
            self.seed
        ));
        let (seed, reach) = (self.seed, self.reach_px);
        self.locations_check.button(ui, move |ctx| locations_map(ctx, seed, key, reach));
        self.locations_check.show(ui, |ui, (summary, whole, close)| {
            ui.label(summary);
            wide_image(ui, whole);
            ui.add_space(4.0);
            wide_image(ui, close);
        });
        ui.add_space(12.0);

        ui.strong("9b. 對應原版地點");
        ui.weak(format!(
            "接著 9a（{name}、種子 {}），照原版階層由上往下分：次大陸 → 區域 → 地區 → 省 → 地點，每一層的地點數照原版，每一刀沿原版各組排得較開的方向（東西或南北）切，相對位置才像原版。原版的湖放進它所屬省裡不碰水的地點。第一張是原版的區域，第二張是新地圖的區域，同一個區域同一種顏色（陸地亮、海暗，湖是淺藍）。第三張是陸地中心附近放大：顏色是地區，黑線是省界，淡線是地點邊界。",
            self.seed
        ));
        let (seed, reach) = (self.seed, self.reach_px);
        self.ids_check.button(ui, move |ctx| ids_map(ctx, seed, key, reach));
        self.ids_check.show(ui, |ui, (summary, vanilla, new, close)| {
            ui.label(summary);
            ui.label("原版的區域");
            wide_image(ui, vanilla);
            ui.label("新地圖的區域");
            wide_image(ui, new);
            ui.label("放大：地區與省");
            wide_image(ui, close);
        });
        ui.add_space(12.0);

        ui.strong("10. 放大成完整地圖並檢查");
        ui.weak(format!(
            "接著 9b（{name}、種子 {}），把每 4 像素一格的結果放大成遊戲用的 16384×8192 地點圖：生成的格子整格換成新地點，原版小島照原圖逐像素搬過去，其餘保留原版。然後檢查：新分配的地點每個剛好一塊、其他地點的塊數不比原版多（原版本來就有很多多塊的島）、每個地點都還有像素、陸地／海／湖的類別都對。有任何問題就列出來並失敗。圖用每個地點在遊戲裡的顏色：第一張每 8 像素取一點，第二張是陸地中心附近的原尺寸。",
            self.seed
        ));
        let (seed, reach) = (self.seed, self.reach_px);
        self.full_check.button(ui, move |ctx| full_map(ctx, seed, key, reach));
        self.full_check.show(ui, |ui, (summary, whole, close)| {
            ui.label(summary);
            wide_image(ui, whole);
            ui.add_space(4.0);
            wide_image(ui, close);
        });
        ui.add_space(12.0);

        ui.strong("11a. 讀原版的高度貼花");
        ui.weak(
            "美洲的高低起伏不在 heightmap.png，而在 16 塊 4×4 鋪滿世界的高度貼花（每塊 8192×4096，每個地點像素 2×2 個貼花像素）。這裡解開遊戲實際讀的 .bin（每個 128×128 小塊一段 LZAV 壓縮），拼成全世界、每 8 個地點像素取一點：藍是低於 4000（海底約 2234），綠→黃→褐→白是越來越高。再和原版地點比對：高於 4000 的應該是陸地，其餘是海或湖。",
        );
        self.relief_check.button(ui, relief_map);
        self.relief_check.show(ui, |ui, (summary, texture)| {
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

        let running = matches!(self.generated, Task::Running(_));
        let generate = egui::Button::new(if running { "生成中…" } else { "生成" });
        if ui.add_enabled_ui(!running, |ui| ui.add_sized([ui.available_width(), 32.0], generate)).inner.clicked() {
            let (key, _) = PRESETS[self.preset];
            let (seed, reach) = (self.seed, self.reach_px);
            self.generated.start(ui.ctx(), move |ctx| land_map(ctx, seed, key, reach));
        }
        if !matches!(self.generated, Task::Idle) && ui.button("回到原版地圖").clicked() {
            self.generated = Task::Idle;
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
            if matches!(self.generated, Task::Idle) {
                ui.weak("原版地圖（按「生成」換成新地圖）");
                self.vanilla.show(ui, |ui, map| show_map(ui, map, fit_image));
            } else {
                ui.weak("生成的新地圖");
                self.generated.show(ui, |ui, land| show_land(ui, land, fit_image));
            }
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

/// 畫生成的陸海圖、圖例與說明。
fn show_land(
    ui: &mut egui::Ui,
    (summary, texture): &(String, egui::TextureHandle),
    image: fn(&mut egui::Ui, &egui::TextureHandle),
) {
    ui.horizontal_wrapped(|ui| {
        for (color, name) in LAND_LEGEND {
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

/// 每格 4 原圖像素。
const STEP: usize = 4;

/// 步驟 7a、7b、8 共用：用 core 讀地圖並決定生成範圍。
fn canvas(reach_px: u32) -> Outcome<eu5_rnw_core::generate::Canvas> {
    let map_data = eu5_rnw_core::game::choose_game(None).map_err(|e| e.to_string())?.map_data();
    eu5_rnw_core::generate::read_canvas(&map_data, STEP, reach_px)
}

/// 步驟 7b 的圖例。
const LAND_LEGEND: [(u32, &str); 4] = [
    (0x6dbb4f, "新陸地"),
    (0x2f6f9f, "新海洋"),
    (0x1e3550, "範圍外與島群的海"),
    (0x9a9586, "範圍外與島群的陸地"),
];

/// 步驟 7b、8：在範圍內生成新世界，保留並必要時搬移原版島群。
fn land_map(ctx: &egui::Context, seed: u64, key: &str, reach_px: u32) -> Outcome<(String, egui::TextureHandle)> {
    use eu5_rnw_core::land::{Cell, components, preset};

    let params = preset(key).ok_or_else(|| format!("unknown world type {key}"))?;
    let c = canvas(reach_px)?;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let start = Instant::now();
    let world = eu5_rnw_core::generate::generate(&c, seed, &params, threads)?;
    let seconds = start.elapsed().as_secs_f64();

    let land: Vec<bool> = world.cells.iter().map(|&cell| cell == Cell::Land).collect();
    let (_, mut sizes) = components(&land, (c.width, c.height));
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    let total: usize = sizes.iter().sum();
    let inside = c.domain.iter().filter(|&&d| d).count();
    let big = sizes.iter().filter(|&&s| s * 100 >= total * 3).count();
    let largest: Vec<String> =
        sizes.iter().take(4).map(|&s| format!("{:.0}%", s as f64 * 100.0 / total.max(1) as f64)).collect();
    let moved = world.moves.iter().filter(|&&m| m != (0, 0)).count();
    let pixels: Vec<u8> = world
        .cells
        .iter()
        .zip(&world.labels)
        .flat_map(|(&cell, &label)| {
            let color = match cell {
                Cell::Land | Cell::Lake => LAND_LEGEND[0].0,
                Cell::Sea => LAND_LEGEND[1].0,
                Cell::Outside if c.sea[usize::from(label)] => LAND_LEGEND[2].0,
                Cell::Outside => LAND_LEGEND[3].0,
            };
            rgb(color)
        })
        .collect();
    let summary = format!(
        "種子 {seed}、{key}：陸地 {total} 格（範圍的 {:.1}%，目標 {:.1}%）；\
         {} 塊陸地，其中 {big} 塊佔陸地 3% 以上，最大幾塊 {}；\
         原版島群 {} 群，{moved} 群被新陸地碰到而搬到外海；\
         特徵寬度 {} 像素、{} 層、persistence {}、最小島 {} 像素；生成 {seconds:.2} 秒。",
        total as f64 * 100.0 / inside.max(1) as f64,
        params.land_share * 100.0,
        sizes.len(),
        largest.join("、"),
        world.groups.len(),
        params.noise.feature_px,
        params.noise.octaves,
        params.noise.persistence,
        params.min_island_px,
    );
    let options = egui::TextureOptions {
        mipmap_mode: Some(egui::TextureFilter::Linear),
        ..egui::TextureOptions::LINEAR
    };
    let image = egui::ColorImage::from_rgb([c.width, c.height], &pixels);
    Ok((summary, ctx.load_texture("land", image, options)))
}

/// 步驟 9a：生成後切成地點；回傳說明、整張圖、陸地中心附近的放大圖。
fn locations_map(
    ctx: &egui::Context,
    seed: u64,
    key: &str,
    reach_px: u32,
) -> Outcome<(String, egui::TextureHandle, egui::TextureHandle)> {
    use eu5_rnw_core::land::{Cell, preset};

    let params = preset(key).ok_or_else(|| format!("unknown world type {key}"))?;
    let c = canvas(reach_px)?;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let world = eu5_rnw_core::generate::generate(&c, seed, &params, threads)?;
    let start = Instant::now();
    let found = eu5_rnw_core::generate::cut_locations(&c, &world, seed)?;
    let seconds = start.elapsed().as_secs_f64();

    let (w, h) = (c.width, c.height);
    let part = &found.part;
    let color = |i: usize| -> u32 {
        let p = part[i];
        if p == u32::MAX {
            return if c.sea[usize::from(world.labels[i])] { LAND_LEGEND[2].0 } else { LAND_LEGEND[3].0 };
        }
        let right = i / w * w + (i + 1) % w;
        if (i + w < part.len() && part[i + w] != p) || part[right] != p {
            return 0x202020;
        }
        let r = eu5_rnw_core::rng::at(seed, u64::from(p));
        let (a, b) = ((r & 0xff) as u32, ((r >> 8) & 0xff) as u32);
        match world.cells[i] {
            Cell::Land | Cell::Lake => (0x60 + a / 2) << 16 | (0x90 + b / 3) << 8 | (0x50 + (a ^ b) / 4),
            _ => (0x20 + a / 8) << 16 | (0x50 + b / 4) << 8 | (0x90 + a / 3),
        }
    };
    let pixels: Vec<u8> = (0..part.len()).flat_map(|i| rgb(color(i))).collect();

    // 放大：陸地格子的平均位置附近 768×512 格。
    let land: Vec<usize> = (0..part.len()).filter(|&i| world.cells[i] == Cell::Land).collect();
    let (cx, cy) = (
        land.iter().map(|&i| i % w).sum::<usize>() / land.len().max(1),
        land.iter().map(|&i| i / w).sum::<usize>() / land.len().max(1),
    );
    let (cw, ch) = (768.min(w), 512.min(h));
    let (x0, y0) = (cx.saturating_sub(cw / 2).min(w - cw), cy.saturating_sub(ch / 2).min(h - ch));
    let close: Vec<u8> =
        (0..ch).flat_map(|y| (0..cw).map(move |x| (y0 + y) * w + x0 + x)).flat_map(|i| rgb(color(i))).collect();

    let size_px = |range: std::ops::Range<u32>| -> String {
        let mut sizes = vec![0usize; (range.end - range.start) as usize];
        for &p in part {
            if range.contains(&p) {
                sizes[(p - range.start) as usize] += 1;
            }
        }
        let area = STEP * STEP;
        let (min, max) = (sizes.iter().min().copied().unwrap_or(0), sizes.iter().max().copied().unwrap_or(0));
        let mean = sizes.iter().sum::<usize>() / sizes.len().max(1);
        format!("平均 {} 像素（最小 {}、最大 {}）", mean * area, min * area, max * area)
    };
    let l = found.land as u32;
    let summary = format!(
        "種子 {seed}、{key}：陸地 {} 個地點（含 9b 要放的湖），{}；海 {} 個，{}。切地點 {seconds:.2} 秒。",
        found.land,
        size_px(0..l),
        found.seas,
        size_px(l..l + found.seas as u32),
    );
    let options = egui::TextureOptions {
        mipmap_mode: Some(egui::TextureFilter::Linear),
        ..egui::TextureOptions::LINEAR
    };
    let whole = ctx.load_texture("locations", egui::ColorImage::from_rgb([w, h], &pixels), options);
    let close = ctx.load_texture("locations_close", egui::ColorImage::from_rgb([cw, ch], &close), egui::TextureOptions::NEAREST);
    Ok((summary, whole, close))
}

/// 步驟 10：放大成完整地圖並檢查；回傳說明、縮圖、陸地中心附近的原尺寸。
fn full_map(
    ctx: &egui::Context,
    seed: u64,
    key: &str,
    reach_px: u32,
) -> Outcome<(String, egui::TextureHandle, egui::TextureHandle)> {
    use eu5_rnw_core::generate::{assign_ids, cut_locations, generate};
    use eu5_rnw_core::land::{Cell, preset};

    let params = preset(key).ok_or_else(|| format!("unknown world type {key}"))?;
    let c = canvas(reach_px)?;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let world = generate(&c, seed, &params, threads)?;
    let found = cut_locations(&c, &world, seed)?;
    let done = assign_ids(&c, &world, &found)?;
    let start = Instant::now();
    let labels = eu5_rnw_core::validate::full_map(&c, &world, &found, &done);
    let built = start.elapsed().as_secs_f64();
    let report = eu5_rnw_core::validate::check(&c, &done, &labels)?;
    let checked = start.elapsed().as_secs_f64() - built;

    let new = eu5_rnw_core::locations::LocationMap {
        width: c.vanilla.width,
        height: c.vanilla.height,
        locations: c.vanilla.locations.clone(),
        labels,
    };
    let colors: Vec<[u8; 3]> = new.locations.iter().map(|l| rgb(l.color)).collect();
    let whole = preview(&new, 8, &colors);
    let land: Vec<usize> = (0..done.cells.len()).filter(|&i| done.cells[i] == Cell::Land).collect();
    let (cx, cy) = (
        land.iter().map(|&i| i % c.width).sum::<usize>() / land.len().max(1) * c.step,
        land.iter().map(|&i| i / c.width).sum::<usize>() / land.len().max(1) * c.step,
    );
    let (cw, ch) = (1024.min(new.width), 640.min(new.height));
    let (x0, y0) = (cx.saturating_sub(cw / 2).min(new.width - cw), cy.saturating_sub(ch / 2).min(new.height - ch));
    let close: Vec<u8> =
        (0..ch).flat_map(|y| (0..cw).map(move |x| (y0 + y) * new.width + x0 + x)).flat_map(|i| colors[usize::from(new.labels[i])]).collect();
    let summary = format!(
        "種子 {seed}、{key}：檢查通過。{} 個新分配的地點每個剛好一塊；definitions.txt 的 {} 個地點都有像素；{} 個地點是多塊（都不比原版多）。放大 {built:.2} 秒、檢查 {checked:.2} 秒。",
        report.assigned, report.placed, report.multi_piece,
    );
    Ok((
        summary,
        ctx.load_texture("full", whole, egui::TextureOptions::LINEAR),
        ctx.load_texture("full_close", egui::ColorImage::from_rgb([cw, ch], &close), egui::TextureOptions::NEAREST),
    ))
}

/// 高於這個值算陸地（原專案量到海底約 2234、陸地約 5500 以上）。
const LAND_HEIGHT: u16 = 4000;

/// 步驟 11a：全世界的高度縮圖，並和原版地點的陸地／海比對。
fn relief_map(ctx: &egui::Context) -> Outcome<(String, egui::TextureHandle)> {
    use eu5_rnw_core::geography::Kind;

    let game = eu5_rnw_core::game::choose_game(None).map_err(|e| e.to_string())?;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let start = Instant::now();
    let (w, h, heights) = eu5_rnw_core::terrain::sample_heights(&game.directory, 8, threads).map_err(|e| e.to_string())?;
    let seconds = start.elapsed().as_secs_f64();
    let map = read_location_map(&game.map_data()).map_err(|e| e.to_string())?;
    let geo = read_geography(&game.map_data(), &map.locations).map_err(|e| e.to_string())?;
    let new_world = geo.new_world()?;
    let (lw, lh, labels) = map.downsample(8);
    if (lw, lh) != (w, h) {
        return Err(format!("heights {w}x{h} and locations {lw}x{lh} do not line up"));
    }
    let wet = |l: usize| matches!(geo.kind[l], Kind::Sea | Kind::Lake);
    let agree = |only: &dyn Fn(usize) -> bool| {
        let points: Vec<usize> = (0..labels.len()).filter(|&i| only(usize::from(labels[i]))).collect();
        let same = points.iter().filter(|&&i| (heights[i] > LAND_HEIGHT) != wet(usize::from(labels[i]))).count();
        same as f64 * 100.0 / points.len().max(1) as f64
    };
    let median = |keep: &dyn Fn(usize) -> bool| {
        let mut v: Vec<u16> = (0..labels.len()).filter(|&i| keep(usize::from(labels[i]))).map(|i| heights[i]).collect();
        v.sort_unstable();
        v.get(v.len() / 2).copied().unwrap_or(0)
    };
    let pixels: Vec<u8> = heights
        .iter()
        .flat_map(|&v| {
            if v <= LAND_HEIGHT {
                let t = u32::from(v) * 255 / u32::from(LAND_HEIGHT);
                return rgb((0x10 + t / 6) << 16 | (0x30 + t / 4) << 8 | (0x60 + t / 3));
            }
            // 4000→綠、12000→黃、20000→褐、30000 以上→白。
            let stops = [(4000.0, [70.0, 140.0, 60.0]), (12000.0, [210.0, 200.0, 90.0]), (20000.0, [140.0, 95.0, 60.0]), (30000.0, [245.0, 245.0, 245.0])];
            let v = f64::from(v).min(30000.0);
            let k = stops.windows(2).position(|s| v <= s[1].0).unwrap_or(2);
            let (a, b) = (stops[k], stops[k + 1]);
            let t = (v - a.0) / (b.0 - a.0);
            [0, 1, 2].map(|c| (a.1[c] + (b.1[c] - a.1[c]) * t) as u8)
        })
        .collect();
    let summary = format!(
        "{w}×{h}（每 8 個地點像素一點）。解開 16 塊貼花 {seconds:.2} 秒。海的中位數 {}、陸地 {}。高度和原版陸地／海一致的比例：全世界 {:.1}%、新世界 {:.1}%。",
        median(&|l| geo.kind[l] == Kind::Sea),
        median(&|l| !wet(l)),
        agree(&|l| geo.continent[l].is_some()),
        agree(&|l| new_world[l]),
    );
    Ok((summary, ctx.load_texture("relief", egui::ColorImage::from_rgb([w, h], &pixels), egui::TextureOptions::LINEAR)))
}

/// 名稱的固定顏色（亮度 0x50～0xe0）。
fn name_color(name: &str) -> u32 {
    let h = name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| eu5_rnw_core::rng::mix64(h ^ u64::from(b)));
    let c = |shift: u32| 0x50 + ((h >> shift) & 0xff) as u32 * 0x90 / 0xff;
    c(0) << 16 | c(8) << 8 | c(16)
}

/// 海用暗一半的顏色。
fn dim(color: u32) -> u32 {
    (color >> 1) & 0x7f7f7f
}

/// 步驟 9b：切地點後對應原版 ID；回傳說明、原版區域圖、新區域圖、放大的地區與省。
fn ids_map(
    ctx: &egui::Context,
    seed: u64,
    key: &str,
    reach_px: u32,
) -> Outcome<(String, egui::TextureHandle, egui::TextureHandle, egui::TextureHandle)> {
    use eu5_rnw_core::land::{Cell, preset};

    let params = preset(key).ok_or_else(|| format!("unknown world type {key}"))?;
    let c = canvas(reach_px)?;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let world = eu5_rnw_core::generate::generate(&c, seed, &params, threads)?;
    let found = eu5_rnw_core::generate::cut_locations(&c, &world, seed)?;
    let start = Instant::now();
    let done = eu5_rnw_core::generate::assign_ids(&c, &world, &found)?;
    let seconds = start.elapsed().as_secs_f64();

    let (w, h) = (c.width, c.height);
    let mut pool = vec![false; c.parents.len()];
    for &l in &done.ids {
        pool[l] = true;
    }
    let level_of = |l: usize, level: usize| c.parents[l].as_ref().map_or("", |p| p[level].as_str());
    const LAKE: u32 = 0x9fd8f0;
    let region = |l: usize| {
        let color = name_color(level_of(l, 2));
        match () {
            _ if c.sea[l] => dim(color),
            _ if !c.land[l] => LAKE,
            _ => color,
        }
    };
    let outside = |l: usize| if c.sea[l] { LAND_LEGEND[2].0 } else { LAND_LEGEND[3].0 };
    let vanilla: Vec<u8> = c
        .labels
        .iter()
        .flat_map(|&l| {
            let l = usize::from(l);
            rgb(if pool[l] { region(l) } else { outside(l) })
        })
        .collect();
    let new: Vec<u8> = (0..found.part.len())
        .flat_map(|i| {
            let l = usize::from(done.labels[i]);
            rgb(if found.part[i] == u32::MAX { outside(l) } else { region(l) })
        })
        .collect();

    // 放大：陸地格子的平均位置附近 768×512 格；顏色是地區，黑線省界，淡線地點邊界。
    let land: Vec<usize> = (0..found.part.len()).filter(|&i| done.cells[i] == Cell::Land).collect();
    let (cx, cy) = (
        land.iter().map(|&i| i % w).sum::<usize>() / land.len().max(1),
        land.iter().map(|&i| i / w).sum::<usize>() / land.len().max(1),
    );
    let (cw, ch) = (768.min(w), 512.min(h));
    let (x0, y0) = (cx.saturating_sub(cw / 2).min(w - cw), cy.saturating_sub(ch / 2).min(h - ch));
    let close: Vec<u8> = (0..ch)
        .flat_map(|y| (0..cw).map(move |x| (y0 + y) * w + x0 + x))
        .flat_map(|i| {
            let l = usize::from(done.labels[i]);
            if found.part[i] == u32::MAX {
                return rgb(outside(l));
            }
            let others = [i / w * w + (i + 1) % w, (i + w).min(found.part.len() - 1)].map(|n| usize::from(done.labels[n]));
            let color = match () {
                _ if others.iter().any(|&n| level_of(n, 4) != level_of(l, 4)) => 0x101010,
                _ if !c.land[l] && !c.sea[l] => LAKE,
                _ if others.iter().any(|&n| n != l) => dim(name_color(level_of(l, 3))) + 0x202020,
                _ if c.sea[l] => dim(name_color(level_of(l, 3))),
                _ => name_color(level_of(l, 3)),
            };
            rgb(color)
        })
        .collect();

    let distinct = |level: usize| {
        let names: std::collections::BTreeSet<&str> = done.ids.iter().map(|&l| level_of(l, level)).collect();
        names.len()
    };
    let lakes = done.ids.iter().filter(|&&l| !c.land[l] && !c.sea[l]).count();
    let summary = format!(
        "種子 {seed}、{key}：{} 個原版地點各用一次（陸地 {}，其中湖 {lakes}；海 {}），分在 {} 個區域、{} 個地區、{} 個省。對應 ID {seconds:.2} 秒。",
        done.ids.len(),
        found.land,
        found.seas,
        distinct(2),
        distinct(3),
        distinct(4),
    );
    let options = egui::TextureOptions {
        mipmap_mode: Some(egui::TextureFilter::Linear),
        ..egui::TextureOptions::LINEAR
    };
    let image = |pixels: &[u8], size: [usize; 2]| egui::ColorImage::from_rgb(size, pixels);
    Ok((
        summary,
        ctx.load_texture("ids_vanilla", image(&vanilla, [w, h]), options),
        ctx.load_texture("ids_new", image(&new, [w, h]), options),
        ctx.load_texture("ids_close", image(&close, [cw, ch]), egui::TextureOptions::NEAREST),
    ))
}

/// 步驟 7a：生成範圍＝新世界＋延伸距離內、緩衝外的大西洋與太平洋，每 4 像素一格。
fn domain_map(ctx: &egui::Context, reach_px: u32) -> Outcome<(String, egui::TextureHandle)> {
    use eu5_rnw_core::domain::GUARD_PX;

    let c = canvas(reach_px)?;
    let (w, h) = (c.width, c.height);
    let (mut original, mut added) = (0usize, 0usize);
    let pixels: Vec<u8> = c
        .labels
        .iter()
        .zip(&c.domain)
        .flat_map(|(&label, &inside)| {
            let l = usize::from(label);
            let color = if c.new_world[l] {
                original += 1;
                DOMAIN_LEGEND[0].0
            } else if inside {
                added += 1;
                DOMAIN_LEGEND[1].0
            } else if c.ocean[l] && c.sea[l] {
                DOMAIN_LEGEND[2].0
            } else if c.sea[l] {
                DOMAIN_LEGEND[3].0
            } else {
                DOMAIN_LEGEND[4].0
            };
            rgb(color)
        })
        .collect();
    let summary = format!(
        "延伸 {reach_px} 像素、緩衝 {GUARD_PX} 像素：原版新世界 {original} 格，加上遠洋 {added} 格，範圍共 {} 格\
         （原版的 {:.1} 倍，佔整張地圖 {:.1}%）。",
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
