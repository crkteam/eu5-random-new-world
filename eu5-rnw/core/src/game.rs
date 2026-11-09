//! 尋找 EU5 的安裝位置與 Steam 版本號（只讀，不修改任何東西）。
//!
//! 順序：`--game-dir` 參數 → 環境變數 `EU5_GAME_DIR` → Steam 的每個遊戲庫。

use std::path::{Path, PathBuf};

use crate::error::{Error, Result, read_text};
use crate::vdf;

/// EU5 的 Steam App ID。
pub const APP_ID: &str = "3450310";
pub const GAME_FOLDER: &str = "Europa Universalis V";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameInstall {
    pub directory: PathBuf,
    /// Steam 的 build id（例如 26377361）；不在 Steam 遊戲庫裡時為 None。
    pub build_id: Option<String>,
}

impl GameInstall {
    /// 地圖資料夾：`game/in_game/map_data`。
    pub fn map_data(&self) -> PathBuf {
        self.directory.join("game").join("in_game").join("map_data")
    }
}

/// 一個資料夾是不是 EU5：要有 `game/in_game/map_data/default.map`。
pub fn is_game(directory: &Path) -> bool {
    directory.join("game/in_game/map_data/default.map").is_file()
}

/// 可能的 Steam 根目錄（存在的才回傳，不重複）。
fn steam_roots() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        candidates.extend(registry_steam_path());
        candidates.push(r"C:\Program Files (x86)\Steam".into());
        candidates.push(r"C:\Program Files\Steam".into());
    } else if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        for relative in [".local/share/Steam", ".steam/steam", ".steam/root"] {
            candidates.push(home.join(relative));
        }
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for candidate in candidates {
        if candidate.is_dir() && !roots.iter().any(|r| same_path(r, &candidate)) {
            roots.push(candidate);
        }
    }
    roots
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// 從 Windows 登錄檔讀 Steam 的位置（用系統內建的 `reg query`，唯讀）。
fn registry_steam_path() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let output = std::process::Command::new("reg")
        .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().find(|l| l.contains("SteamPath"))?;
    let value = line.split("REG_SZ").nth(1)?.trim();
    (!value.is_empty()).then(|| PathBuf::from(value.replace('/', "\\")))
}

/// Steam 根目錄及 `libraryfolders.vdf` 列出的所有遊戲庫。
fn libraries(root: &Path) -> Vec<PathBuf> {
    let mut found = vec![root.to_path_buf()];
    let listing = root.join("steamapps").join("libraryfolders.vdf");
    if let Ok(text) = read_text(&listing)
        && let Ok(tree) = vdf::parse(&text)
        && let Some(folders) = tree.get("libraryfolders")
    {
        for (_, folder) in folders.entries() {
            if let Some(path) = folder.get("path").and_then(vdf::Node::text) {
                let path = PathBuf::from(path);
                if path.is_dir() && !found.iter().any(|f| same_path(f, &path)) {
                    found.push(path);
                }
            }
        }
    }
    found
}

/// 從 `appmanifest_3450310.acf` 讀 build id。
fn build_id(library: &Path) -> Option<String> {
    let manifest = library.join("steamapps").join(format!("appmanifest_{APP_ID}.acf"));
    let tree = vdf::parse(&read_text(&manifest).ok()?).ok()?;
    tree.get("AppState")?.get("buildid")?.text().map(str::to_owned)
}

/// 一個遊戲資料夾所在的 Steam 遊戲庫（`<庫>/steamapps/common/<遊戲>`）。
fn library_of(directory: &Path) -> Option<PathBuf> {
    let common = directory.parent()?;
    let steamapps = common.parent()?;
    (steamapps.file_name()? == "steamapps").then(|| steamapps.parent().map(Path::to_path_buf))?
}

fn install_at(directory: PathBuf) -> GameInstall {
    let build_id = library_of(&directory).and_then(|library| build_id(&library));
    GameInstall { directory, build_id }
}

/// 找到這台電腦上所有的 EU5 安裝。
pub fn find_games() -> Vec<GameInstall> {
    let mut games: Vec<GameInstall> = Vec::new();
    let mut add = |directory: PathBuf| {
        if is_game(&directory) && !games.iter().any(|g| same_path(&g.directory, &directory)) {
            games.push(install_at(directory));
        }
    };
    if let Some(dir) = std::env::var_os("EU5_GAME_DIR") {
        add(PathBuf::from(dir));
    }
    for root in steam_roots() {
        for library in libraries(&root) {
            add(library.join("steamapps").join("common").join(GAME_FOLDER));
        }
    }
    games
}

/// 使用者指定的資料夾，否則第一個找到的安裝。
pub fn choose_game(given: Option<&Path>) -> Result<GameInstall> {
    if let Some(directory) = given {
        if !is_game(directory) {
            return Err(Error::Usage(format!(
                "{} is not an EU5 installation (no game/in_game/map_data/default.map)",
                directory.display()
            )));
        }
        return Ok(install_at(directory.to_path_buf()));
    }
    find_games()
        .into_iter()
        .next()
        .ok_or_else(|| Error::Usage("No EU5 installation found; pass --game-dir".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_of_a_steam_game() {
        let dir = Path::new("D").join("lib").join("steamapps").join("common").join(GAME_FOLDER);
        assert_eq!(library_of(&dir), Some(Path::new("D").join("lib")));
        assert_eq!(library_of(Path::new("elsewhere/game")), None);
    }

    /// 需要本機安裝 EU5：`cargo test -p eu5-rnw-core -- --ignored --nocapture`
    #[test]
    #[ignore = "needs an installed EU5"]
    fn finds_the_installed_game() {
        let game = choose_game(None).expect("EU5 should be installed");
        println!("{} build {:?}", game.directory.display(), game.build_id);
        assert!(game.map_data().join("locations.png").is_file());
        assert!(game.build_id.is_some());
    }

    #[test]
    fn build_id_from_manifest() {
        let library = std::env::temp_dir().join(format!("eu5rnw-test-{}", std::process::id()));
        let steamapps = library.join("steamapps");
        std::fs::create_dir_all(&steamapps).unwrap();
        std::fs::write(
            steamapps.join(format!("appmanifest_{APP_ID}.acf")),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"3450310\"\n\t\"buildid\"\t\t\"26377361\"\n}\n",
        )
        .unwrap();
        assert_eq!(build_id(&library).as_deref(), Some("26377361"));
        std::fs::remove_dir_all(&library).unwrap();
    }
}
