//! 整個程式共用的錯誤型別：錯了就明確失敗，不做靜默的備援。

use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum Error {
    /// 讀寫某個檔案失敗。
    Io { path: PathBuf, source: std::io::Error },
    /// 檔案內容不符合預期格式。
    Format { path: PathBuf, message: String },
    /// 其他使用上的錯誤（找不到遊戲、參數不對等）。
    Usage(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn io(path: &Path, source: std::io::Error) -> Self {
        Error::Io { path: path.to_path_buf(), source }
    }

    pub fn format(path: &Path, message: impl Into<String>) -> Self {
        Error::Format { path: path.to_path_buf(), message: message.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Error::Format { path, message } => write!(f, "{}: {message}", path.display()),
            Error::Usage(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

/// 讀取文字檔，去掉 Paradox 檔案常見的 UTF-8 BOM。
pub fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    String::from_utf8(body.to_vec()).map_err(|e| Error::format(path, format!("not UTF-8: {e}")))
}
