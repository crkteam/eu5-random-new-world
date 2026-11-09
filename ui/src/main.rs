//! eu5-rnw 的視窗（egui）。
//!
//! 版型：上方標題列、左側設定面板、中央地圖區、下方狀態列。
//! 目前只有畫面骨架；「生成」之後會呼叫 `core/` 的產生器。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;

/// 原專案的世界類型（之後由 core 提供）。
const PRESETS: [(&str, &str); 6] = [
    ("continents", "大陸"),
    ("supercontinent", "超大陸"),
    ("small_continents", "小型大陸"),
    ("archipelago", "群島"),
    ("islands", "島嶼"),
    ("wall", "高牆"),
];

struct App {
    preset: usize,
    seed: u64,
    status: String,
    /// 顯示「步驟測試」頁而不是產生器。
    show_steps: bool,
    /// 各步驟最近一次按下的結果。
    game_check: Option<String>,
    map_check: Option<Result<(String, egui::TextureHandle), String>>,
    geography_check: Option<Result<String, String>>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let status = match load_cjk_font(&cc.egui_ctx) {
            Ok(()) => game_status(),
            Err(message) => message,
        };
        Self {
            preset: 0,
            seed: 319_218_363,
            status,
            show_steps: false,
            game_check: None,
            map_check: None,
            geography_check: None,
        }
    }

    /// 每個已完成的步驟一個按鈕，讓擁有者自己看結果。
    fn steps(&mut self, ui: &mut egui::Ui) {
        ui.heading("步驟測試");
        ui.weak("每一步完成後會在這裡多一個按鈕。");
        ui.add_space(12.0);

        ui.label("（看得到這頁的中文，就表示字型載入成功。）");
        ui.add_space(12.0);

        ui.strong("1. core 接進 workspace：找到 EU5");
        ui.weak("用 Steam 設定找出遊戲資料夾與 build 編號。");
        if ui.button("執行").clicked() {
            self.game_check = Some(game_status());
        }
        if let Some(result) = &self.game_check {
            ui.label(result);
        }
        ui.add_space(12.0);

        ui.strong("2. 讀取 locations.png");
        ui.weak("讀整張 16384×8192 地點圖；預覽每 16 像素取一點，用各地點自己的顏色。");
        if ui.button("執行").clicked() {
            // shortcut: 在畫面執行緒上讀（release 約 0.5 秒會凍住視窗），之後步驟變慢再改背景執行緒
            self.map_check = Some(check_locations(ui.ctx()));
        }
        match &self.map_check {
            Some(Ok((summary, texture))) => {
                ui.label(summary);
                ui.add(egui::Image::new(texture).shrink_to_fit());
            }
            Some(Err(message)) => {
                ui.colored_label(ui.visuals().error_fg_color, message);
            }
            None => {}
        }
        ui.add_space(12.0);

        ui.strong("3. 找出新世界的地點");
        ui.weak("讀 definitions.txt 的階層與 default.map 的類別，數出大陸 america 底下的地點。");
        if ui.button("執行").clicked() {
            self.geography_check = Some(check_geography());
        }
        match &self.geography_check {
            Some(Ok(summary)) => {
                ui.label(summary);
            }
            Some(Err(message)) => {
                ui.colored_label(ui.visuals().error_fg_color, message);
            }
            None => {}
        }
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
            ui.centered_and_justified(|ui| {
                ui.weak("地圖預覽（生成後顯示）");
            });
        });
    }
}

/// 用 core 找 EU5，回報位置與 Steam build。
fn game_status() -> String {
    match eu5_rnw_core::game::choose_game(None) {
        Ok(game) => format!(
            "找到 EU5：{}（build {}）",
            game.directory.display(),
            game.build_id.as_deref().unwrap_or("未知")
        ),
        Err(error) => error.to_string(),
    }
}

/// 步驟 3：讀地點圖，回報統計並做一張縮小預覽。
fn check_locations(ctx: &egui::Context) -> Result<(String, egui::TextureHandle), String> {
    use eu5_rnw_core::locations::read_location_map;

    let game = eu5_rnw_core::game::choose_game(None).map_err(|e| e.to_string())?;
    let start = std::time::Instant::now();
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
        "{}×{} 像素，{} 個地點（{unused} 個沒有像素），最大的是 {}（{pixels} 像素），耗時 {seconds:.2} 秒",
        map.width,
        map.height,
        map.locations.len(),
        map.locations[largest].key,
    );

    const STEP: usize = 16;
    let (w, h) = (map.width / STEP, map.height / STEP);
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let color = map.locations[usize::from(map.label(x * STEP, y * STEP))].color;
            rgb.extend_from_slice(&color.to_be_bytes()[1..]);
        }
    }
    let image = egui::ColorImage::from_rgb([w, h], &rgb);
    Ok((summary, ctx.load_texture("locations", image, egui::TextureOptions::NEAREST)))
}

/// 步驟 3：每個地點的大陸與類別，數出新世界。
fn check_geography() -> Result<String, String> {
    use eu5_rnw_core::geography::{Kind, read_geography};

    let map_data = eu5_rnw_core::game::choose_game(None).map_err(|e| e.to_string())?.map_data();
    let map = eu5_rnw_core::locations::read_location_map(&map_data).map_err(|e| e.to_string())?;
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
