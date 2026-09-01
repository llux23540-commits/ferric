//! 磁盘持久化：与 eframe 的 `FileStorage` 兼容。
//!
//! 软渲染后端与 wgpu / glow 后端读写**同一个 `app.ron`**（`storage_dir(app_id)/app.ron`），
//! 这样用户在「软渲染」和「GPU」之间来回切换，设置、草稿、窗口尺寸都不会丢。
//! 序列化格式（RON 编码的 `HashMap<String, String>`）刻意与 eframe 保持一致。

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;

pub struct FileStorage {
    path: PathBuf,
    kv: HashMap<String, String>,
    dirty: bool,
}

impl FileStorage {
    /// 按 app_id 定位数据目录并读入已有状态。目录建不了就返回 `None`（不持久化，
    /// 但应用照常启动 —— 持久化不该成为打不开的理由）。
    pub fn from_app_id(app_id: &str) -> Option<Self> {
        let dir = eframe::storage_dir(app_id)?;
        if let Err(err) = std::fs::create_dir_all(&dir) {
            log::warn!("软渲染持久化目录创建失败：{} · {err}", dir.display());
            return None;
        }
        let path = dir.join("app.ron");
        let kv = read_ron(&path).unwrap_or_default();
        Some(Self {
            path,
            kv,
            dirty: false,
        })
    }
}

impl eframe::Storage for FileStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.kv.get(key).cloned()
    }

    fn set_string(&mut self, key: &str, value: String) {
        if self.kv.get(key) != Some(&value) {
            self.kv.insert(key.to_owned(), value);
            self.dirty = true;
        }
    }

    fn remove_string(&mut self, key: &str) {
        self.kv.remove(key);
        self.dirty = true;
    }

    fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(file) = std::fs::File::create(&self.path) {
            let mut writer = std::io::BufWriter::new(file);
            let config = Default::default();
            if ron::Options::default()
                .to_io_writer_pretty(&mut writer, &self.kv, config)
                .and_then(|()| writer.flush().map_err(|err| err.into()))
                .is_err()
            {
                log::warn!("软渲染状态写盘失败：{}", self.path.display());
            }
        }
    }
}

fn read_ron<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Option<T> {
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    ron::de::from_reader(reader).ok()
}

/// 可共享的存储句柄：软渲染后端自己保留一份 [`std::sync::Arc`]，同时把本包装
/// 注入 `FerricApp`（`new_soft` 的 storage），两处读写的是同一份 [`FileStorage`]。
/// 这样「关闭 / 重启时落盘」和「应用内部读草稿」不会各写各的。
#[derive(Clone)]
pub struct SharedStorage(pub std::sync::Arc<std::sync::Mutex<FileStorage>>);

impl eframe::Storage for SharedStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.lock().ok()?.get_string(key)
    }

    fn set_string(&mut self, key: &str, value: String) {
        if let Ok(mut inner) = self.0.lock() {
            inner.set_string(key, value);
        }
    }

    fn remove_string(&mut self, key: &str) {
        if let Ok(mut inner) = self.0.lock() {
            inner.remove_string(key);
        }
    }

    fn flush(&mut self) {
        if let Ok(mut inner) = self.0.lock() {
            inner.flush();
        }
    }
}
